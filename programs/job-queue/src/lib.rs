use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("5MSKMK96xy7rVkXYAgBPZmZfwRkvehe8P94qvVHhCSP1");

#[program]
pub mod job_queue {
    use super::*;

    /// Create a new job queue with the given configuration.
    /// The authority becomes the owner who can manage workers and update settings.
    pub fn create_queue(
        ctx: Context<CreateQueue>,
        name: String,
        max_retries: u8,
        job_timeout_seconds: i64,
        num_buckets: u8,
        require_authority_submit: bool,
    ) -> Result<()> {
        instructions::create_queue::handler(
            ctx,
            name,
            max_retries,
            job_timeout_seconds,
            num_buckets,
            require_authority_submit,
        )
    }

    /// Initialize a sharded bucket for the queue. Must be called sequentially
    /// for each bucket_index from 0 to num_buckets-1 before jobs can be submitted.
    pub fn init_bucket(ctx: Context<InitBucket>, bucket_index: u8) -> Result<()> {
        instructions::init_bucket::handler(ctx, bucket_index)
    }

    /// Update queue configuration. Only the queue authority can call this.
    pub fn update_queue(
        ctx: Context<UpdateQueue>,
        max_retries: Option<u8>,
        job_timeout_seconds: Option<i64>,
        is_paused: Option<bool>,
        require_authority_submit: Option<bool>,
    ) -> Result<()> {
        instructions::update_queue::handler(
            ctx,
            max_retries,
            job_timeout_seconds,
            is_paused,
            require_authority_submit,
        )
    }

    /// Register a worker for the queue. Only the queue authority can register workers.
    /// The worker_authority is the public key that the worker will use to sign claims.
    pub fn register_worker(ctx: Context<RegisterWorker>) -> Result<()> {
        instructions::register_worker::handler(ctx)
    }

    /// Deactivate a registered worker. Only the queue authority can deregister workers.
    pub fn deregister_worker(ctx: Context<DeregisterWorker>) -> Result<()> {
        instructions::deregister_worker::handler(ctx)
    }

    /// Submit a new job to the queue. The job data is referenced by its SHA-256 hash
    /// (actual data stored off-chain). The job is assigned to a bucket based on its ID
    /// for sharded distribution.
    pub fn submit_job(
        ctx: Context<SubmitJob>,
        data_hash: [u8; 32],
        priority: u8,
    ) -> Result<()> {
        instructions::submit_job::handler(ctx, data_hash, priority)
    }

    /// A registered worker claims a pending job. The worker must sign the transaction.
    /// The job transitions from Pending to Assigned status atomically.
    pub fn claim_job(ctx: Context<ClaimJob>) -> Result<()> {
        instructions::claim_job::handler(ctx)
    }

    /// Worker sends a heartbeat to indicate it is still processing the job.
    /// Jobs without heartbeats within the timeout period become reclaimable.
    pub fn heartbeat(ctx: Context<Heartbeat>) -> Result<()> {
        instructions::heartbeat::handler(ctx)
    }

    /// Worker marks a job as completed with a result hash.
    /// The result data is stored off-chain, referenced by the hash.
    pub fn complete_job(ctx: Context<CompleteJob>, result_hash: [u8; 32]) -> Result<()> {
        instructions::complete_job::handler(ctx, result_hash)
    }

    /// Worker reports the job as failed with an error code.
    /// If retries remain, the job returns to Pending status; otherwise it's permanently Failed.
    pub fn fail_job(ctx: Context<FailJob>, error_code: u32) -> Result<()> {
        instructions::fail_job::handler(ctx, error_code)
    }

    /// Permissionless crank: anyone can call this to time out a stale job
    /// whose last heartbeat exceeds the queue's timeout threshold.
    pub fn timeout_job(ctx: Context<TimeoutJob>) -> Result<()> {
        instructions::timeout_job::handler(ctx)
    }
}
