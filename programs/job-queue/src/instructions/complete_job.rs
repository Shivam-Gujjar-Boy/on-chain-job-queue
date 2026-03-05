use anchor_lang::prelude::*;
use crate::state::{Queue, Job, Worker, JobStatus};
use crate::errors::JobQueueError;
use crate::events::JobCompleted;

#[derive(Accounts)]
pub struct CompleteJob<'info> {
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
        constraint = worker.authority == worker_authority.key() @ JobQueueError::UnauthorizedWorker,
        seeds = [b"worker", queue.key().as_ref(), worker_authority.key().as_ref()],
        bump = worker.bump,
    )]
    pub worker: Account<'info, Worker>,
}

pub fn handler(ctx: Context<CompleteJob>, result_hash: [u8; 32]) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;

    let job = &mut ctx.accounts.job;
    let queue = &mut ctx.accounts.queue;
    let worker = &mut ctx.accounts.worker;

    // Mark job as completed
    job.status = JobStatus::Completed;
    job.completed_at = now;
    job.result_hash = result_hash;

    // Update queue counters
    queue.active_count = queue.active_count.saturating_sub(1);
    queue.completed_count = queue
        .completed_count
        .checked_add(1)
        .ok_or(JobQueueError::ArithmeticOverflow)?;

    // Update worker stats
    worker.jobs_completed = worker
        .jobs_completed
        .checked_add(1)
        .ok_or(JobQueueError::ArithmeticOverflow)?;
    worker.last_active_at = now;

    emit!(JobCompleted {
        queue: queue.key(),
        job_id: job.job_id,
        worker: ctx.accounts.worker_authority.key(),
        result_hash,
        timestamp: now,
    });

    Ok(())
}
