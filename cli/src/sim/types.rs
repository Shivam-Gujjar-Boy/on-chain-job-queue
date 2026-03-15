use solana_sdk::pubkey::Pubkey;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct QueueRuntime {
    pub name: String,
    pub pubkey: Pubkey,
}

#[derive(Debug, Clone)]
pub struct WorkerRuntime {
    pub worker_id: usize,
    pub queue_index: usize,
    pub authority: Pubkey,
}

#[derive(Debug, Clone, Default)]
pub struct QueueStats {
    pub submitted: u64,
    pub claimed: u64,
    pub completed: u64,
    pub failed: u64,
    pub timed_out: u64,
    pub onchain_pending: u64,
    pub onchain_active: u64,
    pub onchain_completed: u64,
    pub onchain_failed: u64,
}

#[derive(Debug, Clone, Default)]
pub struct WorkerStats {
    pub claimed: u64,
    pub completed: u64,
    pub failed: u64,
    pub heartbeats: u64,
    pub stalls: u64,
    pub tx_errors: u64,
}

#[derive(Debug, Clone)]
pub struct SimulationSnapshot {
    pub started_at: Instant,
    pub duration_seconds: u64,
    pub scenario: String,
    pub queues: Vec<QueueRuntime>,
    pub workers: Vec<WorkerRuntime>,
    pub queue_stats: Vec<QueueStats>,
    pub worker_stats: Vec<WorkerStats>,
    pub total_tx_success: u64,
    pub total_tx_fail: u64,
    pub total_job_submissions: u64,
    pub total_job_completions: u64,
    pub total_job_failures: u64,
    pub total_job_timeouts: u64,
    pub done: bool,
    pub recent_logs: VecDeque<String>,
}

impl SimulationSnapshot {
    pub fn new(duration_seconds: u64, scenario: String) -> Self {
        Self {
            started_at: Instant::now(),
            duration_seconds,
            scenario,
            queues: Vec::new(),
            workers: Vec::new(),
            queue_stats: Vec::new(),
            worker_stats: Vec::new(),
            total_tx_success: 0,
            total_tx_fail: 0,
            total_job_submissions: 0,
            total_job_completions: 0,
            total_job_failures: 0,
            total_job_timeouts: 0,
            done: false,
            recent_logs: VecDeque::with_capacity(200),
        }
    }

    pub fn elapsed_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    pub fn push_log(&mut self, message: impl Into<String>) {
        if self.recent_logs.len() >= 120 {
            self.recent_logs.pop_front();
        }
        self.recent_logs.push_back(message.into());
    }
}

pub type SharedState = Arc<Mutex<SimulationSnapshot>>;
