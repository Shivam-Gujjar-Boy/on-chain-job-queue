use anchor_lang::prelude::*;
use crate::state::{Queue, Job, Bucket, JobStatus};
use crate::errors::JobQueueError;
use crate::events::JobSubmitted;

#[derive(Accounts)]
pub struct SubmitJob<'info> {
    #[account(mut)]
    pub submitter: Signer<'info>,

    #[account(
        mut,
        constraint = !queue.is_paused @ JobQueueError::QueuePaused,
        constraint = queue.buckets_initialized == queue.num_buckets @ JobQueueError::QueueNotReady,
    )]
    pub queue: Account<'info, Queue>,

    #[account(
        init,
        payer = submitter,
        space = 8 + Job::INIT_SPACE,
        seeds = [
            b"job",
            queue.key().as_ref(),
            &queue.total_jobs_created.to_le_bytes(),
        ],
        bump,
    )]
    pub job: Account<'info, Job>,

    #[account(
        mut,
        seeds = [
            b"bucket",
            queue.key().as_ref(),
            &[(queue.total_jobs_created % queue.num_buckets as u64) as u8],
        ],
        bump = bucket.bump,
        constraint = bucket.queue == queue.key() @ JobQueueError::InvalidBucket,
    )]
    pub bucket: Account<'info, Bucket>,

    pub system_program: Program<'info, System>,
}

pub fn handler(
    ctx: Context<SubmitJob>,
    data_hash: [u8; 32],
    priority: u8,
) -> Result<()> {
    let queue = &mut ctx.accounts.queue;

    // Optionally restrict submission to authority only
    if queue.require_authority_submit {
        require!(
            ctx.accounts.submitter.key() == queue.authority,
            JobQueueError::UnauthorizedSubmitter
        );
    }

    let job_id = queue.total_jobs_created;
    let bucket_index = (job_id % queue.num_buckets as u64) as u8;
    let now = Clock::get()?.unix_timestamp;

    // Initialize the job account
    let job = &mut ctx.accounts.job;
    job.queue = queue.key();
    job.job_id = job_id;
    job.bucket_index = bucket_index;
    job.submitter = ctx.accounts.submitter.key();
    job.data_hash = data_hash;
    job.priority = priority;
    job.status = JobStatus::Pending;
    job.assigned_worker = Pubkey::default();
    job.retry_count = 0;
    job.max_retries = queue.max_retries;
    job.created_at = now;
    job.assigned_at = 0;
    job.completed_at = 0;
    job.last_heartbeat = 0;
    job.result_hash = [0u8; 32];
    job.error_code = 0;
    job.bump = ctx.bumps.job;

    // Update bucket pending count
    let bucket = &mut ctx.accounts.bucket;
    bucket.pending_count = bucket
        .pending_count
        .checked_add(1)
        .ok_or(JobQueueError::ArithmeticOverflow)?;

    // Update queue counters
    queue.total_jobs_created = queue
        .total_jobs_created
        .checked_add(1)
        .ok_or(JobQueueError::ArithmeticOverflow)?;
    queue.pending_count = queue
        .pending_count
        .checked_add(1)
        .ok_or(JobQueueError::ArithmeticOverflow)?;

    emit!(JobSubmitted {
        queue: queue.key(),
        job_id,
        submitter: job.submitter,
        data_hash,
        priority,
        bucket_index,
        timestamp: now,
    });

    Ok(())
}
