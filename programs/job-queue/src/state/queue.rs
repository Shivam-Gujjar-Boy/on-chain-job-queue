use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace)]
pub struct Queue {
    /// The authority who controls this queue (can manage workers, update config)
    pub authority: Pubkey,
    /// Human-readable queue name (max 32 bytes)
    #[max_len(32)]
    pub name: String,
    /// Maximum number of retry attempts for failed/timed-out jobs
    pub max_retries: u8,
    /// Seconds before a job is considered timed out (no heartbeat)
    pub job_timeout_seconds: i64,
    /// Number of sharded buckets for contention-free job claiming
    pub num_buckets: u8,
    /// Number of buckets that have been initialized so far
    pub buckets_initialized: u8,
    /// Total number of jobs ever created in this queue (also serves as next job ID)
    pub total_jobs_created: u64,
    /// Current count of jobs in pending status
    pub pending_count: u64,
    /// Current count of jobs in assigned/active status
    pub active_count: u64,
    /// Total count of jobs that completed successfully
    pub completed_count: u64,
    /// Total count of jobs that failed permanently
    pub failed_count: u64,
    /// Whether the queue is paused (no new submissions or claims)
    pub is_paused: bool,
    /// Whether only the authority can submit jobs
    pub require_authority_submit: bool,
    /// Unix timestamp when the queue was created
    pub created_at: i64,
    /// PDA bump seed
    pub bump: u8,
}
