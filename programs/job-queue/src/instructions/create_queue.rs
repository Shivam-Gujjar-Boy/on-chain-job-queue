use anchor_lang::prelude::*;
use crate::state::Queue;
use crate::errors::JobQueueError;
use crate::events::QueueCreated;

#[derive(Accounts)]
#[instruction(name: String)]
pub struct CreateQueue<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        init,
        payer = authority,
        space = 8 + Queue::INIT_SPACE,
        seeds = [b"queue", authority.key().as_ref(), name.as_bytes()],
        bump,
    )]
    pub queue: Account<'info, Queue>,

    pub system_program: Program<'info, System>,
}

pub fn handler(
    ctx: Context<CreateQueue>,
    name: String,
    max_retries: u8,
    job_timeout_seconds: i64,
    num_buckets: u8,
    require_authority_submit: bool,
) -> Result<()> {
    require!(!name.is_empty(), JobQueueError::NameEmpty);
    require!(name.len() <= 32, JobQueueError::NameTooLong);
    require!(
        num_buckets >= 1 && num_buckets <= 16,
        JobQueueError::InvalidBucketCount
    );
    require!(job_timeout_seconds > 0, JobQueueError::InvalidTimeout);
    require!(max_retries <= 10, JobQueueError::InvalidMaxRetries);

    let now = Clock::get()?.unix_timestamp;
    let queue = &mut ctx.accounts.queue;

    queue.authority = ctx.accounts.authority.key();
    queue.name = name.clone();
    queue.max_retries = max_retries;
    queue.job_timeout_seconds = job_timeout_seconds;
    queue.num_buckets = num_buckets;
    queue.buckets_initialized = 0;
    queue.total_jobs_created = 0;
    queue.pending_count = 0;
    queue.active_count = 0;
    queue.completed_count = 0;
    queue.failed_count = 0;
    queue.is_paused = false;
    queue.require_authority_submit = require_authority_submit;
    queue.created_at = now;
    queue.bump = ctx.bumps.queue;

    emit!(QueueCreated {
        queue: queue.key(),
        authority: queue.authority,
        name,
        num_buckets,
        max_retries,
        job_timeout_seconds,
        timestamp: now,
    });

    Ok(())
}
