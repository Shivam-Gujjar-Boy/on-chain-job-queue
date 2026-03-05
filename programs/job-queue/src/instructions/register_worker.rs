use anchor_lang::prelude::*;
use crate::state::{Queue, Worker};
use crate::errors::JobQueueError;
use crate::events::WorkerRegistered;

#[derive(Accounts)]
pub struct RegisterWorker<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        has_one = authority @ JobQueueError::Unauthorized,
    )]
    pub queue: Account<'info, Queue>,

    /// CHECK: The public key of the worker being registered. Validated via PDA derivation.
    pub worker_authority: UncheckedAccount<'info>,

    #[account(
        init,
        payer = authority,
        space = 8 + Worker::INIT_SPACE,
        seeds = [b"worker", queue.key().as_ref(), worker_authority.key().as_ref()],
        bump,
    )]
    pub worker: Account<'info, Worker>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<RegisterWorker>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;

    let worker = &mut ctx.accounts.worker;
    worker.queue = ctx.accounts.queue.key();
    worker.authority = ctx.accounts.worker_authority.key();
    worker.is_active = true;
    worker.jobs_completed = 0;
    worker.jobs_failed = 0;
    worker.registered_at = now;
    worker.last_active_at = now;
    worker.bump = ctx.bumps.worker;

    emit!(WorkerRegistered {
        queue: ctx.accounts.queue.key(),
        worker: ctx.accounts.worker_authority.key(),
        timestamp: now,
    });

    Ok(())
}
