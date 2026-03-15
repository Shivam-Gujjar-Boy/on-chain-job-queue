use crate::sim::chain::{ChainClient, JobStatus};
use crate::sim::config::SimulationConfig;
use crate::sim::types::{QueueRuntime, QueueStats, SharedState, SimulationSnapshot, WorkerRuntime, WorkerStats};
use anyhow::Result;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sha2::{Digest, Sha256};
use solana_sdk::signature::{Keypair, Signer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;

#[derive(Clone)]
struct WorkerCtx {
    worker_id: usize,
    queue_index: usize,
    keypair: Arc<Keypair>,
}

pub async fn run_simulation(
    config: SimulationConfig,
    chain: Arc<ChainClient>,
    state: SharedState,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    initialize_chain_objects(&config, chain.clone(), state.clone()).await?;

    let deadline = Instant::now() + Duration::from_secs(config.duration_seconds);
    let worker_contexts = load_worker_contexts(&state);
    let queue_pubkeys = load_queue_pubkeys(&state);

    let mut handles: Vec<JoinHandle<()>> = Vec::new();

    for client_id in 0..config.num_clients {
        handles.push(tokio::spawn(client_agent_loop(
            client_id,
            config.clone(),
            chain.clone(),
            state.clone(),
            queue_pubkeys.clone(),
            stop.clone(),
            deadline,
        )));
    }

    for worker in worker_contexts {
        handles.push(tokio::spawn(worker_agent_loop(
            worker,
            config.clone(),
            chain.clone(),
            state.clone(),
            queue_pubkeys.clone(),
            stop.clone(),
            deadline,
        )));
    }

    handles.push(tokio::spawn(cranker_loop(
        config.clone(),
        chain.clone(),
        state.clone(),
        queue_pubkeys.clone(),
        stop.clone(),
        deadline,
    )));

    handles.push(tokio::spawn(queue_poller_loop(
        chain,
        state.clone(),
        queue_pubkeys,
        stop.clone(),
        deadline,
    )));

    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    stop.store(true, Ordering::Relaxed);
    for handle in handles {
        let _ = handle.await;
    }

    if let Ok(mut guard) = state.lock() {
        guard.done = true;
        guard.push_log("Simulation finished");
    }

    Ok(())
}

async fn initialize_chain_objects(
    config: &SimulationConfig,
    chain: Arc<ChainClient>,
    state: SharedState,
) -> Result<()> {
    let timestamp = unix_timestamp();
    let mut queues = Vec::with_capacity(config.num_queues);

    for queue_idx in 0..config.num_queues {
        let queue_name = format!("sim-{}-q{}", timestamp, queue_idx);
        let queue_name_for_task = queue_name.clone();
        let max_retries = config.max_retries;
        let timeout_seconds = config.job_timeout_seconds;
        let num_buckets = config.num_buckets;
        let chain_clone = chain.clone();
        let queue_pubkey = tokio::task::spawn_blocking(move || -> Result<_> {
            let queue = chain_clone.create_queue(
                &queue_name_for_task,
                max_retries,
                timeout_seconds,
                num_buckets,
            )?;
            chain_clone.init_all_buckets(&queue)?;
            Ok(queue)
        })
        .await??;

        queues.push(QueueRuntime {
            name: queue_name,
            pubkey: queue_pubkey,
        });
    }

    let mut workers = Vec::with_capacity(config.num_workers);
    let mut worker_keys: Vec<Arc<Keypair>> = Vec::with_capacity(config.num_workers);

    for worker_id in 0..config.num_workers {
        let queue_index = worker_id % config.num_queues;
        let queue = queues[queue_index].pubkey;
        let keypair = Arc::new(Keypair::new());
        let keypair_pub = keypair.pubkey();
        let chain_clone = chain.clone();

        tokio::task::spawn_blocking(move || chain_clone.register_worker(&queue, &keypair_pub)).await??;

        workers.push(WorkerRuntime {
            worker_id,
            queue_index,
            authority: keypair_pub,
        });
        worker_keys.push(keypair);
    }

    if let Ok(mut guard) = state.lock() {
        guard.queues = queues;
        guard.workers = workers;
        guard.queue_stats = vec![QueueStats::default(); config.num_queues];
        guard.worker_stats = vec![WorkerStats::default(); config.num_workers];
        guard.push_log("Queues and workers initialized on localnet");
    }

    store_worker_keypairs(state, worker_keys);
    Ok(())
}

fn store_worker_keypairs(state: SharedState, workers: Vec<Arc<Keypair>>) {
    WORKER_KEYS
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .expect("worker key lock poisoned")
        .clone_from(&workers);

    if let Ok(mut guard) = state.lock() {
        guard.push_log("Worker keypairs generated in-memory");
    }
}

fn load_worker_contexts(state: &SharedState) -> Vec<WorkerCtx> {
    let keys = WORKER_KEYS
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .expect("worker key lock poisoned")
        .clone();

    let mut contexts = Vec::new();
    if let Ok(guard) = state.lock() {
        for (idx, runtime) in guard.workers.iter().enumerate() {
            if let Some(keypair) = keys.get(idx) {
                contexts.push(WorkerCtx {
                    worker_id: runtime.worker_id,
                    queue_index: runtime.queue_index,
                    keypair: keypair.clone(),
                });
            }
        }
    }
    contexts
}

fn load_queue_pubkeys(state: &SharedState) -> Vec<solana_sdk::pubkey::Pubkey> {
    if let Ok(guard) = state.lock() {
        guard.queues.iter().map(|q| q.pubkey).collect()
    } else {
        Vec::new()
    }
}

async fn client_agent_loop(
    client_id: usize,
    config: SimulationConfig,
    chain: Arc<ChainClient>,
    state: SharedState,
    queue_pubkeys: Vec<solana_sdk::pubkey::Pubkey>,
    stop: Arc<AtomicBool>,
    deadline: Instant,
) {
    let mut rng = StdRng::seed_from_u64(config.rng_seed ^ ((client_id as u64 + 17) * 1117));

    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        let wait_secs = sample_exponential_wait(&mut rng, config.client_submit_rate_per_sec);
        tokio::time::sleep(Duration::from_secs_f64(wait_secs)).await;

        if stop.load(Ordering::Relaxed) {
            break;
        }

        let queue_index = rng.gen_range(0..queue_pubkeys.len());
        let queue = queue_pubkeys[queue_index];
        let mut data_hash = [0u8; 32];
        rng.fill(&mut data_hash);
        let priority = rng.gen_range(0..=config.max_priority);

        let chain_clone = chain.clone();
        let submit_result = tokio::task::spawn_blocking(move || chain_clone.submit_job(&queue, data_hash, priority)).await;

        match submit_result {
            Ok(Ok(job_id)) => {
                if let Ok(mut guard) = state.lock() {
                    guard.total_tx_success += 1;
                    guard.total_job_submissions += 1;
                    if let Some(stats) = guard.queue_stats.get_mut(queue_index) {
                        stats.submitted += 1;
                    }
                    guard.push_log(format!(
                        "client#{client_id} submitted job#{job_id} to queue[{queue_index}] prio={priority}"
                    ));
                }
            }
            Ok(Err(err)) => {
                if let Ok(mut guard) = state.lock() {
                    guard.total_tx_fail += 1;
                    guard.push_log(format!("client#{client_id} submit failed: {err}"));
                }
            }
            Err(join_err) => {
                if let Ok(mut guard) = state.lock() {
                    guard.total_tx_fail += 1;
                    guard.push_log(format!("client#{client_id} task join error: {join_err}"));
                }
            }
        }
    }
}

async fn worker_agent_loop(
    worker: WorkerCtx,
    config: SimulationConfig,
    chain: Arc<ChainClient>,
    state: SharedState,
    queue_pubkeys: Vec<solana_sdk::pubkey::Pubkey>,
    stop: Arc<AtomicBool>,
    deadline: Instant,
) {
    let mut rng = StdRng::seed_from_u64(config.rng_seed ^ ((worker.worker_id as u64 + 97) * 3571));
    let queue = queue_pubkeys[worker.queue_index];
    let poll = Duration::from_millis(config.poll_interval_ms);

    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(poll).await;

        let pending_jobs = {
            let chain_clone = chain.clone();
            let queue_copy = queue;
            tokio::task::spawn_blocking(move || chain_clone.list_jobs_by_status(&queue_copy, JobStatus::Pending)).await
        };

        let jobs = match pending_jobs {
            Ok(Ok(jobs)) if !jobs.is_empty() => jobs,
            Ok(Ok(_)) => continue,
            Ok(Err(err)) => {
                register_worker_error(&state, worker.worker_id, format!("list pending failed: {err}"));
                continue;
            }
            Err(err) => {
                register_worker_error(&state, worker.worker_id, format!("list pending join failed: {err}"));
                continue;
            }
        };

        let mut sorted_jobs = jobs;
        sorted_jobs.sort_by(|a, b| b.1.priority.cmp(&a.1.priority).then(a.1.job_id.cmp(&b.1.job_id)));
        let (job_pda, job) = match sorted_jobs.into_iter().next() {
            Some(v) => v,
            None => continue,
        };

        let claim_result = {
            let chain_clone = chain.clone();
            let worker_key = worker.keypair.clone();
            tokio::task::spawn_blocking(move || {
                chain_clone.claim_job(&queue, &job_pda, job.bucket_index, &worker_key)
            })
            .await
        };

        match claim_result {
            Ok(Ok(())) => {
                if let Ok(mut guard) = state.lock() {
                    guard.total_tx_success += 1;
                    if let Some(stats) = guard.worker_stats.get_mut(worker.worker_id) {
                        stats.claimed += 1;
                    }
                    if let Some(stats) = guard.queue_stats.get_mut(worker.queue_index) {
                        stats.claimed += 1;
                    }
                    guard.push_log(format!(
                        "worker#{} claimed job#{} on queue[{}]",
                        worker.worker_id, job.job_id, worker.queue_index
                    ));
                }
            }
            Ok(Err(err)) => {
                register_worker_error(&state, worker.worker_id, format!("claim failed: {err}"));
                continue;
            }
            Err(err) => {
                register_worker_error(&state, worker.worker_id, format!("claim join failed: {err}"));
                continue;
            }
        }

        let process_ms = rng.gen_range(config.processing_time_min_ms..=config.processing_time_max_ms);
        let outcome_roll: f64 = rng.gen_range(0.0..1.0);
        let do_success = outcome_roll <= config.worker_success_probability;
        let do_fail = !do_success && (outcome_roll <= config.worker_success_probability + config.worker_fail_probability);
        let do_stall = !do_success && !do_fail;

        if do_stall {
            tokio::time::sleep(Duration::from_millis(
                process_ms + ((config.job_timeout_seconds.max(1) as u64) * 1000),
            ))
            .await;
            if let Ok(mut guard) = state.lock() {
                if let Some(stats) = guard.worker_stats.get_mut(worker.worker_id) {
                    stats.stalls += 1;
                }
                guard.push_log(format!(
                    "worker#{} stalled job#{} (waiting for timeout)",
                    worker.worker_id, job.job_id
                ));
            }
            continue;
        }

        let heartbeat_interval = Duration::from_millis(config.heartbeat_interval_ms.max(100));
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(process_ms) && !stop.load(Ordering::Relaxed) {
            tokio::time::sleep(heartbeat_interval).await;
            let chain_clone = chain.clone();
            let worker_key = worker.keypair.clone();
            let hb_res = tokio::task::spawn_blocking(move || chain_clone.heartbeat(&queue, &job_pda, &worker_key)).await;
            match hb_res {
                Ok(Ok(())) => {
                    if let Ok(mut guard) = state.lock() {
                        guard.total_tx_success += 1;
                        if let Some(stats) = guard.worker_stats.get_mut(worker.worker_id) {
                            stats.heartbeats += 1;
                        }
                    }
                }
                Ok(Err(err)) => {
                    register_worker_error(&state, worker.worker_id, format!("heartbeat failed: {err}"));
                    break;
                }
                Err(err) => {
                    register_worker_error(&state, worker.worker_id, format!("heartbeat join failed: {err}"));
                    break;
                }
            }
        }

        if do_success {
            let mut hasher = Sha256::new();
            hasher.update(job.data_hash);
            hasher.update((worker.worker_id as u64).to_le_bytes());
            hasher.update(job.job_id.to_le_bytes());
            let result_hash: [u8; 32] = hasher.finalize().into();
            let chain_clone = chain.clone();
            let worker_key = worker.keypair.clone();
            let result = tokio::task::spawn_blocking(move || {
                chain_clone.complete_job(&queue, &job_pda, &worker_key, result_hash)
            })
            .await;

            match result {
                Ok(Ok(())) => {
                    if let Ok(mut guard) = state.lock() {
                        guard.total_tx_success += 1;
                        guard.total_job_completions += 1;
                        if let Some(stats) = guard.worker_stats.get_mut(worker.worker_id) {
                            stats.completed += 1;
                        }
                        if let Some(stats) = guard.queue_stats.get_mut(worker.queue_index) {
                            stats.completed += 1;
                        }
                        guard.push_log(format!(
                            "worker#{} completed job#{}",
                            worker.worker_id, job.job_id
                        ));
                    }
                }
                Ok(Err(err)) => {
                    register_worker_error(&state, worker.worker_id, format!("complete failed: {err}"));
                }
                Err(err) => {
                    register_worker_error(&state, worker.worker_id, format!("complete join failed: {err}"));
                }
            }
        } else if do_fail {
            let error_code = 100 + rng.gen_range(0..900) as u32;
            let chain_clone = chain.clone();
            let worker_key = worker.keypair.clone();
            let result = tokio::task::spawn_blocking(move || {
                chain_clone.fail_job(&queue, &job_pda, job.bucket_index, &worker_key, error_code)
            })
            .await;

            match result {
                Ok(Ok(())) => {
                    if let Ok(mut guard) = state.lock() {
                        guard.total_tx_success += 1;
                        guard.total_job_failures += 1;
                        if let Some(stats) = guard.worker_stats.get_mut(worker.worker_id) {
                            stats.failed += 1;
                        }
                        if let Some(stats) = guard.queue_stats.get_mut(worker.queue_index) {
                            stats.failed += 1;
                        }
                        guard.push_log(format!(
                            "worker#{} failed job#{} code={}",
                            worker.worker_id, job.job_id, error_code
                        ));
                    }
                }
                Ok(Err(err)) => {
                    register_worker_error(&state, worker.worker_id, format!("fail failed: {err}"));
                }
                Err(err) => {
                    register_worker_error(&state, worker.worker_id, format!("fail join failed: {err}"));
                }
            }
        }
    }
}

async fn cranker_loop(
    config: SimulationConfig,
    chain: Arc<ChainClient>,
    state: SharedState,
    queue_pubkeys: Vec<solana_sdk::pubkey::Pubkey>,
    stop: Arc<AtomicBool>,
    deadline: Instant,
) {
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_secs(1)).await;

        for (queue_index, queue) in queue_pubkeys.iter().enumerate() {
            let queue_key = *queue;
            let chain_clone = chain.clone();
            let assigned = tokio::task::spawn_blocking({
                let queue_copy = queue_key;
                move || chain_clone.list_jobs_by_status(&queue_copy, JobStatus::Assigned)
            })
            .await;

            let assigned_jobs = match assigned {
                Ok(Ok(jobs)) => jobs,
                _ => continue,
            };

            let now = unix_timestamp() as i64;
            for (job_pda, job) in assigned_jobs {
                let heartbeat_base = job.last_heartbeat.max(job.assigned_at);
                if now - heartbeat_base <= config.job_timeout_seconds {
                    continue;
                }

                let chain_clone = chain.clone();
                let timeout_queue = queue_key;
                let timeout_result = tokio::task::spawn_blocking(move || {
                    chain_clone.timeout_job(&timeout_queue, &job_pda, job.bucket_index)
                })
                .await;

                if let Ok(Ok(())) = timeout_result {
                    if let Ok(mut guard) = state.lock() {
                        guard.total_tx_success += 1;
                        guard.total_job_timeouts += 1;
                        if let Some(stats) = guard.queue_stats.get_mut(queue_index) {
                            stats.timed_out += 1;
                        }
                        guard.push_log(format!("cranker timed out job#{} queue[{queue_index}]", job.job_id));
                    }
                }
            }
        }
    }
}

async fn queue_poller_loop(
    chain: Arc<ChainClient>,
    state: SharedState,
    queue_pubkeys: Vec<solana_sdk::pubkey::Pubkey>,
    stop: Arc<AtomicBool>,
    deadline: Instant,
) {
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(900)).await;

        for (queue_index, queue_pubkey) in queue_pubkeys.iter().enumerate() {
            let chain_clone = chain.clone();
            let queue = tokio::task::spawn_blocking({
                let queue_copy = *queue_pubkey;
                move || chain_clone.fetch_queue(&queue_copy)
            })
            .await;

            if let Ok(Ok(queue_data)) = queue {
                if let Ok(mut guard) = state.lock() {
                    if let Some(stats) = guard.queue_stats.get_mut(queue_index) {
                        stats.onchain_pending = queue_data.pending_count;
                        stats.onchain_active = queue_data.active_count;
                        stats.onchain_completed = queue_data.completed_count;
                        stats.onchain_failed = queue_data.failed_count;
                    }
                }
            }
        }
    }
}

fn register_worker_error(state: &SharedState, worker_id: usize, error: String) {
    if let Ok(mut guard) = state.lock() {
        guard.total_tx_fail += 1;
        if let Some(stats) = guard.worker_stats.get_mut(worker_id) {
            stats.tx_errors += 1;
        }
        guard.push_log(format!("worker#{worker_id} {error}"));
    }
}

fn sample_exponential_wait(rng: &mut StdRng, rate_per_sec: f64) -> f64 {
    let safe_rate = rate_per_sec.max(0.001);
    let u = rng.gen_range(f64::EPSILON..1.0);
    (-u.ln() / safe_rate).max(0.02)
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}

static WORKER_KEYS: std::sync::OnceLock<std::sync::Mutex<Vec<Arc<Keypair>>>> = std::sync::OnceLock::new();

pub fn build_initial_snapshot(config: &SimulationConfig) -> SimulationSnapshot {
    let scenario = format!(
        "queues={} clients={} workers={} submit_rate={:.2}/s success={:.2} fail={:.2}",
        config.num_queues,
        config.num_clients,
        config.num_workers,
        config.client_submit_rate_per_sec,
        config.worker_success_probability,
        config.worker_fail_probability
    );
    SimulationSnapshot::new(config.duration_seconds, scenario)
}

