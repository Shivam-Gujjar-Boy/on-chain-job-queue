use anchor_lang::prelude::*;
use crate::state::Queue;
use crate::errors::JobQueueError;
use crate::events::QueueUpdated;

#[derive(Accounts)]
pub struct UpdateQueue<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        has_one = authority @ JobQueueError::Unauthorized,
    )]
    pub queue: Account<'info, Queue>,
}

pub fn handler(
    ctx: Context<UpdateQueue>,
    max_retries: Option<u8>,
    job_timeout_seconds: Option<i64>,
    is_paused: Option<bool>,
    require_authority_submit: Option<bool>,
) -> Result<()> {
    let queue = &mut ctx.accounts.queue;

    if let Some(mr) = max_retries {
        require!(mr <= 10, JobQueueError::InvalidMaxRetries);
        queue.max_retries = mr;
    }

    if let Some(timeout) = job_timeout_seconds {
        require!(timeout > 0, JobQueueError::InvalidTimeout);
        queue.job_timeout_seconds = timeout;
    }

    if let Some(paused) = is_paused {
        queue.is_paused = paused;
    }

    if let Some(ras) = require_authority_submit {
        queue.require_authority_submit = ras;
    }

    emit!(QueueUpdated {
        queue: queue.key(),
        authority: queue.authority,
        timestamp: Clock::get()?.unix_timestamp,
    });

    Ok(())
}
