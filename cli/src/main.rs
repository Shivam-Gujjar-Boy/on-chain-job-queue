use anyhow::{anyhow, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair},
    signer::Signer,
    system_program,
    transaction::Transaction,
};
use std::str::FromStr;
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Program ID (must match the deployed program)
// ---------------------------------------------------------------------------
const PROGRAM_ID: &str = "5MSKMK96xy7rVkXYAgBPZmZfwRkvehe8P94qvVHhCSP1";

fn program_id() -> Pubkey {
    Pubkey::from_str(PROGRAM_ID).unwrap()
}

// ---------------------------------------------------------------------------
// Anchor discriminator helper
// ---------------------------------------------------------------------------
fn anchor_discriminator(method_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(format!("global:{}", method_name));
    let hash = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash[..8]);
    disc
}

// ---------------------------------------------------------------------------
// PDA derivation helpers
// ---------------------------------------------------------------------------
fn find_queue_pda(authority: &Pubkey, name: &str) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"queue", authority.as_ref(), name.as_bytes()],
        &program_id(),
    )
}

fn find_bucket_pda(queue: &Pubkey, bucket_index: u8) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"bucket", queue.as_ref(), &[bucket_index]],
        &program_id(),
    )
}

fn find_job_pda(queue: &Pubkey, job_id: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"job", queue.as_ref(), &job_id.to_le_bytes()],
        &program_id(),
    )
}

fn find_worker_pda(queue: &Pubkey, worker_authority: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"worker", queue.as_ref(), worker_authority.as_ref()],
        &program_id(),
    )
}

// ---------------------------------------------------------------------------
// On-chain account structures (borsh-compatible, must match program layout)
// ---------------------------------------------------------------------------
#[derive(BorshDeserialize, Debug)]
pub struct QueueAccount {
    pub authority: Pubkey,
    pub name: String,
    pub max_retries: u8,
    pub job_timeout_seconds: i64,
    pub num_buckets: u8,
    pub buckets_initialized: u8,
    pub total_jobs_created: u64,
    pub pending_count: u64,
    pub active_count: u64,
    pub completed_count: u64,
    pub failed_count: u64,
    pub is_paused: bool,
    pub require_authority_submit: bool,
    pub created_at: i64,
    pub bump: u8,
}

#[derive(BorshDeserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Assigned,
    Completed,
    Failed,
    TimedOut,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Pending => write!(f, "Pending"),
            JobStatus::Assigned => write!(f, "Assigned"),
            JobStatus::Completed => write!(f, "Completed"),
            JobStatus::Failed => write!(f, "Failed"),
            JobStatus::TimedOut => write!(f, "TimedOut"),
        }
    }
}

#[derive(BorshDeserialize, Debug)]
pub struct JobAccount {
    pub queue: Pubkey,
    pub job_id: u64,
    pub bucket_index: u8,
    pub submitter: Pubkey,
    pub data_hash: [u8; 32],
    pub priority: u8,
    pub status: JobStatus,
    pub assigned_worker: Pubkey,
    pub retry_count: u8,
    pub max_retries: u8,
    pub created_at: i64,
    pub assigned_at: i64,
    pub completed_at: i64,
    pub last_heartbeat: i64,
    pub result_hash: [u8; 32],
    pub error_code: u32,
    pub bump: u8,
}

#[derive(BorshDeserialize, Debug)]
pub struct WorkerAccount {
    pub queue: Pubkey,
    pub authority: Pubkey,
    pub is_active: bool,
    pub jobs_completed: u64,
    pub jobs_failed: u64,
    pub registered_at: i64,
    pub last_active_at: i64,
    pub bump: u8,
}

#[derive(BorshDeserialize, Debug)]
pub struct BucketAccount {
    pub queue: Pubkey,
    pub bucket_index: u8,
    pub pending_count: u32,
    pub bump: u8,
}

// ---------------------------------------------------------------------------
// Instruction argument serialization
// ---------------------------------------------------------------------------
#[derive(BorshSerialize)]
struct CreateQueueArgs {
    name: String,
    max_retries: u8,
    job_timeout_seconds: i64,
    num_buckets: u8,
    require_authority_submit: bool,
}

#[derive(BorshSerialize)]
struct InitBucketArgs {
    bucket_index: u8,
}

#[derive(BorshSerialize)]
struct UpdateQueueArgs {
    max_retries: Option<u8>,
    job_timeout_seconds: Option<i64>,
    is_paused: Option<bool>,
    require_authority_submit: Option<bool>,
}

#[derive(BorshSerialize)]
struct SubmitJobArgs {
    data_hash: [u8; 32],
    priority: u8,
}

#[derive(BorshSerialize)]
struct CompleteJobArgs {
    result_hash: [u8; 32],
}

#[derive(BorshSerialize)]
struct FailJobArgs {
    error_code: u32,
}

// ---------------------------------------------------------------------------
// Helper to build an Anchor instruction
// ---------------------------------------------------------------------------
fn build_ix(
    method: &str,
    accounts: Vec<AccountMeta>,
    args: &impl BorshSerialize,
) -> Result<Instruction> {
    let disc = anchor_discriminator(method);
    let mut data = disc.to_vec();
    borsh::to_writer(&mut data, args)?;
    Ok(Instruction {
        program_id: program_id(),
        accounts,
        data,
    })
}

fn build_ix_no_args(method: &str, accounts: Vec<AccountMeta>) -> Result<Instruction> {
    let disc = anchor_discriminator(method);
    Ok(Instruction {
        program_id: program_id(),
        accounts,
        data: disc.to_vec(),
    })
}

// ---------------------------------------------------------------------------
// Deserialization from raw account data (skip 8-byte Anchor discriminator)
// ---------------------------------------------------------------------------
fn deserialize_account<T: BorshDeserialize>(data: &[u8]) -> Result<T> {
    if data.len() < 8 {
        return Err(anyhow!("Account data too short"));
    }
    let mut reader = &data[8..];
    T::deserialize_reader(&mut reader).map_err(|e| anyhow!("Deserialization error: {}", e))
}

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------
#[derive(Parser)]
#[command(name = "jq-cli")]
#[command(about = "On-chain Job Queue — CLI Client")]
struct Cli {
    /// Solana RPC URL
    #[arg(long, default_value = "https://api.devnet.solana.com")]
    rpc_url: String,

    /// Path to the payer/authority keypair file
    #[arg(long, default_value = "~/.config/solana/id.json")]
    keypair: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new job queue
    CreateQueue {
        /// Queue name (max 32 chars)
        #[arg(long)]
        name: String,
        /// Maximum retry attempts per job
        #[arg(long, default_value = "3")]
        max_retries: u8,
        /// Seconds before a job times out without heartbeat
        #[arg(long, default_value = "60")]
        timeout: i64,
        /// Number of sharded buckets (1-16)
        #[arg(long, default_value = "4")]
        buckets: u8,
        /// Restrict job submission to authority only
        #[arg(long, default_value = "false")]
        authority_only: bool,
    },

    /// Initialize all buckets for a queue
    InitBuckets {
        /// Queue account address
        #[arg(long)]
        queue: String,
    },

    /// Register a worker for a queue
    RegisterWorker {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Path to the worker's keypair file
        #[arg(long)]
        worker_keypair: String,
    },

    /// Deregister a worker from a queue
    DeregisterWorker {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Worker authority public key
        #[arg(long)]
        worker: String,
    },

    /// Submit a new job to the queue
    SubmitJob {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Hex-encoded SHA-256 hash of job data
        #[arg(long)]
        data_hash: String,
        /// Job priority (0-255)
        #[arg(long, default_value = "0")]
        priority: u8,
    },

    /// Claim a pending job as a worker
    ClaimJob {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Job ID to claim
        #[arg(long)]
        job_id: u64,
        /// Path to the worker keypair file
        #[arg(long)]
        worker_keypair: String,
    },

    /// Send a heartbeat for an assigned job
    Heartbeat {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Job ID
        #[arg(long)]
        job_id: u64,
        /// Path to the worker keypair file
        #[arg(long)]
        worker_keypair: String,
    },

    /// Mark a job as completed
    CompleteJob {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Job ID
        #[arg(long)]
        job_id: u64,
        /// Hex-encoded SHA-256 hash of result data
        #[arg(long)]
        result_hash: String,
        /// Path to the worker keypair file
        #[arg(long)]
        worker_keypair: String,
    },

    /// Report a job as failed
    FailJob {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Job ID
        #[arg(long)]
        job_id: u64,
        /// Application-defined error code
        #[arg(long, default_value = "1")]
        error_code: u32,
        /// Path to the worker keypair file
        #[arg(long)]
        worker_keypair: String,
    },

    /// Time out a stale job (permissionless crank)
    TimeoutJob {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Job ID of the stale job
        #[arg(long)]
        job_id: u64,
    },

    /// Show queue status and statistics
    QueueStatus {
        /// Queue account address
        #[arg(long)]
        queue: String,
    },

    /// Show details of a specific job
    JobStatus {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Job ID
        #[arg(long)]
        job_id: u64,
    },

    /// Show worker statistics
    WorkerStatus {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Worker authority public key
        #[arg(long)]
        worker: String,
    },

    /// Run a continuous worker process that polls for jobs
    RunWorker {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Path to the worker keypair file
        #[arg(long)]
        worker_keypair: String,
        /// Poll interval in seconds
        #[arg(long, default_value = "5")]
        poll_interval: u64,
    },

    /// Pause or unpause a queue
    PauseQueue {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Whether to pause (true) or unpause (false)
        #[arg(long)]
        paused: bool,
    },

    /// List all pending jobs in a queue
    ListJobs {
        /// Queue account address
        #[arg(long)]
        queue: String,
        /// Filter by status: pending, assigned, completed, failed, timedout
        #[arg(long, default_value = "pending")]
        status: String,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
fn load_keypair(path: &str) -> Result<Keypair> {
    let expanded = shellexpand::tilde(path);
    let path_str: String = expanded.to_string();
    read_keypair_file(&path_str)
        .map_err(|e| anyhow!("Failed to read keypair from {}: {}", path, e))
}

fn parse_pubkey(s: &str) -> Result<Pubkey> {
    Pubkey::from_str(s).map_err(|e| anyhow!("Invalid pubkey '{}': {}", s, e))
}

fn parse_hex_hash(s: &str) -> Result<[u8; 32]> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 64 {
        return Err(anyhow!(
            "Hash must be 64 hex characters (32 bytes), got {}",
            s.len()
        ));
    }
    let bytes = hex_decode(s)?;
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(hash)
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|e| anyhow!("Invalid hex character: {}", e))
        })
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn send_and_confirm(
    client: &RpcClient,
    payer: &Keypair,
    instructions: Vec<Instruction>,
    signers: &[&Keypair],
) -> Result<String> {
    let recent_blockhash = client.get_latest_blockhash()?;
    let mut all_signers: Vec<&Keypair> = vec![payer];
    for s in signers {
        if s.pubkey() != payer.pubkey() {
            all_signers.push(s);
        }
    }

    let tx = Transaction::new_signed_with_payer(
        &instructions,
        Some(&payer.pubkey()),
        &all_signers.iter().map(|k| *k as &dyn Signer).collect::<Vec<_>>(),
        recent_blockhash,
    );

    let sig = client.send_and_confirm_transaction(&tx)?;
    Ok(sig.to_string())
}

fn fetch_queue(client: &RpcClient, queue_pubkey: &Pubkey) -> Result<QueueAccount> {
    let account = client.get_account(queue_pubkey)?;
    deserialize_account::<QueueAccount>(&account.data)
}

fn fetch_job(client: &RpcClient, job_pubkey: &Pubkey) -> Result<JobAccount> {
    let account = client.get_account(job_pubkey)?;
    deserialize_account::<JobAccount>(&account.data)
}

fn status_byte(status: &str) -> Result<u8> {
    match status.to_lowercase().as_str() {
        "pending" => Ok(0),
        "assigned" => Ok(1),
        "completed" => Ok(2),
        "failed" => Ok(3),
        "timedout" => Ok(4),
        _ => Err(anyhow!("Unknown status: {}", status)),
    }
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------
fn cmd_create_queue(
    client: &RpcClient,
    payer: &Keypair,
    name: &str,
    max_retries: u8,
    timeout: i64,
    buckets: u8,
    authority_only: bool,
) -> Result<()> {
    let (queue_pda, _) = find_queue_pda(&payer.pubkey(), name);
    println!("Creating queue '{}' at {}", name, queue_pda);

    let args = CreateQueueArgs {
        name: name.to_string(),
        max_retries,
        job_timeout_seconds: timeout,
        num_buckets: buckets,
        require_authority_submit: authority_only,
    };

    let ix = build_ix(
        "create_queue",
        vec![
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new(queue_pda, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        &args,
    )?;

    let sig = send_and_confirm(client, payer, vec![ix], &[])?;
    println!("✓ Queue created. Signature: {}", sig);
    println!("  Queue address: {}", queue_pda);
    println!("  Explorer: https://explorer.solana.com/tx/{}?cluster=devnet", sig);

    Ok(())
}

fn cmd_init_buckets(client: &RpcClient, payer: &Keypair, queue_str: &str) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let queue = fetch_queue(client, &queue_pubkey)?;

    println!(
        "Initializing {} buckets ({} already done)...",
        queue.num_buckets, queue.buckets_initialized
    );

    for i in queue.buckets_initialized..queue.num_buckets {
        let (bucket_pda, _) = find_bucket_pda(&queue_pubkey, i);
        let args = InitBucketArgs { bucket_index: i };

        let ix = build_ix(
            "init_bucket",
            vec![
                AccountMeta::new(payer.pubkey(), true),
                AccountMeta::new(queue_pubkey, false),
                AccountMeta::new(bucket_pda, false),
                AccountMeta::new_readonly(system_program::id(), false),
            ],
            &args,
        )?;

        let sig = send_and_confirm(client, payer, vec![ix], &[])?;
        println!("  ✓ Bucket {} initialized. Sig: {}", i, sig);
    }

    println!("✓ All buckets initialized.");
    Ok(())
}

fn cmd_register_worker(
    client: &RpcClient,
    payer: &Keypair,
    queue_str: &str,
    worker_keypair_path: &str,
) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let worker_kp = load_keypair(worker_keypair_path)?;
    let (worker_pda, _) = find_worker_pda(&queue_pubkey, &worker_kp.pubkey());

    println!(
        "Registering worker {} for queue {}",
        worker_kp.pubkey(),
        queue_pubkey
    );

    let ix = build_ix_no_args(
        "register_worker",
        vec![
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new_readonly(queue_pubkey, false),
            AccountMeta::new_readonly(worker_kp.pubkey(), false),
            AccountMeta::new(worker_pda, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    )?;

    let sig = send_and_confirm(client, payer, vec![ix], &[])?;
    println!("✓ Worker registered. Signature: {}", sig);
    println!("  Worker PDA: {}", worker_pda);
    println!("  Explorer: https://explorer.solana.com/tx/{}?cluster=devnet", sig);

    Ok(())
}

fn cmd_deregister_worker(
    client: &RpcClient,
    payer: &Keypair,
    queue_str: &str,
    worker_str: &str,
) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let worker_authority = parse_pubkey(worker_str)?;
    let (worker_pda, _) = find_worker_pda(&queue_pubkey, &worker_authority);

    let ix = build_ix_no_args(
        "deregister_worker",
        vec![
            AccountMeta::new_readonly(payer.pubkey(), true),
            AccountMeta::new_readonly(queue_pubkey, false),
            AccountMeta::new(worker_pda, false),
        ],
    )?;

    let sig = send_and_confirm(client, payer, vec![ix], &[])?;
    println!("✓ Worker deregistered. Signature: {}", sig);

    Ok(())
}

fn cmd_submit_job(
    client: &RpcClient,
    payer: &Keypair,
    queue_str: &str,
    data_hash_str: &str,
    priority: u8,
) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let data_hash = parse_hex_hash(data_hash_str)?;

    let queue = fetch_queue(client, &queue_pubkey)?;
    let job_id = queue.total_jobs_created;
    let bucket_index = (job_id % queue.num_buckets as u64) as u8;

    let (job_pda, _) = find_job_pda(&queue_pubkey, job_id);
    let (bucket_pda, _) = find_bucket_pda(&queue_pubkey, bucket_index);

    println!("Submitting job #{} to bucket {}...", job_id, bucket_index);

    let args = SubmitJobArgs {
        data_hash,
        priority,
    };

    let ix = build_ix(
        "submit_job",
        vec![
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new(queue_pubkey, false),
            AccountMeta::new(job_pda, false),
            AccountMeta::new(bucket_pda, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        &args,
    )?;

    let sig = send_and_confirm(client, payer, vec![ix], &[])?;
    println!("✓ Job #{} submitted. Signature: {}", job_id, sig);
    println!("  Job PDA: {}", job_pda);
    println!("  Explorer: https://explorer.solana.com/tx/{}?cluster=devnet", sig);

    Ok(())
}

fn cmd_claim_job(
    client: &RpcClient,
    payer: &Keypair,
    queue_str: &str,
    job_id: u64,
    worker_keypair_path: &str,
) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let worker_kp = load_keypair(worker_keypair_path)?;

    let (job_pda, _) = find_job_pda(&queue_pubkey, job_id);
    let job = fetch_job(client, &job_pda)?;

    let (bucket_pda, _) = find_bucket_pda(&queue_pubkey, job.bucket_index);
    let (worker_pda, _) = find_worker_pda(&queue_pubkey, &worker_kp.pubkey());

    let ix = build_ix_no_args(
        "claim_job",
        vec![
            AccountMeta::new_readonly(worker_kp.pubkey(), true),
            AccountMeta::new(queue_pubkey, false),
            AccountMeta::new(job_pda, false),
            AccountMeta::new(bucket_pda, false),
            AccountMeta::new(worker_pda, false),
        ],
    )?;

    let sig = send_and_confirm(client, payer, vec![ix], &[&worker_kp])?;
    println!("✓ Job #{} claimed by worker {}. Sig: {}", job_id, worker_kp.pubkey(), sig);
    println!("  Explorer: https://explorer.solana.com/tx/{}?cluster=devnet", sig);

    Ok(())
}

fn cmd_heartbeat(
    client: &RpcClient,
    payer: &Keypair,
    queue_str: &str,
    job_id: u64,
    worker_keypair_path: &str,
) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let worker_kp = load_keypair(worker_keypair_path)?;

    let (job_pda, _) = find_job_pda(&queue_pubkey, job_id);
    let (worker_pda, _) = find_worker_pda(&queue_pubkey, &worker_kp.pubkey());

    let ix = build_ix_no_args(
        "heartbeat",
        vec![
            AccountMeta::new_readonly(worker_kp.pubkey(), true),
            AccountMeta::new_readonly(queue_pubkey, false),
            AccountMeta::new(job_pda, false),
            AccountMeta::new(worker_pda, false),
        ],
    )?;

    let sig = send_and_confirm(client, payer, vec![ix], &[&worker_kp])?;
    println!("✓ Heartbeat sent for job #{}. Sig: {}", job_id, sig);

    Ok(())
}

fn cmd_complete_job(
    client: &RpcClient,
    payer: &Keypair,
    queue_str: &str,
    job_id: u64,
    result_hash_str: &str,
    worker_keypair_path: &str,
) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let worker_kp = load_keypair(worker_keypair_path)?;
    let result_hash = parse_hex_hash(result_hash_str)?;

    let (job_pda, _) = find_job_pda(&queue_pubkey, job_id);
    let (worker_pda, _) = find_worker_pda(&queue_pubkey, &worker_kp.pubkey());

    let args = CompleteJobArgs { result_hash };

    let ix = build_ix(
        "complete_job",
        vec![
            AccountMeta::new_readonly(worker_kp.pubkey(), true),
            AccountMeta::new(queue_pubkey, false),
            AccountMeta::new(job_pda, false),
            AccountMeta::new(worker_pda, false),
        ],
        &args,
    )?;

    let sig = send_and_confirm(client, payer, vec![ix], &[&worker_kp])?;
    println!("✓ Job #{} completed. Sig: {}", job_id, sig);
    println!("  Explorer: https://explorer.solana.com/tx/{}?cluster=devnet", sig);

    Ok(())
}

fn cmd_fail_job(
    client: &RpcClient,
    payer: &Keypair,
    queue_str: &str,
    job_id: u64,
    error_code: u32,
    worker_keypair_path: &str,
) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let worker_kp = load_keypair(worker_keypair_path)?;

    let (job_pda, _) = find_job_pda(&queue_pubkey, job_id);
    let job = fetch_job(client, &job_pda)?;
    let (bucket_pda, _) = find_bucket_pda(&queue_pubkey, job.bucket_index);
    let (worker_pda, _) = find_worker_pda(&queue_pubkey, &worker_kp.pubkey());

    let args = FailJobArgs { error_code };

    let ix = build_ix(
        "fail_job",
        vec![
            AccountMeta::new_readonly(worker_kp.pubkey(), true),
            AccountMeta::new(queue_pubkey, false),
            AccountMeta::new(job_pda, false),
            AccountMeta::new(bucket_pda, false),
            AccountMeta::new(worker_pda, false),
        ],
        &args,
    )?;

    let sig = send_and_confirm(client, payer, vec![ix], &[&worker_kp])?;
    println!("✓ Job #{} failed (code={}). Sig: {}", job_id, error_code, sig);

    Ok(())
}

fn cmd_timeout_job(
    client: &RpcClient,
    payer: &Keypair,
    queue_str: &str,
    job_id: u64,
) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;

    let (job_pda, _) = find_job_pda(&queue_pubkey, job_id);
    let job = fetch_job(client, &job_pda)?;
    let (bucket_pda, _) = find_bucket_pda(&queue_pubkey, job.bucket_index);

    let ix = build_ix_no_args(
        "timeout_job",
        vec![
            AccountMeta::new_readonly(payer.pubkey(), true),
            AccountMeta::new(queue_pubkey, false),
            AccountMeta::new(job_pda, false),
            AccountMeta::new(bucket_pda, false),
        ],
    )?;

    let sig = send_and_confirm(client, payer, vec![ix], &[])?;
    println!("✓ Job #{} timed out. Sig: {}", job_id, sig);

    Ok(())
}

fn cmd_queue_status(client: &RpcClient, queue_str: &str) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let queue = fetch_queue(client, &queue_pubkey)?;

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                    QUEUE STATUS                             ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("  Address:              {}", queue_pubkey);
    println!("  Name:                 {}", queue.name);
    println!("  Authority:            {}", queue.authority);
    println!("  Status:               {}", if queue.is_paused { "PAUSED" } else { "ACTIVE" });
    println!("  Buckets:              {}/{}", queue.buckets_initialized, queue.num_buckets);
    println!("  Max Retries:          {}", queue.max_retries);
    println!("  Timeout:              {}s", queue.job_timeout_seconds);
    println!("  Authority-only Subm.: {}", queue.require_authority_submit);
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("  Total Jobs Created:   {}", queue.total_jobs_created);
    println!("  Pending:              {}", queue.pending_count);
    println!("  Active:               {}", queue.active_count);
    println!("  Completed:            {}", queue.completed_count);
    println!("  Failed:               {}", queue.failed_count);
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}

fn cmd_job_status(client: &RpcClient, queue_str: &str, job_id: u64) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let (job_pda, _) = find_job_pda(&queue_pubkey, job_id);
    let job = fetch_job(client, &job_pda)?;

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                     JOB STATUS                              ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("  Job ID:               {}", job.job_id);
    println!("  Job PDA:              {}", job_pda);
    println!("  Queue:                {}", job.queue);
    println!("  Bucket:               {}", job.bucket_index);
    println!("  Status:               {}", job.status);
    println!("  Priority:             {}", job.priority);
    println!("  Submitter:            {}", job.submitter);
    println!("  Data Hash:            {}", hex_encode(&job.data_hash));
    println!("  Assigned Worker:      {}", if job.assigned_worker == Pubkey::default() { "None".to_string() } else { job.assigned_worker.to_string() });
    println!("  Retries:              {}/{}", job.retry_count, job.max_retries);
    println!("  Created At:           {}", job.created_at);
    println!("  Assigned At:          {}", if job.assigned_at == 0 { "N/A".to_string() } else { job.assigned_at.to_string() });
    println!("  Completed At:         {}", if job.completed_at == 0 { "N/A".to_string() } else { job.completed_at.to_string() });
    println!("  Last Heartbeat:       {}", if job.last_heartbeat == 0 { "N/A".to_string() } else { job.last_heartbeat.to_string() });
    println!("  Result Hash:          {}", hex_encode(&job.result_hash));
    println!("  Error Code:           {}", job.error_code);
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}

fn cmd_worker_status(client: &RpcClient, queue_str: &str, worker_str: &str) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let worker_authority = parse_pubkey(worker_str)?;
    let (worker_pda, _) = find_worker_pda(&queue_pubkey, &worker_authority);

    let account = client.get_account(&worker_pda)?;
    let worker = deserialize_account::<WorkerAccount>(&account.data)?;

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                   WORKER STATUS                             ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("  Worker PDA:           {}", worker_pda);
    println!("  Authority:            {}", worker.authority);
    println!("  Queue:                {}", worker.queue);
    println!("  Active:               {}", worker.is_active);
    println!("  Jobs Completed:       {}", worker.jobs_completed);
    println!("  Jobs Failed:          {}", worker.jobs_failed);
    println!("  Registered At:        {}", worker.registered_at);
    println!("  Last Active At:       {}", worker.last_active_at);
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}

fn cmd_list_jobs(client: &RpcClient, queue_str: &str, status_filter: &str) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let status = status_byte(status_filter)?;

    // Job account data size: 8 (disc) + 210 (data) = 218 — use INIT_SPACE from program
    // The status byte is at offset: 8(disc) + 32(queue) + 8(job_id) + 1(bucket) + 32(submitter) + 32(data_hash) + 1(priority) = 114
    let filters = vec![
        RpcFilterType::Memcmp(Memcmp::new_base58_encoded(8, queue_pubkey.as_ref())),
        RpcFilterType::Memcmp(Memcmp::new_base58_encoded(114, &[status])),
    ];

    let config = RpcProgramAccountsConfig {
        filters: Some(filters),
        account_config: RpcAccountInfoConfig::default(),
        ..Default::default()
    };

    let accounts = client.get_program_accounts_with_config(&program_id(), config)?;

    println!("Found {} {} job(s):", accounts.len(), status_filter);
    for (pubkey, account) in &accounts {
        if let Ok(job) = deserialize_account::<JobAccount>(&account.data) {
            println!(
                "  Job #{:<4} | prio={:<3} | retries={}/{} | bucket={} | {}",
                job.job_id, job.priority, job.retry_count, job.max_retries, job.bucket_index, pubkey
            );
        }
    }

    Ok(())
}

fn cmd_pause_queue(
    client: &RpcClient,
    payer: &Keypair,
    queue_str: &str,
    paused: bool,
) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;

    let args = UpdateQueueArgs {
        max_retries: None,
        job_timeout_seconds: None,
        is_paused: Some(paused),
        require_authority_submit: None,
    };

    let ix = build_ix(
        "update_queue",
        vec![
            AccountMeta::new_readonly(payer.pubkey(), true),
            AccountMeta::new(queue_pubkey, false),
        ],
        &args,
    )?;

    let sig = send_and_confirm(client, payer, vec![ix], &[])?;
    println!(
        "✓ Queue {}. Sig: {}",
        if paused { "paused" } else { "unpaused" },
        sig
    );

    Ok(())
}

fn cmd_run_worker(
    client: &RpcClient,
    payer: &Keypair,
    queue_str: &str,
    worker_keypair_path: &str,
    poll_interval: u64,
) -> Result<()> {
    let queue_pubkey = parse_pubkey(queue_str)?;
    let worker_kp = load_keypair(worker_keypair_path)?;
    let (worker_pda, _) = find_worker_pda(&queue_pubkey, &worker_kp.pubkey());

    println!("═══════════════════════════════════════════════════");
    println!("  Worker Process Started");
    println!("  Queue:   {}", queue_pubkey);
    println!("  Worker:  {}", worker_kp.pubkey());
    println!("  Poll:    {}s", poll_interval);
    println!("═══════════════════════════════════════════════════");

    loop {
        // Find pending jobs using getProgramAccounts
        let filters = vec![
            RpcFilterType::Memcmp(Memcmp::new_base58_encoded(8, queue_pubkey.as_ref())),
            RpcFilterType::Memcmp(Memcmp::new_base58_encoded(114, &[0])), // Pending = 0
        ];

        let config = RpcProgramAccountsConfig {
            filters: Some(filters),
            account_config: RpcAccountInfoConfig::default(),
            ..Default::default()
        };

        match client.get_program_accounts_with_config(&program_id(), config) {
            Ok(accounts) if !accounts.is_empty() => {
                // Sort by priority (desc) then job_id (asc) for FIFO within same priority
                let mut jobs: Vec<(Pubkey, JobAccount)> = accounts
                    .into_iter()
                    .filter_map(|(pk, acc)| {
                        deserialize_account::<JobAccount>(&acc.data).ok().map(|j| (pk, j))
                    })
                    .collect();
                jobs.sort_by(|a, b| b.1.priority.cmp(&a.1.priority).then(a.1.job_id.cmp(&b.1.job_id)));

                for (job_pda, job) in jobs.iter().take(1) {
                    println!("[Worker] Claiming job #{} (prio={})...", job.job_id, job.priority);

                    let (bucket_pda, _) = find_bucket_pda(&queue_pubkey, job.bucket_index);

                    let claim_ix = build_ix_no_args(
                        "claim_job",
                        vec![
                            AccountMeta::new_readonly(worker_kp.pubkey(), true),
                            AccountMeta::new(queue_pubkey, false),
                            AccountMeta::new(*job_pda, false),
                            AccountMeta::new(bucket_pda, false),
                            AccountMeta::new(worker_pda, false),
                        ],
                    )?;

                    match send_and_confirm(client, payer, vec![claim_ix], &[&worker_kp]) {
                        Ok(sig) => {
                            println!("[Worker] ✓ Claimed job #{}. Sig: {}", job.job_id, sig);

                            // Simulate processing with heartbeats
                            println!("[Worker] Processing job #{}...", job.job_id);

                            // Send a heartbeat
                            let hb_ix = build_ix_no_args(
                                "heartbeat",
                                vec![
                                    AccountMeta::new_readonly(worker_kp.pubkey(), true),
                                    AccountMeta::new_readonly(queue_pubkey, false),
                                    AccountMeta::new(*job_pda, false),
                                    AccountMeta::new(worker_pda, false),
                                ],
                            )?;
                            let _ = send_and_confirm(client, payer, vec![hb_ix], &[&worker_kp]);
                            println!("[Worker] ♥ Heartbeat sent for job #{}", job.job_id);

                            // Simulate work by hashing the data_hash as "result"
                            let mut hasher = Sha256::new();
                            hasher.update(&job.data_hash);
                            hasher.update(b"processed");
                            let result: [u8; 32] = hasher.finalize().into();

                            let complete_args = CompleteJobArgs {
                                result_hash: result,
                            };
                            let complete_ix = build_ix(
                                "complete_job",
                                vec![
                                    AccountMeta::new_readonly(worker_kp.pubkey(), true),
                                    AccountMeta::new(queue_pubkey, false),
                                    AccountMeta::new(*job_pda, false),
                                    AccountMeta::new(worker_pda, false),
                                ],
                                &complete_args,
                            )?;

                            match send_and_confirm(
                                client,
                                payer,
                                vec![complete_ix],
                                &[&worker_kp],
                            ) {
                                Ok(sig) => {
                                    println!(
                                        "[Worker] ✓ Completed job #{}. Sig: {}",
                                        job.job_id, sig
                                    );
                                }
                                Err(e) => {
                                    println!(
                                        "[Worker] ✗ Failed to complete job #{}: {}",
                                        job.job_id, e
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            println!("[Worker] ✗ Failed to claim job #{}: {}", job.job_id, e);
                        }
                    }
                }
            }
            Ok(_) => {
                // No pending jobs
            }
            Err(e) => {
                println!("[Worker] Error querying jobs: {}", e);
            }
        }

        thread::sleep(Duration::from_secs(poll_interval));
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------
fn main() -> Result<()> {
    let cli = Cli::parse();

    let keypair_path = shellexpand::tilde(&cli.keypair).to_string();
    let payer = load_keypair(&keypair_path)?;
    let client = RpcClient::new_with_commitment(&cli.rpc_url, CommitmentConfig::confirmed());

    println!("RPC:    {}", cli.rpc_url);
    println!("Payer:  {}", payer.pubkey());
    println!();

    match cli.command {
        Commands::CreateQueue {
            name,
            max_retries,
            timeout,
            buckets,
            authority_only,
        } => cmd_create_queue(&client, &payer, &name, max_retries, timeout, buckets, authority_only),

        Commands::InitBuckets { queue } => cmd_init_buckets(&client, &payer, &queue),

        Commands::RegisterWorker {
            queue,
            worker_keypair,
        } => cmd_register_worker(&client, &payer, &queue, &worker_keypair),

        Commands::DeregisterWorker { queue, worker } => {
            cmd_deregister_worker(&client, &payer, &queue, &worker)
        }

        Commands::SubmitJob {
            queue,
            data_hash,
            priority,
        } => cmd_submit_job(&client, &payer, &queue, &data_hash, priority),

        Commands::ClaimJob {
            queue,
            job_id,
            worker_keypair,
        } => cmd_claim_job(&client, &payer, &queue, job_id, &worker_keypair),

        Commands::Heartbeat {
            queue,
            job_id,
            worker_keypair,
        } => cmd_heartbeat(&client, &payer, &queue, job_id, &worker_keypair),

        Commands::CompleteJob {
            queue,
            job_id,
            result_hash,
            worker_keypair,
        } => cmd_complete_job(&client, &payer, &queue, job_id, &result_hash, &worker_keypair),

        Commands::FailJob {
            queue,
            job_id,
            error_code,
            worker_keypair,
        } => cmd_fail_job(&client, &payer, &queue, job_id, error_code, &worker_keypair),

        Commands::TimeoutJob { queue, job_id } => cmd_timeout_job(&client, &payer, &queue, job_id),

        Commands::QueueStatus { queue } => cmd_queue_status(&client, &queue),

        Commands::JobStatus { queue, job_id } => cmd_job_status(&client, &queue, job_id),

        Commands::WorkerStatus { queue, worker } => cmd_worker_status(&client, &queue, &worker),

        Commands::RunWorker {
            queue,
            worker_keypair,
            poll_interval,
        } => cmd_run_worker(&client, &payer, &queue, &worker_keypair, poll_interval),

        Commands::PauseQueue { queue, paused } => {
            cmd_pause_queue(&client, &payer, &queue, paused)
        }

        Commands::ListJobs { queue, status } => cmd_list_jobs(&client, &queue, &status),
    }
}

// Tiny shellexpand-like tilde expansion
mod shellexpand {
    pub fn tilde(path: &str) -> String {
        if path.starts_with("~/") || path == "~" {
            if let Ok(home) = std::env::var("HOME") {
                return path.replacen('~', &home, 1);
            }
        }
        path.to_string()
    }
}
