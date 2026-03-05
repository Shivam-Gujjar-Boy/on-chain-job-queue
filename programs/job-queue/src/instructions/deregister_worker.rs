use anchor_lang::prelude::*;
use crate::state::{Queue, Worker};
use crate::errors::JobQueueError;
use crate::events::WorkerDeregistered;

#[derive(Accounts)]
pub struct DeregisterWorker<'info> {
    pub authority: Signer<'info>,

    #[account(
        has_one = authority @ JobQueueError::Unauthorized,
    )]
    pub queue: Account<'info, Queue>,

    #[account(
        mut,
        has_one = queue,
        seeds = [b"worker", queue.key().as_ref(), worker.authority.as_ref()],
        bump = worker.bump,
    )]
    pub worker: Account<'info, Worker>,
}

pub fn handler(ctx: Context<DeregisterWorker>) -> Result<()> {
    let worker = &mut ctx.accounts.worker;
    worker.is_active = false;

    emit!(WorkerDeregistered {
        queue: ctx.accounts.queue.key(),
        worker: worker.authority,
        timestamp: Clock::get()?.unix_timestamp,
    });

    Ok(())
}
