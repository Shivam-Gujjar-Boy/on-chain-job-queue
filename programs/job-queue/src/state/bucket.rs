use anchor_lang::prelude::*;

/// Sharded bucket for contention-free job claiming.
/// Jobs are distributed across buckets using job_id % num_buckets.
/// Workers target specific buckets, allowing parallel processing without lock contention.
#[account]
#[derive(InitSpace)]
pub struct Bucket {
    /// The queue this bucket belongs to
    pub queue: Pubkey,
    /// The index of this bucket (0..num_buckets)
    pub bucket_index: u8,
    /// Number of pending jobs in this bucket
    pub pending_count: u32,
    /// PDA bump seed
    pub bump: u8,
}
