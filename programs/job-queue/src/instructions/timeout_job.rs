use anchor_lang::prelude::*;
use crate::state::{Queue, Job, Bucket, JobStatus};
use crate::errors::JobQueueError;
use crate::events::JobTimedOut;

/// Permissionless crank instruction: anyone can call this to time out a stale job.
/// If the assigned worker has not sent a heartbeat within queue.job_timeout_seconds,
/// the job is either retried (if retries remain) or permanently marked as TimedOut.
#[derive(Accounts)]
pub struct TimeoutJob<'info> {
    /// Anyone can crank timeouts — no authority required
    pub cranker: Signer<'info>,

    #[account(mut)]
    pub queue: Account<'info, Queue>,

    #[account(
        mut,
        has_one = queue,
        constraint = job.status == JobStatus::Assigned @ JobQueueError::JobNotAssigned,
    )]
    pub job: Account<'info, Job>,

    #[account(
        mut,
        has_one = queue,
        seeds = [b"bucket", queue.key().as_ref(), &[job.bucket_index]],
        bump = bucket.bump,
    )]
    pub bucket: Account<'info, Bucket>,
}

pub fn handler(ctx: Context<TimeoutJob>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;

    let job = &mut ctx.accounts.job;
    let queue = &mut ctx.accounts.queue;
    let bucket = &mut ctx.accounts.bucket;

    // Verify that the job has actually timed out
    require!(
        now - job.last_heartbeat > queue.job_timeout_seconds,
        JobQueueError::JobNotTimedOut
    );

    if job.retry_count < job.max_retries {
        // Retry: return job to pending status
        job.status = JobStatus::Pending;
        job.retry_count = job
            .retry_count
            .checked_add(1)
            .ok_or(JobQueueError::ArithmeticOverflow)?;
        job.assigned_worker = Pubkey::default();
        job.assigned_at = 0;
        job.last_heartbeat = 0;

        // Update bucket pending count
        bucket.pending_count = bucket
            .pending_count
            .checked_add(1)
            .ok_or(JobQueueError::ArithmeticOverflow)?;

        // Update queue counters
        queue.active_count = queue.active_count.saturating_sub(1);
        queue.pending_count = queue
            .pending_count
            .checked_add(1)
            .ok_or(JobQueueError::ArithmeticOverflow)?;

        emit!(JobTimedOut {
            queue: queue.key(),
            job_id: job.job_id,
            retry_count: job.retry_count,
            retrying: true,
            timestamp: now,
        });
    } else {
        // No more retries: permanent timeout
        job.status = JobStatus::TimedOut;
        job.completed_at = now;

        // Update queue counters
        queue.active_count = queue.active_count.saturating_sub(1);
        queue.failed_count = queue
            .failed_count
            .checked_add(1)
            .ok_or(JobQueueError::ArithmeticOverflow)?;

        emit!(JobTimedOut {
            queue: queue.key(),
            job_id: job.job_id,
            retry_count: job.retry_count,
            retrying: false,
            timestamp: now,
        });
    }

    Ok(())
}
