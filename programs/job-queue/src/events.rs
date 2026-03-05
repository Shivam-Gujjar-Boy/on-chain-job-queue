use anchor_lang::prelude::*;

#[event]
pub struct QueueCreated {
    pub queue: Pubkey,
    pub authority: Pubkey,
    pub name: String,
    pub num_buckets: u8,
    pub max_retries: u8,
    pub job_timeout_seconds: i64,
    pub timestamp: i64,
}

#[event]
pub struct QueueUpdated {
    pub queue: Pubkey,
    pub authority: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct BucketInitialized {
    pub queue: Pubkey,
    pub bucket: Pubkey,
    pub bucket_index: u8,
    pub timestamp: i64,
}

#[event]
pub struct WorkerRegistered {
    pub queue: Pubkey,
    pub worker: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct WorkerDeregistered {
    pub queue: Pubkey,
    pub worker: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct JobSubmitted {
    pub queue: Pubkey,
    pub job_id: u64,
    pub submitter: Pubkey,
    pub data_hash: [u8; 32],
    pub priority: u8,
    pub bucket_index: u8,
    pub timestamp: i64,
}

#[event]
pub struct JobClaimed {
    pub queue: Pubkey,
    pub job_id: u64,
    pub worker: Pubkey,
    pub bucket_index: u8,
    pub timestamp: i64,
}

#[event]
pub struct HeartbeatReceived {
    pub queue: Pubkey,
    pub job_id: u64,
    pub worker: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct JobCompleted {
    pub queue: Pubkey,
    pub job_id: u64,
    pub worker: Pubkey,
    pub result_hash: [u8; 32],
    pub timestamp: i64,
}

#[event]
pub struct JobRetried {
    pub queue: Pubkey,
    pub job_id: u64,
    pub retry_count: u8,
    pub max_retries: u8,
    pub error_code: u32,
    pub timestamp: i64,
}

#[event]
pub struct JobFailed {
    pub queue: Pubkey,
    pub job_id: u64,
    pub worker: Pubkey,
    pub error_code: u32,
    pub retry_count: u8,
    pub timestamp: i64,
}

#[event]
pub struct JobTimedOut {
    pub queue: Pubkey,
    pub job_id: u64,
    pub retry_count: u8,
    pub retrying: bool,
    pub timestamp: i64,
}
