use anchor_lang::prelude::*;

#[error_code]
pub enum JobQueueError {
    #[msg("Queue name must be between 1 and 32 characters")]
    NameTooLong,

    #[msg("Queue name cannot be empty")]
    NameEmpty,

    #[msg("Number of buckets must be between 1 and 16")]
    InvalidBucketCount,

    #[msg("Job timeout must be greater than 0 seconds")]
    InvalidTimeout,

    #[msg("Max retries cannot exceed 10")]
    InvalidMaxRetries,

    #[msg("Bucket index is out of range for this queue")]
    InvalidBucketIndex,

    #[msg("Buckets must be initialized sequentially")]
    BucketInitOutOfOrder,

    #[msg("Queue is paused — no submissions or claims allowed")]
    QueuePaused,

    #[msg("Queue is not fully initialized (not all buckets created)")]
    QueueNotReady,

    #[msg("Only queue authority can submit jobs to this queue")]
    UnauthorizedSubmitter,

    #[msg("Invalid bucket account provided for this job")]
    InvalidBucket,

    #[msg("Job is not in pending status")]
    JobNotPending,

    #[msg("Job is not in assigned status")]
    JobNotAssigned,

    #[msg("Worker is not authorized for this operation")]
    UnauthorizedWorker,

    #[msg("Worker is not active")]
    WorkerNotActive,

    #[msg("Job has not timed out yet")]
    JobNotTimedOut,

    #[msg("Unauthorized — only queue authority can perform this action")]
    Unauthorized,

    #[msg("Arithmetic overflow in counter update")]
    ArithmeticOverflow,
}
