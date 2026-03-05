use anchor_lang::prelude::*;
use crate::state::{Queue, Job, Bucket, Worker, JobStatus};
use crate::errors::JobQueueError;
use crate::events::{JobRetried, JobFailed};

#[derive(Accounts)]
pub struct FailJob<'info> {
    pub worker_authority: Signer<'info>,

    #[account(mut)]
    pub queue: Account<'info, Queue>,

    #[account(
        mut,
        has_one = queue,
        constraint = job.status == JobStatus::Assigned @ JobQueueError::JobNotAssigned,
        constraint = job.assigned_worker == worker_authority.key() @ JobQueueError::UnauthorizedWorker,
    )]
    pub job: Account<'info, Job>,

    #[account(
        mut,
        has_one = queue,
        seeds = [b"bucket", queue.key().as_ref(), &[job.bucket_index]],
        bump = bucket.bump,
    )]
    pub bucket: Account<'info, Bucket>,

    #[account(
        mut,
        has_one = queue,
        constraint = worker.authority == worker_authority.key() @ JobQueueError::UnauthorizedWorker,
        seeds = [b"worker", queue.key().as_ref(), worker_authority.key().as_ref()],
        bump = worker.bump,
    )]
    pub worker: Account<'info, Worker>,
}

pub fn handler(ctx: Context<FailJob>, error_code: u32) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;

    let job = &mut ctx.accounts.job;
    let queue = &mut ctx.accounts.queue;
    let bucket = &mut ctx.accounts.bucket;
    let worker = &mut ctx.accounts.worker;

    job.error_code = error_code;

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

        // Update bucket — job goes back to pending in its bucket
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

        emit!(JobRetried {
            queue: queue.key(),
            job_id: job.job_id,
            retry_count: job.retry_count,
            max_retries: job.max_retries,
            error_code,
            timestamp: now,
        });
    } else {
        // No more retries: permanent failure
        job.status = JobStatus::Failed;
        job.completed_at = now;

        // Update queue counters
        queue.active_count = queue.active_count.saturating_sub(1);
        queue.failed_count = queue
            .failed_count
            .checked_add(1)
            .ok_or(JobQueueError::ArithmeticOverflow)?;

        // Update worker failure stats
        worker.jobs_failed = worker
            .jobs_failed
            .checked_add(1)
            .ok_or(JobQueueError::ArithmeticOverflow)?;

        emit!(JobFailed {
            queue: queue.key(),
            job_id: job.job_id,
            worker: ctx.accounts.worker_authority.key(),
            error_code,
            retry_count: job.retry_count,
            timestamp: now,
        });
    }

    worker.last_active_at = now;

    Ok(())
}
