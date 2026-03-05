use anchor_lang::prelude::*;
use crate::state::{Queue, Bucket};
use crate::errors::JobQueueError;
use crate::events::BucketInitialized;

#[derive(Accounts)]
#[instruction(bucket_index: u8)]
pub struct InitBucket<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        mut,
        has_one = authority @ JobQueueError::Unauthorized,
    )]
    pub queue: Account<'info, Queue>,

    #[account(
        init,
        payer = authority,
        space = 8 + Bucket::INIT_SPACE,
        seeds = [b"bucket", queue.key().as_ref(), &[bucket_index]],
        bump,
    )]
    pub bucket: Account<'info, Bucket>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitBucket>, bucket_index: u8) -> Result<()> {
    let queue = &mut ctx.accounts.queue;

    require!(
        bucket_index < queue.num_buckets,
        JobQueueError::InvalidBucketIndex
    );
    require!(
        bucket_index == queue.buckets_initialized,
        JobQueueError::BucketInitOutOfOrder
    );

    let bucket = &mut ctx.accounts.bucket;
    bucket.queue = queue.key();
    bucket.bucket_index = bucket_index;
    bucket.pending_count = 0;
    bucket.bump = ctx.bumps.bucket;

    queue.buckets_initialized += 1;

    emit!(BucketInitialized {
        queue: queue.key(),
        bucket: bucket.key(),
        bucket_index,
        timestamp: Clock::get()?.unix_timestamp,
    });

    Ok(())
}
