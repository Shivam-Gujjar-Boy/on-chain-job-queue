use anchor_lang::prelude::*;
use crate::state::{Queue, Job, Bucket, Worker, JobStatus};
use crate::errors::JobQueueError;
use crate::events::JobClaimed;

#[derive(Accounts)]
pub struct ClaimJob<'info> {
    pub worker_authority: Signer<'info>,

    #[account(
        mut,
        constraint = !queue.is_paused @ JobQueueError::QueuePaused,
    )]
    pub queue: Account<'info, Queue>,

    #[account(
        mut,
        has_one = queue,
        constraint = job.status == JobStatus::Pending @ JobQueueError::JobNotPending,
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
        constraint = worker.is_active @ JobQueueError::WorkerNotActive,
        seeds = [b"worker", queue.key().as_ref(), worker_authority.key().as_ref()],
        bump = worker.bump,
    )]
    pub worker: Account<'info, Worker>,
}

pub fn handler(ctx: Context<ClaimJob>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;

    let job = &mut ctx.accounts.job;
    let queue = &mut ctx.accounts.queue;
    let bucket = &mut ctx.accounts.bucket;
    let worker = &mut ctx.accounts.worker;

    // Assign the job to this worker
    job.status = JobStatus::Assigned;
    job.assigned_worker = ctx.accounts.worker_authority.key();
    job.assigned_at = now;
    job.last_heartbeat = now;

    // Update bucket pending count
    bucket.pending_count = bucket.pending_count.saturating_sub(1);

    // Update queue counters
    queue.pending_count = queue.pending_count.saturating_sub(1);
    queue.active_count = queue
        .active_count
        .checked_add(1)
        .ok_or(JobQueueError::ArithmeticOverflow)?;

    // Update worker last activity
    worker.last_active_at = now;

    emit!(JobClaimed {
        queue: queue.key(),
        job_id: job.job_id,
        worker: ctx.accounts.worker_authority.key(),
        bucket_index: job.bucket_index,
        timestamp: now,
    });

    Ok(())
}
