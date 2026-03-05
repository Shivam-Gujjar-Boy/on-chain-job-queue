import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { JobQueue } from "../target/types/job_queue";
import { expect } from "chai";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";

describe("job-queue", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.JobQueue as Program<JobQueue>;
  const authority = provider.wallet;

  // Test queue configuration
  const QUEUE_NAME = "test-queue";
  const MAX_RETRIES = 3;
  const JOB_TIMEOUT = 10; // 10 seconds for testing
  const NUM_BUCKETS = 4;

  // Worker keypairs
  const worker1 = Keypair.generate();
  const worker2 = Keypair.generate();
  const worker3 = Keypair.generate();

  // Store derived addresses
  let queuePda: PublicKey;
  let queueBump: number;
  let bucketPdas: PublicKey[] = [];

  // Helper: derive queue PDA
  function findQueuePda(auth: PublicKey, name: string): [PublicKey, number] {
    return PublicKey.findProgramAddressSync(
      [Buffer.from("queue"), auth.toBuffer(), Buffer.from(name)],
      program.programId
    );
  }

  // Helper: derive bucket PDA
  function findBucketPda(queue: PublicKey, index: number): [PublicKey, number] {
    return PublicKey.findProgramAddressSync(
      [Buffer.from("bucket"), queue.toBuffer(), Buffer.from([index])],
      program.programId
    );
  }

  // Helper: derive job PDA
  function findJobPda(queue: PublicKey, jobId: number): [PublicKey, number] {
    const buf = Buffer.alloc(8);
    buf.writeBigUInt64LE(BigInt(jobId));
    return PublicKey.findProgramAddressSync(
      [Buffer.from("job"), queue.toBuffer(), buf],
      program.programId
    );
  }

  // Helper: derive worker PDA
  function findWorkerPda(
    queue: PublicKey,
    workerAuth: PublicKey
  ): [PublicKey, number] {
    return PublicKey.findProgramAddressSync(
      [Buffer.from("worker"), queue.toBuffer(), workerAuth.toBuffer()],
      program.programId
    );
  }

  // Helper: generate a random 32-byte hash
  function randomHash(): number[] {
    return Array.from({ length: 32 }, () => Math.floor(Math.random() * 256));
  }

  // Helper: sleep for ms
  function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  // =========================================================================
  // 1. Queue Creation
  // =========================================================================
  describe("Queue Management", () => {
    it("creates a queue with valid configuration", async () => {
      [queuePda, queueBump] = findQueuePda(authority.publicKey, QUEUE_NAME);

      const tx = await program.methods
        .createQueue(QUEUE_NAME, MAX_RETRIES, new anchor.BN(JOB_TIMEOUT), NUM_BUCKETS, false)
        .accountsPartial({
          authority: authority.publicKey,
          queue: queuePda,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      console.log("    Queue created:", queuePda.toBase58());
      console.log("    Tx:", tx);

      const queue = await program.account.queue.fetch(queuePda);
      expect(queue.authority.toBase58()).to.equal(authority.publicKey.toBase58());
      expect(queue.name).to.equal(QUEUE_NAME);
      expect(queue.maxRetries).to.equal(MAX_RETRIES);
      expect(queue.jobTimeoutSeconds.toNumber()).to.equal(JOB_TIMEOUT);
      expect(queue.numBuckets).to.equal(NUM_BUCKETS);
      expect(queue.bucketsInitialized).to.equal(0);
      expect(queue.totalJobsCreated.toNumber()).to.equal(0);
      expect(queue.pendingCount.toNumber()).to.equal(0);
      expect(queue.isPaused).to.equal(false);
    });

    it("rejects queue with empty name", async () => {
      try {
        const [badPda] = findQueuePda(authority.publicKey, "");
        await program.methods
          .createQueue("", 3, new anchor.BN(60), 4, false)
          .accountsPartial({
            authority: authority.publicKey,
            queue: badPda,
            systemProgram: SystemProgram.programId,
          })
          .rpc();
        expect.fail("Should have thrown");
      } catch (err) {
        expect(err.toString()).to.include("NameEmpty");
      }
    });

    it("rejects queue with invalid bucket count (0)", async () => {
      try {
        const [badPda] = findQueuePda(authority.publicKey, "bad-buckets");
        await program.methods
          .createQueue("bad-buckets", 3, new anchor.BN(60), 0, false)
          .accountsPartial({
            authority: authority.publicKey,
            queue: badPda,
            systemProgram: SystemProgram.programId,
          })
          .rpc();
        expect.fail("Should have thrown");
      } catch (err) {
        expect(err.toString()).to.include("InvalidBucketCount");
      }
    });
  });

  // =========================================================================
  // 2. Bucket Initialization
  // =========================================================================
  describe("Bucket Initialization", () => {
    it("initializes all buckets sequentially", async () => {
      for (let i = 0; i < NUM_BUCKETS; i++) {
        const [bucketPda] = findBucketPda(queuePda, i);
        bucketPdas.push(bucketPda);

        const tx = await program.methods
          .initBucket(i)
          .accountsPartial({
            authority: authority.publicKey,
            queue: queuePda,
            bucket: bucketPda,
            systemProgram: SystemProgram.programId,
          })
          .rpc();

        console.log(`    Bucket ${i} initialized: ${bucketPda.toBase58()}`);
      }

      const queue = await program.account.queue.fetch(queuePda);
      expect(queue.bucketsInitialized).to.equal(NUM_BUCKETS);
    });

    it("rejects out-of-order bucket initialization", async () => {
      // Create a new queue for this test
      const name2 = "order-test";
      const [q2] = findQueuePda(authority.publicKey, name2);

      await program.methods
        .createQueue(name2, 3, new anchor.BN(60), 4, false)
        .accountsPartial({
          authority: authority.publicKey,
          queue: q2,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      // Try to init bucket 1 before bucket 0
      try {
        const [b1] = findBucketPda(q2, 1);
        await program.methods
          .initBucket(1)
          .accountsPartial({
            authority: authority.publicKey,
            queue: q2,
            bucket: b1,
            systemProgram: SystemProgram.programId,
          })
          .rpc();
        expect.fail("Should have thrown");
      } catch (err) {
        expect(err.toString()).to.include("BucketInitOutOfOrder");
      }
    });
  });

  // =========================================================================
  // 3. Worker Registration
  // =========================================================================
  describe("Worker Management", () => {
    it("registers workers", async () => {
      for (const worker of [worker1, worker2, worker3]) {
        const [workerPda] = findWorkerPda(queuePda, worker.publicKey);

        await program.methods
          .registerWorker()
          .accountsPartial({
            authority: authority.publicKey,
            queue: queuePda,
            workerAuthority: worker.publicKey,
            worker: workerPda,
            systemProgram: SystemProgram.programId,
          })
          .rpc();

        console.log(`    Worker registered: ${worker.publicKey.toBase58()}`);

        const workerAccount = await program.account.worker.fetch(workerPda);
        expect(workerAccount.isActive).to.equal(true);
        expect(workerAccount.jobsCompleted.toNumber()).to.equal(0);
      }
    });

    it("rejects worker registration from non-authority", async () => {
      const fakeAuth = Keypair.generate();
      const fakeWorker = Keypair.generate();
      const [workerPda] = findWorkerPda(queuePda, fakeWorker.publicKey);

      try {
        await program.methods
          .registerWorker()
          .accountsPartial({
            authority: fakeAuth.publicKey,
            queue: queuePda,
            workerAuthority: fakeWorker.publicKey,
            worker: workerPda,
            systemProgram: SystemProgram.programId,
          })
          .signers([fakeAuth])
          .rpc();
        expect.fail("Should have thrown");
      } catch (err) {
        // Expected: authority mismatch
        expect(err).to.exist;
      }
    });

    it("deregisters a worker", async () => {
      const [workerPda] = findWorkerPda(queuePda, worker3.publicKey);

      await program.methods
        .deregisterWorker()
        .accountsPartial({
          authority: authority.publicKey,
          queue: queuePda,
          worker: workerPda,
        })
        .rpc();

      const workerAccount = await program.account.worker.fetch(workerPda);
      expect(workerAccount.isActive).to.equal(false);
    });
  });

  // =========================================================================
  // 4. Job Submission
  // =========================================================================
  describe("Job Submission", () => {
    it("submits a job to the queue", async () => {
      const queue = await program.account.queue.fetch(queuePda);
      const jobId = queue.totalJobsCreated.toNumber();
      const bucketIndex = jobId % NUM_BUCKETS;

      const [jobPda] = findJobPda(queuePda, jobId);
      const [bucketPda] = findBucketPda(queuePda, bucketIndex);

      const dataHash = randomHash();

      const tx = await program.methods
        .submitJob(dataHash, 5) // priority 5
        .accountsPartial({
          submitter: authority.publicKey,
          queue: queuePda,
          job: jobPda,
          bucket: bucketPda,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      console.log(`    Job #${jobId} submitted to bucket ${bucketIndex}. Tx: ${tx}`);

      const jobAccount = await program.account.job.fetch(jobPda);
      expect(jobAccount.jobId.toNumber()).to.equal(jobId);
      expect(jobAccount.status).to.deep.equal({ pending: {} });
      expect(jobAccount.priority).to.equal(5);
      expect(jobAccount.retryCount).to.equal(0);
      expect(jobAccount.maxRetries).to.equal(MAX_RETRIES);
    });

    it("submits multiple jobs across buckets", async () => {
      for (let i = 0; i < 8; i++) {
        const queue = await program.account.queue.fetch(queuePda);
        const jobId = queue.totalJobsCreated.toNumber();
        const bucketIndex = jobId % NUM_BUCKETS;

        const [jobPda] = findJobPda(queuePda, jobId);
        const [bucketPda] = findBucketPda(queuePda, bucketIndex);

        await program.methods
          .submitJob(randomHash(), i * 10) // varying priorities
          .accountsPartial({
            submitter: authority.publicKey,
            queue: queuePda,
            job: jobPda,
            bucket: bucketPda,
            systemProgram: SystemProgram.programId,
          })
          .rpc();
      }

      const queue = await program.account.queue.fetch(queuePda);
      expect(queue.totalJobsCreated.toNumber()).to.equal(9); // 1 from prev test + 8
      expect(queue.pendingCount.toNumber()).to.equal(9);
      console.log(`    9 jobs submitted across ${NUM_BUCKETS} buckets`);

      // Verify bucket distribution
      for (let i = 0; i < NUM_BUCKETS; i++) {
        const bucket = await program.account.bucket.fetch(bucketPdas[i]);
        console.log(`    Bucket ${i}: ${bucket.pendingCount} pending`);
        expect(bucket.pendingCount).to.be.greaterThan(0);
      }
    });

    it("rejects submission when queue is paused", async () => {
      // Pause the queue
      await program.methods
        .updateQueue(null, null, true, null)
        .accountsPartial({
          authority: authority.publicKey,
          queue: queuePda,
        })
        .rpc();

      try {
        const queue = await program.account.queue.fetch(queuePda);
        const jobId = queue.totalJobsCreated.toNumber();
        const bucketIndex = jobId % NUM_BUCKETS;
        const [jobPda] = findJobPda(queuePda, jobId);
        const [bucketPda] = findBucketPda(queuePda, bucketIndex);

        await program.methods
          .submitJob(randomHash(), 0)
          .accountsPartial({
            submitter: authority.publicKey,
            queue: queuePda,
            job: jobPda,
            bucket: bucketPda,
            systemProgram: SystemProgram.programId,
          })
          .rpc();
        expect.fail("Should have thrown");
      } catch (err) {
        expect(err.toString()).to.include("QueuePaused");
      }

      // Unpause the queue
      await program.methods
        .updateQueue(null, null, false, null)
        .accountsPartial({
          authority: authority.publicKey,
          queue: queuePda,
        })
        .rpc();
    });
  });

  // =========================================================================
  // 5. Job Claiming & Processing
  // =========================================================================
  describe("Job Claiming", () => {
    it("worker claims a pending job", async () => {
      const jobId = 0;
      const [jobPda] = findJobPda(queuePda, jobId);
      const jobBefore = await program.account.job.fetch(jobPda);
      const bucketIndex = jobBefore.bucketIndex;
      const [bucketPda] = findBucketPda(queuePda, bucketIndex);
      const [workerPda] = findWorkerPda(queuePda, worker1.publicKey);

      await program.methods
        .claimJob()
        .accountsPartial({
          workerAuthority: worker1.publicKey,
          queue: queuePda,
          job: jobPda,
          bucket: bucketPda,
          worker: workerPda,
        })
        .signers([worker1])
        .rpc();

      const jobAfter = await program.account.job.fetch(jobPda);
      expect(jobAfter.status).to.deep.equal({ assigned: {} });
      expect(jobAfter.assignedWorker.toBase58()).to.equal(
        worker1.publicKey.toBase58()
      );
      expect(jobAfter.lastHeartbeat.toNumber()).to.be.greaterThan(0);

      console.log(`    Worker1 claimed job #${jobId}`);
    });

    it("rejects claim from inactive worker", async () => {
      const jobId = 1;
      const [jobPda] = findJobPda(queuePda, jobId);
      const jobData = await program.account.job.fetch(jobPda);
      const [bucketPda] = findBucketPda(queuePda, jobData.bucketIndex);
      const [workerPda] = findWorkerPda(queuePda, worker3.publicKey);

      try {
        await program.methods
          .claimJob()
          .accountsPartial({
            workerAuthority: worker3.publicKey,
            queue: queuePda,
            job: jobPda,
            bucket: bucketPda,
            worker: workerPda,
          })
          .signers([worker3])
          .rpc();
        expect.fail("Should have thrown");
      } catch (err) {
        expect(err.toString()).to.include("WorkerNotActive");
      }
    });

    it("rejects claim on already-assigned job", async () => {
      const jobId = 0; // Already claimed by worker1
      const [jobPda] = findJobPda(queuePda, jobId);
      const jobData = await program.account.job.fetch(jobPda);
      const [bucketPda] = findBucketPda(queuePda, jobData.bucketIndex);
      const [workerPda] = findWorkerPda(queuePda, worker2.publicKey);

      try {
        await program.methods
          .claimJob()
          .accountsPartial({
            workerAuthority: worker2.publicKey,
            queue: queuePda,
            job: jobPda,
            bucket: bucketPda,
            worker: workerPda,
          })
          .signers([worker2])
          .rpc();
        expect.fail("Should have thrown");
      } catch (err) {
        expect(err.toString()).to.include("JobNotPending");
      }
    });
  });

  // =========================================================================
  // 6. Heartbeat
  // =========================================================================
  describe("Heartbeat", () => {
    it("worker sends heartbeat for assigned job", async () => {
      const jobId = 0;
      const [jobPda] = findJobPda(queuePda, jobId);
      const [workerPda] = findWorkerPda(queuePda, worker1.publicKey);

      const jobBefore = await program.account.job.fetch(jobPda);
      const hbBefore = jobBefore.lastHeartbeat.toNumber();

      // Small delay to ensure timestamp difference
      await sleep(1500);

      await program.methods
        .heartbeat()
        .accountsPartial({
          workerAuthority: worker1.publicKey,
          queue: queuePda,
          job: jobPda,
          worker: workerPda,
        })
        .signers([worker1])
        .rpc();

      const jobAfter = await program.account.job.fetch(jobPda);
      expect(jobAfter.lastHeartbeat.toNumber()).to.be.greaterThanOrEqual(hbBefore);

      console.log("    Heartbeat sent successfully");
    });

    it("rejects heartbeat from wrong worker", async () => {
      const jobId = 0;
      const [jobPda] = findJobPda(queuePda, jobId);
      const [workerPda] = findWorkerPda(queuePda, worker2.publicKey);

      try {
        await program.methods
          .heartbeat()
          .accountsPartial({
            workerAuthority: worker2.publicKey,
            queue: queuePda,
            job: jobPda,
            worker: workerPda,
          })
          .signers([worker2])
          .rpc();
        expect.fail("Should have thrown");
      } catch (err) {
        expect(err.toString()).to.include("UnauthorizedWorker");
      }
    });
  });

  // =========================================================================
  // 7. Job Completion
  // =========================================================================
  describe("Job Completion", () => {
    it("worker completes a job with result hash", async () => {
      const jobId = 0;
      const [jobPda] = findJobPda(queuePda, jobId);
      const [workerPda] = findWorkerPda(queuePda, worker1.publicKey);
      const resultHash = randomHash();

      await program.methods
        .completeJob(resultHash)
        .accountsPartial({
          workerAuthority: worker1.publicKey,
          queue: queuePda,
          job: jobPda,
          worker: workerPda,
        })
        .signers([worker1])
        .rpc();

      const job = await program.account.job.fetch(jobPda);
      expect(job.status).to.deep.equal({ completed: {} });
      expect(job.completedAt.toNumber()).to.be.greaterThan(0);
      expect(Array.from(job.resultHash)).to.deep.equal(resultHash);

      const queue = await program.account.queue.fetch(queuePda);
      expect(queue.completedCount.toNumber()).to.equal(1);
      expect(queue.activeCount.toNumber()).to.equal(0);

      const worker = await program.account.worker.fetch(workerPda);
      expect(worker.jobsCompleted.toNumber()).to.equal(1);

      console.log("    Job #0 completed successfully");
    });
  });

  // =========================================================================
  // 8. Job Failure & Retry
  // =========================================================================
  describe("Job Failure & Retry", () => {
    let testJobId: number;

    it("worker fails a job and it retries", async () => {
      // Claim job #1
      testJobId = 1;
      const [jobPda] = findJobPda(queuePda, testJobId);
      const jobData = await program.account.job.fetch(jobPda);
      const [bucketPda] = findBucketPda(queuePda, jobData.bucketIndex);
      const [workerPda] = findWorkerPda(queuePda, worker1.publicKey);

      // Claim it
      await program.methods
        .claimJob()
        .accountsPartial({
          workerAuthority: worker1.publicKey,
          queue: queuePda,
          job: jobPda,
          bucket: bucketPda,
          worker: workerPda,
        })
        .signers([worker1])
        .rpc();

      // Fail it with error code 42
      await program.methods
        .failJob(42)
        .accountsPartial({
          workerAuthority: worker1.publicKey,
          queue: queuePda,
          job: jobPda,
          bucket: bucketPda,
          worker: workerPda,
        })
        .signers([worker1])
        .rpc();

      const jobAfter = await program.account.job.fetch(jobPda);
      expect(jobAfter.status).to.deep.equal({ pending: {} });
      expect(jobAfter.retryCount).to.equal(1);
      expect(jobAfter.errorCode).to.equal(42);
      expect(jobAfter.assignedWorker.toBase58()).to.equal(
        PublicKey.default.toBase58()
      );

      console.log("    Job #1 failed and returned to pending (retry 1/3)");
    });

    it("exhausts all retries and permanently fails", async () => {
      const [jobPda] = findJobPda(queuePda, testJobId);
      const [workerPda] = findWorkerPda(queuePda, worker1.publicKey);

      for (let retry = 1; retry < MAX_RETRIES; retry++) {
        const jobData = await program.account.job.fetch(jobPda);
        const [bucketPda] = findBucketPda(queuePda, jobData.bucketIndex);

        // Claim
        await program.methods
          .claimJob()
          .accountsPartial({
            workerAuthority: worker1.publicKey,
            queue: queuePda,
            job: jobPda,
            bucket: bucketPda,
            worker: workerPda,
          })
          .signers([worker1])
          .rpc();

        // Fail
        await program.methods
          .failJob(100 + retry)
          .accountsPartial({
            workerAuthority: worker1.publicKey,
            queue: queuePda,
            job: jobPda,
            bucket: bucketPda,
            worker: workerPda,
          })
          .signers([worker1])
          .rpc();
      }

      // One more claim + fail should exhaust retries
      const jobData = await program.account.job.fetch(jobPda);
      const [bucketPda] = findBucketPda(queuePda, jobData.bucketIndex);

      await program.methods
        .claimJob()
        .accountsPartial({
          workerAuthority: worker1.publicKey,
          queue: queuePda,
          job: jobPda,
          bucket: bucketPda,
          worker: workerPda,
        })
        .signers([worker1])
        .rpc();

      await program.methods
        .failJob(999)
        .accountsPartial({
          workerAuthority: worker1.publicKey,
          queue: queuePda,
          job: jobPda,
          bucket: bucketPda,
          worker: workerPda,
        })
        .signers([worker1])
        .rpc();

      const jobFinal = await program.account.job.fetch(jobPda);
      expect(jobFinal.status).to.deep.equal({ failed: {} });
      expect(jobFinal.retryCount).to.equal(MAX_RETRIES);
      expect(jobFinal.completedAt.toNumber()).to.be.greaterThan(0);

      console.log(`    Job #${testJobId} permanently failed after ${MAX_RETRIES} retries`);
    });
  });

  // =========================================================================
  // 9. Parallel Job Processing (Multiple Workers)
  // =========================================================================
  describe("Parallel Job Processing", () => {
    it("two workers claim different jobs simultaneously", async () => {
      // Get two available pending jobs for different workers
      const [job2Pda] = findJobPda(queuePda, 2);
      const [job3Pda] = findJobPda(queuePda, 3);

      const job2Data = await program.account.job.fetch(job2Pda);
      const job3Data = await program.account.job.fetch(job3Pda);

      const [bucket2Pda] = findBucketPda(queuePda, job2Data.bucketIndex);
      const [bucket3Pda] = findBucketPda(queuePda, job3Data.bucketIndex);
      const [worker1Pda] = findWorkerPda(queuePda, worker1.publicKey);
      const [worker2Pda] = findWorkerPda(queuePda, worker2.publicKey);

      // Submit both claims concurrently
      const [tx1, tx2] = await Promise.all([
        program.methods
          .claimJob()
          .accountsPartial({
            workerAuthority: worker1.publicKey,
            queue: queuePda,
            job: job2Pda,
            bucket: bucket2Pda,
            worker: worker1Pda,
          })
          .signers([worker1])
          .rpc(),
        program.methods
          .claimJob()
          .accountsPartial({
            workerAuthority: worker2.publicKey,
            queue: queuePda,
            job: job3Pda,
            bucket: bucket3Pda,
            worker: worker2Pda,
          })
          .signers([worker2])
          .rpc(),
      ]);

      const job2After = await program.account.job.fetch(job2Pda);
      const job3After = await program.account.job.fetch(job3Pda);

      expect(job2After.assignedWorker.toBase58()).to.equal(
        worker1.publicKey.toBase58()
      );
      expect(job3After.assignedWorker.toBase58()).to.equal(
        worker2.publicKey.toBase58()
      );

      console.log("    Worker1 claimed #2, Worker2 claimed #3 in parallel");

      // Complete both
      const resultHash1 = randomHash();
      const resultHash2 = randomHash();

      await Promise.all([
        program.methods
          .completeJob(resultHash1)
          .accountsPartial({
            workerAuthority: worker1.publicKey,
            queue: queuePda,
            job: job2Pda,
            worker: worker1Pda,
          })
          .signers([worker1])
          .rpc(),
        program.methods
          .completeJob(resultHash2)
          .accountsPartial({
            workerAuthority: worker2.publicKey,
            queue: queuePda,
            job: job3Pda,
            worker: worker2Pda,
          })
          .signers([worker2])
          .rpc(),
      ]);

      console.log("    Both jobs completed in parallel");
    });
  });

  // =========================================================================
  // 10. Job Timeout & Reclamation
  // =========================================================================
  describe("Job Timeout", () => {
    let timeoutQueuePda: PublicKey;
    const TIMEOUT_QUEUE_NAME = "timeout-queue";
    const SHORT_TIMEOUT = 2; // 2 seconds

    before(async () => {
      // Create a queue with very short timeout for testing
      [timeoutQueuePda] = findQueuePda(authority.publicKey, TIMEOUT_QUEUE_NAME);

      await program.methods
        .createQueue(TIMEOUT_QUEUE_NAME, 2, new anchor.BN(SHORT_TIMEOUT), 1, false)
        .accountsPartial({
          authority: authority.publicKey,
          queue: timeoutQueuePda,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      // Init single bucket
      const [bucketPda] = findBucketPda(timeoutQueuePda, 0);
      await program.methods
        .initBucket(0)
        .accountsPartial({
          authority: authority.publicKey,
          queue: timeoutQueuePda,
          bucket: bucketPda,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      // Register worker
      const [workerPda] = findWorkerPda(timeoutQueuePda, worker1.publicKey);
      await program.methods
        .registerWorker()
        .accountsPartial({
          authority: authority.publicKey,
          queue: timeoutQueuePda,
          workerAuthority: worker1.publicKey,
          worker: workerPda,
          systemProgram: SystemProgram.programId,
        })
        .rpc();
    });

    it("times out a stale job and re-queues it", async () => {
      // Submit a job
      const queue = await program.account.queue.fetch(timeoutQueuePda);
      const jobId = queue.totalJobsCreated.toNumber();
      const [jobPda] = findJobPda(timeoutQueuePda, jobId);
      const [bucketPda] = findBucketPda(timeoutQueuePda, 0);
      const [workerPda] = findWorkerPda(timeoutQueuePda, worker1.publicKey);

      await program.methods
        .submitJob(randomHash(), 0)
        .accountsPartial({
          submitter: authority.publicKey,
          queue: timeoutQueuePda,
          job: jobPda,
          bucket: bucketPda,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      // Worker claims it
      await program.methods
        .claimJob()
        .accountsPartial({
          workerAuthority: worker1.publicKey,
          queue: timeoutQueuePda,
          job: jobPda,
          bucket: bucketPda,
          worker: workerPda,
        })
        .signers([worker1])
        .rpc();

      console.log("    Job claimed, waiting for timeout...");

      // Wait for timeout
      await sleep((SHORT_TIMEOUT + 2) * 1000);

      // Anyone can crank the timeout
      await program.methods
        .timeoutJob()
        .accountsPartial({
          cranker: authority.publicKey,
          queue: timeoutQueuePda,
          job: jobPda,
          bucket: bucketPda,
        })
        .rpc();

      const jobAfter = await program.account.job.fetch(jobPda);
      expect(jobAfter.status).to.deep.equal({ pending: {} });
      expect(jobAfter.retryCount).to.equal(1);

      console.log("    Job timed out and returned to pending (retry 1/2)");
    });

    it("rejects timeout if job hasn't actually timed out", async () => {
      // Submit and claim a new job
      const queue = await program.account.queue.fetch(timeoutQueuePda);
      const jobId = queue.totalJobsCreated.toNumber();
      const [jobPda] = findJobPda(timeoutQueuePda, jobId);
      const [bucketPda] = findBucketPda(timeoutQueuePda, 0);
      const [workerPda] = findWorkerPda(timeoutQueuePda, worker1.publicKey);

      await program.methods
        .submitJob(randomHash(), 0)
        .accountsPartial({
          submitter: authority.publicKey,
          queue: timeoutQueuePda,
          job: jobPda,
          bucket: bucketPda,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      await program.methods
        .claimJob()
        .accountsPartial({
          workerAuthority: worker1.publicKey,
          queue: timeoutQueuePda,
          job: jobPda,
          bucket: bucketPda,
          worker: workerPda,
        })
        .signers([worker1])
        .rpc();

      // Try to timeout immediately (should fail)
      try {
        await program.methods
          .timeoutJob()
          .accountsPartial({
            cranker: authority.publicKey,
            queue: timeoutQueuePda,
            job: jobPda,
            bucket: bucketPda,
          })
          .rpc();
        expect.fail("Should have thrown");
      } catch (err) {
        expect(err.toString()).to.include("JobNotTimedOut");
      }
    });
  });

  // =========================================================================
  // 11. Queue Update
  // =========================================================================
  describe("Queue Configuration Update", () => {
    it("authority updates queue settings", async () => {
      await program.methods
        .updateQueue(5, new anchor.BN(120), null, null)
        .accountsPartial({
          authority: authority.publicKey,
          queue: queuePda,
        })
        .rpc();

      const queue = await program.account.queue.fetch(queuePda);
      expect(queue.maxRetries).to.equal(5);
      expect(queue.jobTimeoutSeconds.toNumber()).to.equal(120);
      console.log("    Queue updated: maxRetries=5, timeout=120s");
    });

    it("rejects update from non-authority", async () => {
      const fakeAuth = Keypair.generate();

      try {
        await program.methods
          .updateQueue(1, null, null, null)
          .accountsPartial({
            authority: fakeAuth.publicKey,
            queue: queuePda,
          })
          .signers([fakeAuth])
          .rpc();
        expect.fail("Should have thrown");
      } catch (err) {
        expect(err).to.exist;
      }
    });
  });

  // =========================================================================
  // 12. Final Statistics
  // =========================================================================
  describe("Final Statistics", () => {
    it("displays queue statistics", async () => {
      const queue = await program.account.queue.fetch(queuePda);
      console.log("\n    ═══ FINAL QUEUE STATISTICS ═══");
      console.log(`    Total Jobs Created:  ${queue.totalJobsCreated.toNumber()}`);
      console.log(`    Pending:             ${queue.pendingCount.toNumber()}`);
      console.log(`    Active:              ${queue.activeCount.toNumber()}`);
      console.log(`    Completed:           ${queue.completedCount.toNumber()}`);
      console.log(`    Failed:              ${queue.failedCount.toNumber()}`);
      console.log("    ═══════════════════════════════");
    });
  });
});
