use anchor_lang::prelude::*;

/// Represents the lifecycle status of a job
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum JobStatus {
    /// Job is waiting to be claimed by a worker
    Pending,
    /// Job has been claimed and is being processed
    Assigned,
    /// Job completed successfully
    Completed,
    /// Job failed permanently (retries exhausted)
    Failed,
    /// Job timed out permanently (retries exhausted)
    TimedOut,
}

impl Default for JobStatus {
    fn default() -> Self {
        JobStatus::Pending
    }
}

#[account]
#[derive(InitSpace)]
pub struct Job {
    /// The queue this job belongs to
    pub queue: Pubkey,
    /// Unique sequential job ID within the queue
    pub job_id: u64,
    /// The bucket index this job was assigned to (job_id % num_buckets)
    pub bucket_index: u8,
    /// The account that submitted this job
    pub submitter: Pubkey,
    /// SHA-256 hash of the off-chain job data (stored off-chain for efficiency)
    pub data_hash: [u8; 32],
    /// Job priority (0 = lowest, 255 = highest)
    pub priority: u8,
    /// Current job status
    pub status: JobStatus,
    /// The worker currently assigned to this job (Pubkey::default() if unassigned)
    pub assigned_worker: Pubkey,
    /// Number of times this job has been retried
    pub retry_count: u8,
    /// Maximum retries allowed (copied from queue config at submission time)
    pub max_retries: u8,
    /// Unix timestamp when the job was created
    pub created_at: i64,
    /// Unix timestamp when the job was last assigned (0 if never)
    pub assigned_at: i64,
    /// Unix timestamp when the job was completed/failed (0 if not yet)
    pub completed_at: i64,
    /// Unix timestamp of the last heartbeat from the assigned worker
    pub last_heartbeat: i64,
    /// SHA-256 hash of the job result data (set on completion)
    pub result_hash: [u8; 32],
    /// Error code if the job failed (application-defined)
    pub error_code: u32,
    /// PDA bump seed
    pub bump: u8,
}
