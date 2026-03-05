use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace)]
pub struct Worker {
    /// The queue this worker is registered for
    pub queue: Pubkey,
    /// The worker's signing authority
    pub authority: Pubkey,
    /// Whether this worker is currently active and can claim jobs
    pub is_active: bool,
    /// Total number of jobs this worker has completed successfully
    pub jobs_completed: u64,
    /// Total number of jobs this worker has failed
    pub jobs_failed: u64,
    /// Unix timestamp when the worker was registered
    pub registered_at: i64,
    /// Unix timestamp of the worker's last activity
    pub last_active_at: i64,
    /// PDA bump seed
    pub bump: u8,
}
