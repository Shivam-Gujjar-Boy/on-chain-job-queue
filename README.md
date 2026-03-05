# On-Chain Job Queue — Solana Program

A production-grade **distributed job queue** system rebuilt as a Solana on-chain program using Anchor (Rust). This project reimagines the familiar BullMQ/Redis queue pattern as a decentralized state machine running on Solana, demonstrating how traditional Web2 backend infrastructure can be redesigned using on-chain architecture.

> **Program ID:** [`5MSKMK96xy7rVkXYAgBPZmZfwRkvehe8P94qvVHhCSP1`](https://explorer.solana.com/address/5MSKMK96xy7rVkXYAgBPZmZfwRkvehe8P94qvVHhCSP1?cluster=devnet)  
> **Network:** Solana Devnet  
> **Framework:** Anchor 0.32.1 / Solana 2.3.0

---

## Table of Contents

- [Overview](#overview)
- [How This Works in Web2](#how-this-works-in-web2)
- [How This Works on Solana](#how-this-works-on-solana)
- [Architecture & Account Model](#architecture--account-model)
- [Tradeoffs & Constraints](#tradeoffs--constraints)
- [Program Instructions](#program-instructions)
- [Project Structure](#project-structure)
- [Getting Started](#getting-started)
- [CLI Client Usage](#cli-client-usage)
- [Testing](#testing)
- [Devnet Transaction Links](#devnet-transaction-links)
- [Design Decisions](#design-decisions)

---

## Overview

This system allows **any company/entity** to create their own job queue on-chain, register authorized workers, and orchestrate job processing with full lifecycle management:

- **Multi-tenant queues** — anyone can create independent queue instances
- **Sharded bucket architecture** — jobs are distributed across configurable buckets (1–16) for contention-free parallel claiming
- **Full job lifecycle** — Pending → Assigned → Completed/Failed/TimedOut with automatic retries
- **Heartbeat-based timeout detection** — workers must heartbeat or jobs become reclaimable via a permissionless crank
- **Priority scheduling** — jobs carry a priority byte (0–255) for priority-aware worker processing
- **Off-chain data references** — only SHA-256 hashes stored on-chain; raw data stays off-chain
- **Event emission** — all state transitions emit Anchor events for off-chain indexing
- **Role-based access control** — authority-only queue management, registered-worker-only claiming/completing

---

## How This Works in Web2

A traditional backend job queue (BullMQ, Celery, Sidekiq, AWS SQS) typically consists of:

```
┌─────────────┐     ┌──────────────┐     ┌─────────────┐
│  Producer    │────▶│  Queue Broker │────▶│  Worker(s)  │
│  (API/App)   │     │  (Redis/SQS) │     │  (Consumers)│
└─────────────┘     └──────────────┘     └─────────────┘
```

| Component | Web2 Implementation |
|-----------|-------------------|
| **Queue Broker** | Redis, RabbitMQ, Amazon SQS — centralized message broker |
| **Job Storage** | In-memory (Redis) or database-backed |
| **Worker Discovery** | Service registry, DNS, container orchestration |
| **Claiming** | Atomic `BRPOPLPUSH` (Redis) or `ReceiveMessage` (SQS) with visibility timeout |
| **Retries** | Exponential backoff with configurable max retries |
| **Ordering** | FIFO with optional priority queues |
| **Monitoring** | Separate dashboards (Bull Board, Flower) reading broker state |
| **Trust Model** | All components trust the central broker; single point of failure |
| **State** | Ephemeral — lost if broker crashes without persistence |

### Key Web2 assumptions:
1. A **single trusted operator** runs the broker
2. Workers **pull** jobs from the broker over network connections
3. State is centralized and **can be lost** (Redis flushes, SQS message expiry)
4. Monitoring requires **separate systems** connected to the broker

---

## How This Works on Solana

On Solana, the **blockchain itself is the queue broker**. State is stored in Program Derived Accounts (PDAs), transitions are atomic on-chain instructions, and anyone can verify the queue's state by reading accounts.

```
┌─────────────┐     ┌─────────────────────────┐     ┌─────────────┐
│  Submitter   │────▶│  Solana Program (Queue)  │────▶│  Worker(s)  │
│  (Any signer)│     │  State in PDAs           │     │  (Keypairs) │
└─────────────┘     └─────────────────────────┘     └─────────────┘
                           │                              │
                     Events emitted                Workers sign
                     for indexing                   claim/complete txs
```

| Component | Solana Implementation |
|-----------|---------------------|
| **Queue Broker** | On-chain program with PDA-based state accounts |
| **Job Storage** | Deterministic PDAs — permanent, verifiable, on-chain |
| **Worker Discovery** | Worker PDA accounts created by queue authority |
| **Claiming** | Atomic instruction execution — no double-claims possible |
| **Retries** | On-chain retry counter with configurable max; auto-requeue |
| **Ordering** | Bucket-sharded with priority byte; workers sort off-chain |
| **Monitoring** | Read account data directly; events via `getProgramAccounts` |
| **Trust Model** | Trustless — program logic enforced by validators |
| **State** | Permanent — on-chain until explicitly closed |

### Key Solana differences:
1. **No single operator** — the program runs on thousands of validators
2. Workers **push** transactions to claim jobs (not pull from broker)
3. State is **permanent and auditable** on the ledger
4. **Atomic guarantees** — a job claim either succeeds fully or fails entirely
5. **Permissionless cranking** — anyone can trigger timeout resolution

---

## Architecture & Account Model

### PDA Hierarchy

```
Queue PDA (per queue instance)
├── seeds: ["queue", authority, name]
├── Stores: config, counters, authority
│
├── Bucket PDAs (sharded, 1-16 per queue)
│   ├── seeds: ["bucket", queue, bucket_index]
│   └── Stores: pending count per shard
│
├── Job PDAs (per job)
│   ├── seeds: ["job", queue, job_id_le_bytes]
│   └── Stores: status, data_hash, worker, retries, timestamps
│
└── Worker PDAs (per registered worker)
    ├── seeds: ["worker", queue, worker_authority]
    └── Stores: stats, active flag, queue reference
```

### Sharded Bucket Architecture

To prevent worker contention (a critical scaling issue), jobs are distributed across **sharded buckets** using `job_id % num_buckets`:

```
Queue (num_buckets = 4)
├── Bucket 0: Jobs #0, #4, #8, #12, ...
├── Bucket 1: Jobs #1, #5, #9, #13, ...
├── Bucket 2: Jobs #2, #6, #10, #14, ...
└── Bucket 3: Jobs #3, #7, #11, #15, ...
```

Workers targeting different buckets never contend on the same account, enabling true **parallel processing**. This mirrors Redis Cluster's hash-slot approach but implemented deterministically on-chain.

### Job State Machine

```
                  ┌──────────┐
          submit  │          │  claim
     ────────────▶│ PENDING  │────────────┐
     │            │          │            │
     │            └──────────┘            ▼
     │                 ▲            ┌──────────┐
     │      retry      │            │          │  complete
     │   (retries < max│            │ ASSIGNED │──────────▶ COMPLETED
     │                 │            │          │
     │            ┌────┴───┐        └─────┬────┘
     │            │ REQUEUE│              │
     │            └────────┘         fail │ timeout
     │                 ▲                  │    │
     │                 │            ┌─────▼────▼──┐
     │                 └────────────│  FAIL/      │
     │                  if retries  │  TIMEOUT    │  if retries
     │                  remain      │  (check)    │  exhausted
     │                              └──────┬──────┘
     │                                     │
     │                                     ▼
     │                              ┌──────────┐
     │                              │ FAILED / │
     └──────────────────────────────│ TIMEDOUT │ (permanent)
                                    └──────────┘
```

---

## Tradeoffs & Constraints

| Aspect | Web2 Queue | On-Chain Queue | Impact |
|--------|-----------|----------------|--------|
| **Throughput** | 100K+ jobs/sec (Redis) | ~400 TPS (Solana) | On-chain is orders of magnitude slower; best for high-value, auditable jobs |
| **Cost** | ~Free (infrastructure cost) | ~$0.00025 per tx (SOL) | Each state transition costs a transaction fee |
| **Latency** | < 1ms (Redis) | ~400ms (Solana slot time) | Not suitable for real-time processing |
| **Data Storage** | Store full payloads | Store only hashes | Raw data must be kept off-chain (IPFS, Arweave, S3) |
| **Account Size** | Unlimited | 10 MB max per account | Job metadata must be compact |
| **Ordering** | Native FIFO with priorities | Off-chain priority sorting via `getProgramAccounts` | Workers must query and sort available jobs client-side |
| **Visibility** | Private to broker operator | Fully public on-chain | All job metadata is readable by anyone |
| **Reliability** | Depends on operator backups | Guaranteed by blockchain consensus | On-chain state is immutable and replicated |
| **Worker Auth** | API keys, mTLS | Ed25519 keypair signatures | Cryptographic guarantees; no credential rotation needed |
| **Timeout Crank** | Background thread in broker | Permissionless external crank | Anyone can resolve stale jobs — no single dependency |

### When to use on-chain job queues:
- Jobs that require **verifiable execution** (e.g., bounty payouts, oracle tasks)
- Multi-party workflows where **trust is limited** between participants
- Systems where **auditability** of job processing is critical
- **Decentralized worker networks** where no central coordinator exists

### When NOT to use:
- High-throughput background processing (use Redis/BullMQ)
- Sub-millisecond latency requirements
- Large payload processing (data must be off-chain anyway)

---

## Program Instructions

| Instruction | Signer | Description |
|-------------|--------|-------------|
| `create_queue` | Authority | Create a new queue with name, retries, timeout, bucket count |
| `init_bucket` | Authority | Initialize a sharded bucket (must be sequential: 0, 1, 2, ...) |
| `update_queue` | Authority | Update queue config (retries, timeout, pause, submission policy) |
| `register_worker` | Authority | Register a worker keypair for the queue |
| `deregister_worker` | Authority | Deactivate a registered worker |
| `submit_job` | Submitter* | Submit a job with data hash and priority |
| `claim_job` | Worker | Atomically claim a pending job |
| `heartbeat` | Worker | Keep-alive signal for an assigned job |
| `complete_job` | Worker | Complete a job with result hash |
| `fail_job` | Worker | Report job failure (auto-retries if retries remain) |
| `timeout_job` | Anyone | Permissionless crank to timeout stale jobs |

*Submitter can be anyone, or restricted to authority only via `require_authority_submit`.

---

## Project Structure

```
on-chain-job-queue/
├── programs/job-queue/
│   └── src/
│       ├── lib.rs              # Program entry point & instruction routing
│       ├── state/              # Account data structures
│       │   ├── queue.rs        # Queue PDA (config, counters)
│       │   ├── job.rs          # Job PDA (lifecycle, data hash, status)
│       │   ├── worker.rs       # Worker PDA (stats, active flag)
│       │   └── bucket.rs       # Sharded bucket PDA (pending count)
│       ├── instructions/       # Instruction handlers
│       │   ├── create_queue.rs
│       │   ├── init_bucket.rs
│       │   ├── update_queue.rs
│       │   ├── register_worker.rs
│       │   ├── deregister_worker.rs
│       │   ├── submit_job.rs
│       │   ├── claim_job.rs
│       │   ├── heartbeat.rs
│       │   ├── complete_job.rs
│       │   ├── fail_job.rs
│       │   └── timeout_job.rs
│       ├── errors.rs           # Custom error codes
│       └── events.rs           # Anchor events for indexing
├── cli/                        # Rust CLI client
│   └── src/main.rs             # Full CLI with all commands
├── tests/
│   └── job-queue.ts            # 25 comprehensive TypeScript tests
├── Anchor.toml
├── Cargo.toml
└── README.md
```

---

## Getting Started

### Prerequisites

- Rust 1.70+ with `cargo`
- Solana CLI 2.3.0 (`solana --version`)
- Anchor CLI 0.32.1 (`anchor --version`)
- Node.js 18+ and Yarn

### Build the Program

```bash
# Clone the repo
git clone https://github.com/YOUR_USERNAME/on-chain-job-queue.git
cd on-chain-job-queue

# Build the Solana program
anchor build

# Install JS dependencies for tests
yarn install
```

### Run Tests (Local Validator)

```bash
anchor test
```

This spins up a local validator, deploys the program, and runs all 25 tests covering:
- Queue creation & validation
- Bucket initialization (sequential enforcement)
- Worker registration/deregistration
- Job submission across sharded buckets
- Job claiming (with auth checks)
- Heartbeat mechanism
- Job completion with result hashes
- Job failure with automatic retries
- Retry exhaustion → permanent failure
- Parallel job processing by multiple workers
- Timeout detection & re-queuing
- Queue pause/unpause
- Queue configuration updates

### Build the CLI

```bash
cd cli
cargo build --release
# Binary at: cli/target/release/jq-cli
```

---

## CLI Client Usage

### Create a Queue

```bash
jq-cli --rpc-url https://api.devnet.solana.com create-queue \
  --name "my-queue" \
  --max-retries 3 \
  --timeout 60 \
  --buckets 4
```

### Initialize Buckets

```bash
jq-cli init-buckets --queue <QUEUE_ADDRESS>
```

### Register a Worker

```bash
jq-cli register-worker \
  --queue <QUEUE_ADDRESS> \
  --worker-keypair ./worker-key.json
```

### Submit a Job

```bash
jq-cli submit-job \
  --queue <QUEUE_ADDRESS> \
  --data-hash "a1b2c3d4...64hex" \
  --priority 10
```

### Claim, Heartbeat, Complete

```bash
# Claim
jq-cli claim-job --queue <QUEUE> --job-id 0 --worker-keypair ./worker.json

# Heartbeat
jq-cli heartbeat --queue <QUEUE> --job-id 0 --worker-keypair ./worker.json

# Complete
jq-cli complete-job --queue <QUEUE> --job-id 0 \
  --result-hash "deadbeef...64hex" --worker-keypair ./worker.json
```

### Run Continuous Worker Process

```bash
jq-cli run-worker \
  --queue <QUEUE_ADDRESS> \
  --worker-keypair ./worker.json \
  --poll-interval 5
```

This starts an automated worker loop that:
1. Polls for pending jobs via `getProgramAccounts`
2. Sorts by priority (highest first), then FIFO within same priority
3. Claims, heartbeats, processes, and completes jobs automatically

### Query Status

```bash
# Queue overview
jq-cli queue-status --queue <QUEUE_ADDRESS>

# Individual job
jq-cli job-status --queue <QUEUE_ADDRESS> --job-id 0

# Worker stats
jq-cli worker-status --queue <QUEUE_ADDRESS> --worker <WORKER_PUBKEY>
```

### Timeout Stale Jobs (Permissionless Crank)

```bash
jq-cli timeout-job --queue <QUEUE_ADDRESS> --job-id 5
```

---

## Testing

### Test Results (25/25 passing)

```
  job-queue
    Queue Management
      ✔ creates a queue with valid configuration
      ✔ rejects queue with empty name
      ✔ rejects queue with invalid bucket count (0)
    Bucket Initialization
      ✔ initializes all buckets sequentially
      ✔ rejects out-of-order bucket initialization
    Worker Management
      ✔ registers workers
      ✔ rejects worker registration from non-authority
      ✔ deregisters a worker
    Job Submission
      ✔ submits a job to the queue
      ✔ submits multiple jobs across buckets
      ✔ rejects submission when queue is paused
    Job Claiming
      ✔ worker claims a pending job
      ✔ rejects claim from inactive worker
      ✔ rejects claim on already-assigned job
    Heartbeat
      ✔ worker sends heartbeat for assigned job
      ✔ rejects heartbeat from wrong worker
    Job Completion
      ✔ worker completes a job with result hash
    Job Failure & Retry
      ✔ worker fails a job and it retries
      ✔ exhausts all retries and permanently fails
    Parallel Job Processing
      ✔ two workers claim different jobs simultaneously
    Job Timeout
      ✔ times out a stale job and re-queues it
      ✔ rejects timeout if job hasn't actually timed out
    Queue Configuration Update
      ✔ authority updates queue settings
      ✔ rejects update from non-authority
    Final Statistics
      ✔ displays queue statistics

  25 passing (22s)
```

---

## Devnet Transaction Links

Program deployed and fully operational on Solana Devnet:

| Action | Transaction |
|--------|-------------|
| **Program Deploy** | [5Ve4JeaB394riVnKAXNgtPHkWNG6N7fV2mAdY9xBju42K7qx18apwbRE8FERdRZqf2RouLY1CJZ31X5hVpDBNfoP](https://explorer.solana.com/tx/5Ve4JeaB394riVnKAXNgtPHkWNG6N7fV2mAdY9xBju42K7qx18apwbRE8FERdRZqf2RouLY1CJZ31X5hVpDBNfoP?cluster=devnet) |
| **Create Queue** | [4fn2QfGnUZok6PMvJ21ZhApkDDANMGMsH9xsUFNM5TZCVHRKvgQy2Q2NvNXAipNTbWWYoadyAAs1Wz9FvjYGuBdB](https://explorer.solana.com/tx/4fn2QfGnUZok6PMvJ21ZhApkDDANMGMsH9xsUFNM5TZCVHRKvgQy2Q2NvNXAipNTbWWYoadyAAs1Wz9FvjYGuBdB?cluster=devnet) |
| **Init Bucket 0** | [4eKPVJoLJVmLa7Fnsa8xuYhCirQLcTy2LVVVvbxDmStUZfdJwQvuA8PE9cVULwzcetfj4tWP6PqhLfSSpR2xmLc5](https://explorer.solana.com/tx/4eKPVJoLJVmLa7Fnsa8xuYhCirQLcTy2LVVVvbxDmStUZfdJwQvuA8PE9cVULwzcetfj4tWP6PqhLfSSpR2xmLc5?cluster=devnet) |
| **Init Bucket 1** | [BeUyC8UjYytExcRtNA4cgBCaFeNDiJNEpeFvxjNMSNeZvna4afoYbyYspKBw5xXgNVudR51rufJPwRLCUsmJVYd](https://explorer.solana.com/tx/BeUyC8UjYytExcRtNA4cgBCaFeNDiJNEpeFvxjNMSNeZvna4afoYbyYspKBw5xXgNVudR51rufJPwRLCUsmJVYd?cluster=devnet) |
| **Init Bucket 2** | [5eBNPx91BPTpbYSxhwiCbrJSSS1cDA8tJQm81bZ6mQchcMt4cbw3Qqe2F4hqriwq6LiUiHP2bgniu7aHvmXRaV6F](https://explorer.solana.com/tx/5eBNPx91BPTpbYSxhwiCbrJSSS1cDA8tJQm81bZ6mQchcMt4cbw3Qqe2F4hqriwq6LiUiHP2bgniu7aHvmXRaV6F?cluster=devnet) |
| **Init Bucket 3** | [4PF6GHSUYYTm4GEkHCyFptY3GXj5ygiJjh5VhRiewEFs6H3ERDiST7tx1YqGkVSYk5EPK53fABNmgtf17iw4JX9p](https://explorer.solana.com/tx/4PF6GHSUYYTm4GEkHCyFptY3GXj5ygiJjh5VhRiewEFs6H3ERDiST7tx1YqGkVSYk5EPK53fABNmgtf17iw4JX9p?cluster=devnet) |
| **Register Worker** | [2YNESkqWguZpWo6Rk5JmpM44wDqrMCqX83DTkfxQQYgQwzLYmNzZrgXEPYDt3Dvoa5fXE4mQmLD8xDEffxhQnejS](https://explorer.solana.com/tx/2YNESkqWguZpWo6Rk5JmpM44wDqrMCqX83DTkfxQQYgQwzLYmNzZrgXEPYDt3Dvoa5fXE4mQmLD8xDEffxhQnejS?cluster=devnet) |
| **Submit Job #0** | [jWL89NybdATnbheAkUxLjhYQ3awWCv18egGpUpWHzqGQ4eopyKbAgERJZibTGM1WeeuFoXSuUgPV6s2DXWVdZsr](https://explorer.solana.com/tx/jWL89NybdATnbheAkUxLjhYQ3awWCv18egGpUpWHzqGQ4eopyKbAgERJZibTGM1WeeuFoXSuUgPV6s2DXWVdZsr?cluster=devnet) |
| **Submit Job #1** | [3jxh52PPc7Gzx3LWXJ5HihGD12JHex3dzjrXZgctaLkGvSM9st4wq24upFjeofuiFrpuBhzspvvrQ1fGYEBb7W52](https://explorer.solana.com/tx/3jxh52PPc7Gzx3LWXJ5HihGD12JHex3dzjrXZgctaLkGvSM9st4wq24upFjeofuiFrpuBhzspvvrQ1fGYEBb7W52?cluster=devnet) |
| **Claim Job #0** | [2BvGwZthnhBvbn8v1KHsiZQHqDiLLSPLnLx26yoZQQVXfsGLavtdpFefBcnom4ENBXWqpPn2osv9dyHhtJW3ASzq](https://explorer.solana.com/tx/2BvGwZthnhBvbn8v1KHsiZQHqDiLLSPLnLx26yoZQQVXfsGLavtdpFefBcnom4ENBXWqpPn2osv9dyHhtJW3ASzq?cluster=devnet) |
| **Heartbeat** | [3pwnwHXr3EiUESWwtr9Yx1avRoy4pirFWWzPQh6Riqsf6RHXrs27HRBcYH98jPSh3MAq8Xod6RodpSjEU7cpd5cU](https://explorer.solana.com/tx/3pwnwHXr3EiUESWwtr9Yx1avRoy4pirFWWzPQh6Riqsf6RHXrs27HRBcYH98jPSh3MAq8Xod6RodpSjEU7cpd5cU?cluster=devnet) |
| **Complete Job #0** | [3EJqcdYbuQbNJi6sA814KyvecDEMRBANWsEYD6izF4gjDehKsQAAVVkMydF1iXqqpzHdFqPaiwi42ZVFy1mkDxQ5](https://explorer.solana.com/tx/3EJqcdYbuQbNJi6sA814KyvecDEMRBANWsEYD6izF4gjDehKsQAAVVkMydF1iXqqpzHdFqPaiwi42ZVFy1mkDxQ5?cluster=devnet) |

**Live Queue Account:** [G5mipxEvGxzfa3Yj7V5uFHUKhZT1jgTSZVbmknxEChxg](https://explorer.solana.com/address/G5mipxEvGxzfa3Yj7V5uFHUKhZT1jgTSZVbmknxEChxg?cluster=devnet)

---

## Design Decisions

### Why Sharded Buckets?
On Solana, only one transaction can write to a given account per slot. If all workers compete for the same "queue head" account, you get massive transaction failures. The bucket architecture distributes jobs deterministically across N shards, so workers targeting different buckets never conflict. This mirrors how Redis Cluster uses hash slots.

### Why Off-Chain Data via Hashes?
Solana accounts are expensive to rent (~6.9 SOL/MB/year). Storing a 32-byte SHA-256 hash instead of raw job data keeps on-chain costs minimal while maintaining verifiability. The actual job payload lives on IPFS, Arweave, S3, or any off-chain store — the on-chain hash ensures integrity.

### Why Permissionless Timeout Cranking?
In Web2, the broker itself detects and handles timeouts. On-chain, we need someone to call the timeout instruction. Making it permissionless means anyone (a bot, another worker, or the queue authority) can resolve stale jobs, preventing dependency on any single actor.

### Why Not Store a Priority Queue On-Chain?
Maintaining a sorted data structure on-chain is extremely expensive in compute units. Instead, we store priority as a simple byte on each job and let workers sort client-side using `getProgramAccounts` with `memcmp` filters. This pushes sorting compute off-chain where it's free.

### Why Anchor Over Raw Native?
While the bounty allows raw SDK, Anchor provides: automatic (de)serialization, account validation macros, `InitSpace` for precise account sizing, IDL generation for clients, and event emission — reducing boilerplate by ~60% while maintaining full control over program logic.

---

## License

MIT
A Dsitributed On-chain Job Queue inspired by BullMQ
