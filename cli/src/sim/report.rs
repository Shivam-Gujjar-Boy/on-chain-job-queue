use crate::sim::config::SimulationConfig;
use crate::sim::types::SimulationSnapshot;
use anyhow::{Context, Result};
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn export_results(snapshot: &SimulationSnapshot, config: &SimulationConfig) -> Result<(PathBuf, PathBuf)> {
    let results_dir = resolve_results_dir()?;
    fs::create_dir_all(&results_dir).context("failed to create results directory")?;

    let ts = unix_timestamp();
    let run_id = format!("sim-run-{}", ts);
    let txt_path = results_dir.join(format!("{}.txt", run_id));
    let json_path = results_dir.join(format!("{}.json", run_id));

    let text_report = build_text_report(snapshot, config, ts);
    fs::write(&txt_path, text_report).context("failed to write text simulation report")?;

    let json_report = build_json_report(snapshot, config, ts);
    fs::write(&json_path, serde_json::to_string_pretty(&json_report)?).context("failed to write json simulation report")?;

    Ok((txt_path, json_path))
}

fn resolve_results_dir() -> Result<PathBuf> {
    let cwd = env::current_dir().context("failed to read current dir")?;
    let base = if cwd
        .file_name()
        .map(|name| name == "cli")
        .unwrap_or(false)
    {
        cwd.parent().map(Path::to_path_buf).unwrap_or(cwd)
    } else {
        cwd
    };

    Ok(base.join("results"))
}

fn build_text_report(snapshot: &SimulationSnapshot, config: &SimulationConfig, ts: u64) -> String {
    let mut out = String::new();

    out.push_str("On-Chain Job Queue Simulation Report\n");
    out.push_str("===================================\n");
    out.push_str(&format!("Run ID: sim-run-{}\n", ts));
    out.push_str(&format!("Scenario: {}\n", snapshot.scenario));
    out.push_str(&format!("Elapsed: {}s / {}s\n", snapshot.elapsed_seconds(), snapshot.duration_seconds));
    out.push_str("\n");

    out.push_str("Configuration\n");
    out.push_str("-------------\n");
    out.push_str(&format!("rpc_url: {}\n", config.rpc_url));
    out.push_str(&format!("num_queues: {}\n", config.num_queues));
    out.push_str(&format!("num_clients: {}\n", config.num_clients));
    out.push_str(&format!("num_workers: {}\n", config.num_workers));
    out.push_str(&format!("num_buckets: {}\n", config.num_buckets));
    out.push_str(&format!("max_retries: {}\n", config.max_retries));
    out.push_str(&format!("job_timeout_seconds: {}\n", config.job_timeout_seconds));
    out.push_str(&format!("client_submit_rate_per_sec: {:.3}\n", config.client_submit_rate_per_sec));
    out.push_str(&format!("worker_success_probability: {:.3}\n", config.worker_success_probability));
    out.push_str(&format!("worker_fail_probability: {:.3}\n", config.worker_fail_probability));
    out.push_str("\n");

    out.push_str("Totals\n");
    out.push_str("------\n");
    out.push_str(&format!("tx_success: {}\n", snapshot.total_tx_success));
    out.push_str(&format!("tx_fail: {}\n", snapshot.total_tx_fail));
    out.push_str(&format!("job_submissions: {}\n", snapshot.total_job_submissions));
    out.push_str(&format!("job_completions: {}\n", snapshot.total_job_completions));
    out.push_str(&format!("job_failures: {}\n", snapshot.total_job_failures));
    out.push_str(&format!("job_timeouts: {}\n", snapshot.total_job_timeouts));
    out.push_str("\n");

    out.push_str("Per-Queue\n");
    out.push_str("---------\n");
    for (idx, queue) in snapshot.queues.iter().enumerate() {
        let stats = snapshot.queue_stats.get(idx).cloned().unwrap_or_default();
        out.push_str(&format!(
            "[{}] {} ({})\n  submitted={} claimed={} completed={} failed={} timed_out={} onchain_pending={} onchain_active={} onchain_completed={} onchain_failed={}\n",
            idx,
            queue.name,
            queue.pubkey,
            stats.submitted,
            stats.claimed,
            stats.completed,
            stats.failed,
            stats.timed_out,
            stats.onchain_pending,
            stats.onchain_active,
            stats.onchain_completed,
            stats.onchain_failed
        ));
    }
    out.push_str("\n");

    out.push_str("Per-Worker\n");
    out.push_str("----------\n");
    for (idx, worker) in snapshot.workers.iter().enumerate() {
        let stats = snapshot.worker_stats.get(idx).cloned().unwrap_or_default();
        out.push_str(&format!(
            "worker#{} queue={} authority={} claimed={} completed={} failed={} heartbeats={} stalls={} tx_errors={}\n",
            worker.worker_id,
            worker.queue_index,
            worker.authority,
            stats.claimed,
            stats.completed,
            stats.failed,
            stats.heartbeats,
            stats.stalls,
            stats.tx_errors
        ));
    }
    out.push_str("\n");

    out.push_str("Recent Logs\n");
    out.push_str("-----------\n");
    for line in &snapshot.recent_logs {
        out.push_str(&format!("- {}\n", line));
    }

    out
}

fn build_json_report(snapshot: &SimulationSnapshot, config: &SimulationConfig, ts: u64) -> serde_json::Value {
    let queues = snapshot
        .queues
        .iter()
        .enumerate()
        .map(|(idx, queue)| {
            let stats = snapshot.queue_stats.get(idx).cloned().unwrap_or_default();
            json!({
                "index": idx,
                "name": queue.name,
                "pubkey": queue.pubkey.to_string(),
                "stats": {
                    "submitted": stats.submitted,
                    "claimed": stats.claimed,
                    "completed": stats.completed,
                    "failed": stats.failed,
                    "timed_out": stats.timed_out,
                    "onchain_pending": stats.onchain_pending,
                    "onchain_active": stats.onchain_active,
                    "onchain_completed": stats.onchain_completed,
                    "onchain_failed": stats.onchain_failed
                }
            })
        })
        .collect::<Vec<_>>();

    let workers = snapshot
        .workers
        .iter()
        .enumerate()
        .map(|(idx, worker)| {
            let stats = snapshot.worker_stats.get(idx).cloned().unwrap_or_default();
            json!({
                "worker_id": worker.worker_id,
                "queue_index": worker.queue_index,
                "authority": worker.authority.to_string(),
                "stats": {
                    "claimed": stats.claimed,
                    "completed": stats.completed,
                    "failed": stats.failed,
                    "heartbeats": stats.heartbeats,
                    "stalls": stats.stalls,
                    "tx_errors": stats.tx_errors
                }
            })
        })
        .collect::<Vec<_>>();

    json!({
        "run_id": format!("sim-run-{}", ts),
        "timestamp_unix": ts,
        "scenario": snapshot.scenario,
        "duration_seconds": snapshot.duration_seconds,
        "elapsed_seconds": snapshot.elapsed_seconds(),
        "totals": {
            "tx_success": snapshot.total_tx_success,
            "tx_fail": snapshot.total_tx_fail,
            "job_submissions": snapshot.total_job_submissions,
            "job_completions": snapshot.total_job_completions,
            "job_failures": snapshot.total_job_failures,
            "job_timeouts": snapshot.total_job_timeouts
        },
        "config": {
            "rpc_url": config.rpc_url,
            "num_queues": config.num_queues,
            "num_buckets": config.num_buckets,
            "max_retries": config.max_retries,
            "job_timeout_seconds": config.job_timeout_seconds,
            "num_clients": config.num_clients,
            "num_workers": config.num_workers,
            "duration_seconds": config.duration_seconds,
            "client_submit_rate_per_sec": config.client_submit_rate_per_sec,
            "worker_success_probability": config.worker_success_probability,
            "worker_fail_probability": config.worker_fail_probability,
            "processing_time_min_ms": config.processing_time_min_ms,
            "processing_time_max_ms": config.processing_time_max_ms,
            "heartbeat_interval_ms": config.heartbeat_interval_ms,
            "max_priority": config.max_priority,
            "poll_interval_ms": config.poll_interval_ms,
            "rng_seed": config.rng_seed
        },
        "queues": queues,
        "workers": workers,
        "recent_logs": snapshot.recent_logs.iter().cloned().collect::<Vec<_>>()
    })
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}
