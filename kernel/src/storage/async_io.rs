//! # Asynchronous Block I/O Ring Engine (Phase 15 - M35)
//!
//! ## Overview
//! Provides an `io_uring`-inspired asynchronous block I/O submission and completion
//! engine decoupled from caller execution threads. Applications and kernel subsystems
//! submit read, write, and flush descriptors into a Submission Ring, receiving a unique
//! `AsyncIoId`. Requests are ordered and executed in batches, publishing results and
//! byte buffers into a lock-protected Completion Ring for non-blocking polling or waiting.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::arch::x86_64::apic;
use crate::drivers::block::{BlockDevice, BlockError};
use crate::storage::io_scheduler::{IoRequestOp, IoScheduler, IoSchedulingPolicy};
use crate::sync::Spinlock;

/// Unique identifier for an asynchronous I/O operation.
pub type AsyncIoId = u64;

/// Type of asynchronous block I/O operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncIoOp {
    Read,
    Write,
    Flush,
}

/// Submission descriptor placed into the Asynchronous I/O Submission Queue.
pub struct AsyncIoSubmission {
    pub id: AsyncIoId,
    pub user_data: u64,
    pub op: AsyncIoOp,
    pub device: Arc<dyn BlockDevice>,
    pub sector: u64,
    pub sector_count: usize,
    pub write_data: Vec<u8>,
}

/// Completion entry published into the Completion Queue upon request completion.
#[derive(Debug, Clone)]
pub struct AsyncIoCompletion {
    pub id: AsyncIoId,
    pub user_data: u64,
    pub op: AsyncIoOp,
    pub sector: u64,
    pub bytes_transferred: usize,
    pub result: Result<Vec<u8>, BlockError>,
}

/// Telemetry counters for the Asynchronous Block I/O Engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct AsyncIoStats {
    pub pending_submissions: usize,
    pub pending_completions: usize,
    pub total_submitted: u64,
    pub total_completed: u64,
    pub total_errors: u64,
    pub total_bytes_transferred: u64,
}

struct AsyncIoInner {
    submissions: VecDeque<AsyncIoSubmission>,
    completions: VecDeque<AsyncIoCompletion>,
    next_id: u64,
    total_submitted: u64,
    total_completed: u64,
    total_errors: u64,
    total_bytes_transferred: u64,
}

/// Thread-safe Asynchronous Block I/O Ring Engine.
pub struct AsyncIoEngine {
    inner: Spinlock<AsyncIoInner>,
    scheduler: IoScheduler,
}

impl AsyncIoEngine {
    /// Creates a new Asynchronous Block I/O Engine backed by an Elevator SCAN I/O scheduler.
    pub const fn new() -> Self {
        Self {
            inner: Spinlock::new(AsyncIoInner {
                submissions: VecDeque::new(),
                completions: VecDeque::new(),
                next_id: 1,
                total_submitted: 0,
                total_completed: 0,
                total_errors: 0,
                total_bytes_transferred: 0,
            }),
            scheduler: IoScheduler::new(IoSchedulingPolicy::ElevatorScan),
        }
    }

    /// Submits an asynchronous read request for `sector_count` contiguous sectors.
    pub fn submit_read(
        &self,
        device: Arc<dyn BlockDevice>,
        sector: u64,
        sector_count: usize,
        user_data: u64,
    ) -> AsyncIoId {
        self.submit_raw(device, AsyncIoOp::Read, sector, sector_count, Vec::new(), user_data)
    }

    /// Submits an asynchronous write request for the provided sector buffer.
    pub fn submit_write(
        &self,
        device: Arc<dyn BlockDevice>,
        sector: u64,
        data: Vec<u8>,
        user_data: u64,
    ) -> AsyncIoId {
        let sec_size = device.sector_size();
        let count = core::cmp::max(1, (data.len() + sec_size - 1) / sec_size);
        self.submit_raw(device, AsyncIoOp::Write, sector, count, data, user_data)
    }

    /// Submits an asynchronous cache flush command for `device`.
    pub fn submit_flush(&self, device: Arc<dyn BlockDevice>, user_data: u64) -> AsyncIoId {
        self.submit_raw(device, AsyncIoOp::Flush, 0, 0, Vec::new(), user_data)
    }

    fn submit_raw(
        &self,
        device: Arc<dyn BlockDevice>,
        op: AsyncIoOp,
        sector: u64,
        sector_count: usize,
        write_data: Vec<u8>,
        user_data: u64,
    ) -> AsyncIoId {
        let mut guard = self.inner.lock();
        let id = guard.next_id;
        guard.next_id += 1;
        guard.total_submitted += 1;

        let sched_op = match op {
            AsyncIoOp::Read => IoRequestOp::Read,
            AsyncIoOp::Write => IoRequestOp::Write,
            AsyncIoOp::Flush => IoRequestOp::Flush,
        };
        let tick = apic::get_ticks();
        let _ = self.scheduler.enqueue(sched_op, sector, sector_count, Vec::new(), tick);

        guard.submissions.push_back(AsyncIoSubmission {
            id,
            user_data,
            op,
            device,
            sector,
            sector_count,
            write_data,
        });
        id
    }

    /// Processes up to `max_batch` queued I/O submissions and moves results into the Completion Ring.
    ///
    /// Returns the number of requests completed during this pass.
    pub fn process_batch(&self, max_batch: usize) -> usize {
        let mut processed = 0usize;
        let limit = core::cmp::max(1, max_batch);

        for _ in 0..limit {
            let sub = {
                let mut guard = self.inner.lock();
                guard.submissions.pop_front()
            };

            let sub = match sub {
                Some(s) => s,
                None => break,
            };

            // Advance scheduler head state
            let _ = self.scheduler.dequeue_next(apic::get_ticks());

            let sec_size = sub.device.sector_size();
            let mut bytes_transferred = 0usize;
            let result = match sub.op {
                AsyncIoOp::Read => {
                    let total_bytes = sub.sector_count * sec_size;
                    let mut buf = vec![0u8; total_bytes];
                    match sub.device.read_sectors(sub.sector, sub.sector_count, &mut buf) {
                        Ok(()) => {
                            bytes_transferred = total_bytes;
                            Ok(buf)
                        }
                        Err(e) => Err(e),
                    }
                }
                AsyncIoOp::Write => {
                    let expected_bytes = sub.sector_count * sec_size;
                    let mut padded = sub.write_data;
                    if padded.len() < expected_bytes {
                        padded.resize(expected_bytes, 0);
                    }
                    match sub.device.write_sectors(sub.sector, sub.sector_count, &padded) {
                        Ok(()) => {
                            bytes_transferred = expected_bytes;
                            Ok(Vec::new())
                        }
                        Err(e) => Err(e),
                    }
                }
                AsyncIoOp::Flush => match sub.device.flush() {
                    Ok(()) => Ok(Vec::new()),
                    Err(e) => Err(e),
                },
            };

            let mut guard = self.inner.lock();
            guard.total_completed += 1;
            if result.is_err() {
                guard.total_errors += 1;
            } else {
                guard.total_bytes_transferred += bytes_transferred as u64;
            }
            guard.completions.push_back(AsyncIoCompletion {
                id: sub.id,
                user_data: sub.user_data,
                op: sub.op,
                sector: sub.sector,
                bytes_transferred,
                result,
            });
            processed += 1;
        }

        processed
    }

    /// Pops the next completed I/O entry from the Completion Ring, if available.
    pub fn pop_completion(&self) -> Option<AsyncIoCompletion> {
        self.inner.lock().completions.pop_front()
    }

    /// Drives the engine until the completion matching `target_id` is ready and returns it.
    pub fn wait_for(&self, target_id: AsyncIoId) -> Option<AsyncIoCompletion> {
        loop {
            // Check if already present in completion queue
            {
                let mut guard = self.inner.lock();
                if let Some(pos) = guard.completions.iter().position(|c| c.id == target_id) {
                    return guard.completions.remove(pos);
                }
                if guard.submissions.is_empty() {
                    return None;
                }
            }
            self.process_batch(8);
        }
    }

    /// Returns telemetry counters for the Asynchronous Block I/O Engine.
    pub fn stats(&self) -> AsyncIoStats {
        let guard = self.inner.lock();
        AsyncIoStats {
            pending_submissions: guard.submissions.len(),
            pending_completions: guard.completions.len(),
            total_submitted: guard.total_submitted,
            total_completed: guard.total_completed,
            total_errors: guard.total_errors,
            total_bytes_transferred: guard.total_bytes_transferred,
        }
    }
}

/// Global Asynchronous Block I/O Ring Engine instance.
pub static GLOBAL_ASYNC_IO: AsyncIoEngine = AsyncIoEngine::new();
