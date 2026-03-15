use anyhow::{anyhow, Result};
use std::io::{self, Write};

#[derive(Debug, Clone)]
pub struct SimulationConfig {
    pub rpc_url: String,
    pub keypair_path: String,
    pub num_queues: usize,
    pub num_buckets: u8,
    pub max_retries: u8,
    pub job_timeout_seconds: i64,
    pub num_clients: usize,
    pub num_workers: usize,
    pub duration_seconds: u64,
    pub client_submit_rate_per_sec: f64,
    pub worker_success_probability: f64,
    pub worker_fail_probability: f64,
    pub processing_time_min_ms: u64,
    pub processing_time_max_ms: u64,
    pub heartbeat_interval_ms: u64,
    pub max_priority: u8,
    pub poll_interval_ms: u64,
    pub rng_seed: u64,
}

impl SimulationConfig {
    pub fn prompt(rpc_url: String, keypair_path: String) -> Result<Self> {
        println!("\n=== Job Queue Localnet Simulation Setup ===");
        println!("Press Enter to accept defaults.\n");

        let num_queues = prompt_parse("Number of queues", 2usize)?;
        let num_buckets = prompt_parse("Buckets per queue (1-16)", 4u8)?;
        let max_retries = prompt_parse("Max retries per job", 3u8)?;
        let job_timeout_seconds = prompt_parse("Job timeout seconds", 10i64)?;
        let num_clients = prompt_parse("Client agents", 8usize)?;
        let num_workers = prompt_parse("Worker agents", 6usize)?;
        let duration_seconds = prompt_parse("Simulation duration (seconds)", 90u64)?;
        let client_submit_rate_per_sec =
            prompt_parse("Client submit rate per second", 0.8f64)?;
        let worker_success_probability = prompt_parse("Worker success probability (0..1)", 0.80f64)?;
        let worker_fail_probability = prompt_parse("Worker fail probability (0..1)", 0.15f64)?;
        let processing_time_min_ms = prompt_parse("Worker min processing ms", 500u64)?;
        let processing_time_max_ms = prompt_parse("Worker max processing ms", 3500u64)?;
        let heartbeat_interval_ms = prompt_parse("Heartbeat interval ms", 1000u64)?;
        let max_priority = prompt_parse("Max job priority (0..255)", 20u8)?;
        let poll_interval_ms = prompt_parse("Polling interval ms", 800u64)?;
        let rng_seed = prompt_parse("Random seed", 42u64)?;

        let config = Self {
            rpc_url,
            keypair_path,
            num_queues,
            num_buckets,
            max_retries,
            job_timeout_seconds,
            num_clients,
            num_workers,
            duration_seconds,
            client_submit_rate_per_sec,
            worker_success_probability,
            worker_fail_probability,
            processing_time_min_ms,
            processing_time_max_ms,
            heartbeat_interval_ms,
            max_priority,
            poll_interval_ms,
            rng_seed,
        };

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.num_queues == 0 {
            return Err(anyhow!("num_queues must be > 0"));
        }
        if self.num_workers == 0 {
            return Err(anyhow!("num_workers must be > 0"));
        }
        if self.num_clients == 0 {
            return Err(anyhow!("num_clients must be > 0"));
        }
        if !(1..=16).contains(&self.num_buckets) {
            return Err(anyhow!("num_buckets must be in [1,16]"));
        }
        if self.processing_time_min_ms == 0 || self.processing_time_max_ms == 0 {
            return Err(anyhow!("processing times must be > 0"));
        }
        if self.processing_time_min_ms > self.processing_time_max_ms {
            return Err(anyhow!("processing_time_min_ms must be <= processing_time_max_ms"));
        }
        if !(0.0..=1.0).contains(&self.worker_success_probability) {
            return Err(anyhow!("worker_success_probability must be in [0,1]"));
        }
        if !(0.0..=1.0).contains(&self.worker_fail_probability) {
            return Err(anyhow!("worker_fail_probability must be in [0,1]"));
        }
        if self.worker_success_probability + self.worker_fail_probability > 1.0 {
            return Err(anyhow!(
                "success_probability + fail_probability must be <= 1.0"
            ));
        }
        if self.client_submit_rate_per_sec <= 0.0 {
            return Err(anyhow!("client_submit_rate_per_sec must be > 0"));
        }
        Ok(())
    }
}

fn prompt_parse<T>(label: &str, default: T) -> Result<T>
where
    T: std::str::FromStr + Copy + std::fmt::Display,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    print!("{} [{}]: ", label, default);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Ok(default);
    }

    trimmed
        .parse::<T>()
        .map_err(|e| anyhow!("Invalid value for '{}': {}", label, e))
}
