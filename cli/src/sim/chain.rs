use anyhow::{anyhow, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_account_decoder_client_types::UiAccountEncoding;
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
use std::sync::Arc;

const PROGRAM_ID: &str = "5MSKMK96xy7rVkXYAgBPZmZfwRkvehe8P94qvVHhCSP1";
const JOB_STATUS_OFFSET: usize = 114;

#[derive(BorshDeserialize, Debug, Clone)]
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

#[derive(BorshDeserialize, Debug, Clone)]
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

#[derive(Clone)]
pub struct ChainClient {
    rpc: Arc<RpcClient>,
    payer: Arc<Keypair>,
    program_id: Pubkey,
    rpc_url: String,
}

impl ChainClient {
    pub fn new(rpc_url: &str, keypair_path: &str) -> Result<Self> {
        let keypair = load_keypair(keypair_path)?;
        Ok(Self {
            rpc: Arc::new(RpcClient::new_with_commitment(
                rpc_url,
                CommitmentConfig::confirmed(),
            )),
            payer: Arc::new(keypair),
            program_id: Pubkey::from_str(PROGRAM_ID)?,
            rpc_url: rpc_url.to_string(),
        })
    }

    pub fn payer_pubkey(&self) -> Pubkey {
        self.payer.pubkey()
    }

    pub fn preflight_checks(&self) -> Result<()> {
        let program_account = self
            .rpc
            .get_account(&self.program_id)
            .map_err(|e| anyhow!(
                "Program {} is not available on RPC {}: {}. Deploy your program to localnet first (anchor deploy --provider.cluster localnet).",
                self.program_id,
                self.rpc_url,
                e
            ))?;

        if !program_account.executable {
            return Err(anyhow!(
                "Account {} exists but is not executable on {}. Ensure the correct program is deployed.",
                self.program_id,
                self.rpc_url
            ));
        }

        let balance = self.rpc.get_balance(&self.payer.pubkey())?;
        let min_lamports = 50_000_000;
        if balance < min_lamports {
            return Err(anyhow!(
                "Payer {} has low balance ({} lamports). Airdrop on localnet before simulation: solana airdrop 50 {} --url {}",
                self.payer.pubkey(),
                balance,
                self.payer.pubkey(),
                self.rpc_url
            ));
        }

        Ok(())
    }

    pub fn create_queue(
        &self,
        name: &str,
        max_retries: u8,
        timeout_secs: i64,
        num_buckets: u8,
    ) -> Result<Pubkey> {
        let (queue_pda, _) = find_queue_pda(&self.program_id, &self.payer.pubkey(), name);
        let args = CreateQueueArgs {
            name: name.to_owned(),
            max_retries,
            job_timeout_seconds: timeout_secs,
            num_buckets,
            require_authority_submit: false,
        };
        let ix = build_ix(
            &self.program_id,
            "create_queue",
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(queue_pda, false),
                AccountMeta::new_readonly(system_program::id(), false),
            ],
            &args,
        )?;
        self.send_and_confirm(vec![ix], &[])?;
        Ok(queue_pda)
    }

    pub fn init_all_buckets(&self, queue: &Pubkey) -> Result<()> {
        let queue_data = self.fetch_queue(queue)?;
        for i in queue_data.buckets_initialized..queue_data.num_buckets {
            let (bucket_pda, _) = find_bucket_pda(&self.program_id, queue, i);
            let args = InitBucketArgs { bucket_index: i };
            let ix = build_ix(
                &self.program_id,
                "init_bucket",
                vec![
                    AccountMeta::new(self.payer.pubkey(), true),
                    AccountMeta::new(*queue, false),
                    AccountMeta::new(bucket_pda, false),
                    AccountMeta::new_readonly(system_program::id(), false),
                ],
                &args,
            )?;
            self.send_and_confirm(vec![ix], &[])?;
        }
        Ok(())
    }

    pub fn register_worker(&self, queue: &Pubkey, worker: &Pubkey) -> Result<()> {
        let (worker_pda, _) = find_worker_pda(&self.program_id, queue, worker);
        let ix = build_ix_no_args(
            &self.program_id,
            "register_worker",
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(*queue, false),
                AccountMeta::new_readonly(*worker, false),
                AccountMeta::new(worker_pda, false),
                AccountMeta::new_readonly(system_program::id(), false),
            ],
        )?;
        self.send_and_confirm(vec![ix], &[])?;
        Ok(())
    }

    pub fn submit_job(&self, queue: &Pubkey, data_hash: [u8; 32], priority: u8) -> Result<u64> {
        let queue_data = self.fetch_queue(queue)?;
        let job_id = queue_data.total_jobs_created;
        let bucket_index = (job_id % queue_data.num_buckets as u64) as u8;
        let (job_pda, _) = find_job_pda(&self.program_id, queue, job_id);
        let (bucket_pda, _) = find_bucket_pda(&self.program_id, queue, bucket_index);

        let args = SubmitJobArgs {
            data_hash,
            priority,
        };
        let ix = build_ix(
            &self.program_id,
            "submit_job",
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(*queue, false),
                AccountMeta::new(job_pda, false),
                AccountMeta::new(bucket_pda, false),
                AccountMeta::new_readonly(system_program::id(), false),
            ],
            &args,
        )?;

        self.send_and_confirm(vec![ix], &[])?;
        Ok(job_id)
    }

    pub fn list_jobs_by_status(&self, queue: &Pubkey, status: JobStatus) -> Result<Vec<(Pubkey, JobAccount)>> {
        let filters = vec![
            RpcFilterType::Memcmp(Memcmp::new_base58_encoded(8, queue.as_ref())),
            RpcFilterType::Memcmp(Memcmp::new_base58_encoded(
                JOB_STATUS_OFFSET,
                &[status as u8],
            )),
        ];
        let config = RpcProgramAccountsConfig {
            filters: Some(filters),
            account_config: RpcAccountInfoConfig {
                encoding: Some(UiAccountEncoding::Base64),
                ..Default::default()
            },
            ..Default::default()
        };

        let accounts = self
            .rpc
            .get_program_accounts_with_config(&self.program_id, config)?;

        let mut jobs = Vec::new();
        for (pk, acc) in accounts {
            if let Ok(job) = deserialize_account::<JobAccount>(&acc.data) {
                jobs.push((pk, job));
            }
        }
        Ok(jobs)
    }

    pub fn claim_job(
        &self,
        queue: &Pubkey,
        job_pda: &Pubkey,
        bucket_index: u8,
        worker: &Keypair,
    ) -> Result<()> {
        let (bucket_pda, _) = find_bucket_pda(&self.program_id, queue, bucket_index);
        let (worker_pda, _) = find_worker_pda(&self.program_id, queue, &worker.pubkey());
        let ix = build_ix_no_args(
            &self.program_id,
            "claim_job",
            vec![
                AccountMeta::new_readonly(worker.pubkey(), true),
                AccountMeta::new(*queue, false),
                AccountMeta::new(*job_pda, false),
                AccountMeta::new(bucket_pda, false),
                AccountMeta::new(worker_pda, false),
            ],
        )?;
        self.send_and_confirm(vec![ix], &[worker])?;
        Ok(())
    }

    pub fn heartbeat(&self, queue: &Pubkey, job_pda: &Pubkey, worker: &Keypair) -> Result<()> {
        let (worker_pda, _) = find_worker_pda(&self.program_id, queue, &worker.pubkey());
        let ix = build_ix_no_args(
            &self.program_id,
            "heartbeat",
            vec![
                AccountMeta::new_readonly(worker.pubkey(), true),
                AccountMeta::new_readonly(*queue, false),
                AccountMeta::new(*job_pda, false),
                AccountMeta::new(worker_pda, false),
            ],
        )?;
        self.send_and_confirm(vec![ix], &[worker])?;
        Ok(())
    }

    pub fn complete_job(
        &self,
        queue: &Pubkey,
        job_pda: &Pubkey,
        worker: &Keypair,
        result_hash: [u8; 32],
    ) -> Result<()> {
        let (worker_pda, _) = find_worker_pda(&self.program_id, queue, &worker.pubkey());
        let args = CompleteJobArgs { result_hash };
        let ix = build_ix(
            &self.program_id,
            "complete_job",
            vec![
                AccountMeta::new_readonly(worker.pubkey(), true),
                AccountMeta::new(*queue, false),
                AccountMeta::new(*job_pda, false),
                AccountMeta::new(worker_pda, false),
            ],
            &args,
        )?;
        self.send_and_confirm(vec![ix], &[worker])?;
        Ok(())
    }

    pub fn fail_job(
        &self,
        queue: &Pubkey,
        job_pda: &Pubkey,
        bucket_index: u8,
        worker: &Keypair,
        error_code: u32,
    ) -> Result<()> {
        let (bucket_pda, _) = find_bucket_pda(&self.program_id, queue, bucket_index);
        let (worker_pda, _) = find_worker_pda(&self.program_id, queue, &worker.pubkey());

        let args = FailJobArgs { error_code };
        let ix = build_ix(
            &self.program_id,
            "fail_job",
            vec![
                AccountMeta::new_readonly(worker.pubkey(), true),
                AccountMeta::new(*queue, false),
                AccountMeta::new(*job_pda, false),
                AccountMeta::new(bucket_pda, false),
                AccountMeta::new(worker_pda, false),
            ],
            &args,
        )?;
        self.send_and_confirm(vec![ix], &[worker])?;
        Ok(())
    }

    pub fn timeout_job(&self, queue: &Pubkey, job_pda: &Pubkey, bucket_index: u8) -> Result<()> {
        let (bucket_pda, _) = find_bucket_pda(&self.program_id, queue, bucket_index);
        let ix = build_ix_no_args(
            &self.program_id,
            "timeout_job",
            vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(*queue, false),
                AccountMeta::new(*job_pda, false),
                AccountMeta::new(bucket_pda, false),
            ],
        )?;
        self.send_and_confirm(vec![ix], &[])?;
        Ok(())
    }

    pub fn fetch_queue(&self, queue: &Pubkey) -> Result<QueueAccount> {
        let account = self.rpc.get_account(queue)?;
        deserialize_account::<QueueAccount>(&account.data)
    }

    fn send_and_confirm(&self, instructions: Vec<Instruction>, signers: &[&Keypair]) -> Result<String> {
        let recent_blockhash = self.rpc.get_latest_blockhash()?;

        let mut all_signers: Vec<&dyn Signer> = vec![self.payer.as_ref() as &dyn Signer];
        for signer in signers {
            if signer.pubkey() != self.payer.pubkey() {
                all_signers.push(*signer as &dyn Signer);
            }
        }

        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&self.payer.pubkey()),
            &all_signers,
            recent_blockhash,
        );

        let signature = self.rpc.send_and_confirm_transaction(&tx)?;
        Ok(signature.to_string())
    }
}

pub fn load_keypair(path: &str) -> Result<Keypair> {
    let expanded = shellexpand::tilde(path);
    let path_str: String = expanded.to_string();
    read_keypair_file(&path_str)
        .map_err(|e| anyhow!("Failed to read keypair from {}: {}", path, e))
}

fn anchor_discriminator(method_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(format!("global:{}", method_name));
    let hash = hasher.finalize();
    let mut discriminator = [0u8; 8];
    discriminator.copy_from_slice(&hash[..8]);
    discriminator
}

fn build_ix(
    program_id: &Pubkey,
    method: &str,
    accounts: Vec<AccountMeta>,
    args: &impl BorshSerialize,
) -> Result<Instruction> {
    let discriminator = anchor_discriminator(method);
    let mut data = discriminator.to_vec();
    borsh::to_writer(&mut data, args)?;
    Ok(Instruction {
        program_id: *program_id,
        accounts,
        data,
    })
}

fn build_ix_no_args(program_id: &Pubkey, method: &str, accounts: Vec<AccountMeta>) -> Result<Instruction> {
    Ok(Instruction {
        program_id: *program_id,
        accounts,
        data: anchor_discriminator(method).to_vec(),
    })
}

fn deserialize_account<T: BorshDeserialize>(data: &[u8]) -> Result<T> {
    if data.len() < 8 {
        return Err(anyhow!("Account data too short"));
    }
    let mut reader = &data[8..];
    T::deserialize_reader(&mut reader).map_err(|e| anyhow!("Deserialization error: {}", e))
}

fn find_queue_pda(program_id: &Pubkey, authority: &Pubkey, name: &str) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"queue", authority.as_ref(), name.as_bytes()],
        program_id,
    )
}

fn find_bucket_pda(program_id: &Pubkey, queue: &Pubkey, bucket_index: u8) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"bucket", queue.as_ref(), &[bucket_index]], program_id)
}

fn find_job_pda(program_id: &Pubkey, queue: &Pubkey, job_id: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"job", queue.as_ref(), &job_id.to_le_bytes()], program_id)
}

fn find_worker_pda(program_id: &Pubkey, queue: &Pubkey, worker: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"worker", queue.as_ref(), worker.as_ref()], program_id)
}
