use anchor_lang::prelude::*;
use crate::state::{Queue, Job, Worker, JobStatus};
use crate::errors::JobQueueError;
use crate::events::HeartbeatReceived;

#[derive(Accounts)]
pub struct Heartbeat<'info> {
    pub worker_authority: Signer<'info>,

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

pub fn handler(ctx: Context<Heartbeat>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;

    let job = &mut ctx.accounts.job;
    let worker = &mut ctx.accounts.worker;

    job.last_heartbeat = now;
    worker.last_active_at = now;

    emit!(HeartbeatReceived {
        queue: ctx.accounts.queue.key(),
        job_id: job.job_id,
        worker: ctx.accounts.worker_authority.key(),
        timestamp: now,
    });

    Ok(())
}
