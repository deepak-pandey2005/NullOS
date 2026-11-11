//! # Advanced Storage Subsystem (Phase 15 - M35)
//!
//! ## Architecture
//! Unifies high-performance block caching, file page caching, I/O request scheduling,
//! asynchronous submission/completion rings, crash-consistent Write-Ahead Logging (WAL),
//! and cycle-accurate storage benchmarking:
//! - [`block_cache`]: Sector-level LRU cache with Write-Through and Write-Back policies.
//! - [`page_cache`]: 4 KiB VFS file page cache indexed by `(inode_id, page_index)`.
//! - [`io_scheduler`]: FIFO, C-LOOK Elevator SCAN, and Deadline I/O request scheduler with merging.
//! - [`async_io`]: `io_uring`-inspired asynchronous Submission/Completion Ring engine.
//! - [`journal`]: Transactional Write-Ahead Log with CRC32 integrity checks and crash recovery.
//! - [`bench`]: Cycle-accurate storage throughput and latency benchmarking suite.

#![allow(dead_code)]

pub mod async_io;
pub mod bench;
pub mod block_cache;
pub mod io_scheduler;
pub mod journal;
pub mod page_cache;

use alloc::sync::Arc;
use alloc::vec;

use crate::drivers::block::{self, BlockDevice, SECTOR_SIZE};
use crate::memory::PAGE_SIZE_4K;
use crate::println;

use self::bench::RamBlockDevice;
use self::block_cache::{CachePolicy, CachedBlockDevice, PRIMARY_BLOCK_CACHE};
use self::io_scheduler::{IoRequestOp, IoScheduler, IoSchedulingPolicy, GLOBAL_IO_SCHEDULER};
use self::journal::{Journal, GLOBAL_JOURNAL};
use self::page_cache::GLOBAL_PAGE_CACHE;

/// Initializes the Advanced Storage Subsystem, layering a Write-Back LRU Block Cache
/// over the primary registered block device (if present).
pub fn init() {
    println!();
    println!("----------------------------------------");
    println!(" [STORAGE] Initializing Advanced Storage");
    println!("----------------------------------------");

    if let Some(primary) = block::primary_device() {
        let cached = Arc::new(CachedBlockDevice::new(
            primary.clone(),
            256,
            CachePolicy::WriteBack,
        ));
        *PRIMARY_BLOCK_CACHE.lock() = Some(cached);
        println!(
            "[STORAGE] Layering 256-sector Write-Back Block Cache over '{}'.",
            primary.name()
        );
    } else {
        println!("[STORAGE] No primary block device detected; storage caches active in standby.");
    }

    println!(
        "[STORAGE] Page Cache active (256 x 4 KiB pages = 1 MiB). I/O Scheduler: Deadline + Elevator SCAN."
    );
    println!("[STORAGE] Asynchronous Block I/O Ring & WAL Journal initialized.");
}

/// Prints a comprehensive diagnostic summary of all storage caches, queues, and journals.
pub fn print_status() {
    println!("=== NullOS Advanced Storage Subsystem Status (M35) ===");

    let devs = block::all_devices();
    println!("Registered Block Devices ({}):", devs.len());
    for d in devs {
        let mib = d.capacity_bytes() / (1024 * 1024);
        println!(
            "  - {:10} : {} sectors x {} bytes ({} MiB)",
            d.name(),
            d.sector_count(),
            d.sector_size(),
            mib
        );
    }

    if let Some(ref cache) = *PRIMARY_BLOCK_CACHE.lock() {
        let st = cache.stats();
        println!(
            "Primary Block Cache ({}): policy={:?}, used={}/{}, dirty={}, hits={}, misses={}, writebacks={}, evictions={}",
            cache.name(),
            cache.policy(),
            st.used_blocks,
            st.capacity_blocks,
            st.dirty_blocks,
            st.hits,
            st.misses,
            st.writebacks,
            st.evictions
        );
    }

    let pst = GLOBAL_PAGE_CACHE.stats();
    println!(
        "VFS Page Cache: cached={}/{} pages, dirty={}, hits={}, misses={}, writebacks={}, evictions={}",
        pst.cached_pages,
        pst.capacity_pages,
        pst.dirty_pages,
        pst.hits,
        pst.misses,
        pst.writebacks,
        pst.evictions
    );

    let ist = GLOBAL_IO_SCHEDULER.stats();
    println!(
        "I/O Scheduler: policy={:?}, queued={}, enqueued={}, dispatched={}, merged={}, deadline_overrides={}",
        GLOBAL_IO_SCHEDULER.policy(),
        ist.queue_depth,
        ist.total_enqueued,
        ist.total_dispatched,
        ist.total_merged,
        ist.deadline_overrides
    );

    let ast = async_io::GLOBAL_ASYNC_IO.stats();
    println!(
        "Async I/O Ring: sub_pending={}, cqe_pending={}, submitted={}, completed={}, bytes={}",
        ast.pending_submissions,
        ast.pending_completions,
        ast.total_submitted,
        ast.total_completed,
        ast.total_bytes_transferred
    );

    let jst = GLOBAL_JOURNAL.stats();
    println!(
        "WAL Journal: next_tx={}, active={}, pending_ckpt={}, committed={}, aborted={}, checkpointed={}, replayed_blocks={}",
        jst.next_tx_id,
        jst.active_txs,
        jst.committed_pending_checkpoint,
        jst.total_committed,
        jst.total_aborted,
        jst.total_checkpointed,
        jst.total_replayed_blocks
    );
}

/// Runs comprehensive self-tests validating all 8 M35 Storage Subsystem capabilities.
pub fn run_self_tests() {
    println!();
    println!("--------------------------------------------------");
    println!(" [STORAGE] Running Milestone 35 Storage Tests     ");
    println!("--------------------------------------------------");

    // 1. Test Block Cache + Write-Back Cache + LRU Eviction
    println!("[STORAGE] Test 1: Verifying Block Cache, Write-Back & LRU Eviction...");
    {
        let raw = Arc::new(RamBlockDevice::new("ram-test0", 64));
        let cache = CachedBlockDevice::new(raw.clone(), 4, CachePolicy::WriteBack);

        let pattern_a = [0xAAu8; SECTOR_SIZE];
        let pattern_b = [0xBBu8; SECTOR_SIZE];
        let mut read_buf = [0u8; SECTOR_SIZE];

        // Write sector 1 in Write-Back mode -> raw device must NOT yet see 0xAA
        assert!(cache.write_sector(1, &pattern_a).is_ok());
        assert!(raw.read_sector(1, &mut read_buf).is_ok());
        assert_eq!(read_buf[0], 0x00, "Write-Back leaked early to backing disk!");

        // Read from cache -> hit!
        assert!(cache.read_sector(1, &mut read_buf).is_ok());
        assert_eq!(read_buf[0], 0xAA);
        assert_eq!(cache.stats().dirty_blocks, 1);

        // Flush -> backing disk now updated & dirty_blocks == 0
        assert!(cache.flush().is_ok());
        assert_eq!(cache.stats().dirty_blocks, 0);
        assert!(raw.read_sector(1, &mut read_buf).is_ok());
        assert_eq!(read_buf[0], 0xAA);

        // Overflow 4-entry cache with 6 distinct dirty writes to test automatic dirty victim writeback
        for s in 10..16u64 {
            assert!(cache.write_sector(s, &pattern_b).is_ok());
        }
        let st = cache.stats();
        assert!(st.evictions >= 2, "Expected LRU evictions when overflowing cache");
        assert!(st.writebacks >= 2, "Expected dirty victim writebacks on eviction");
        assert!(cache.flush().is_ok());
        assert!(raw.read_sector(10, &mut read_buf).is_ok());
        assert_eq!(read_buf[0], 0xBB);
    }

    // 2. Test VFS 4 KiB Page Cache
    println!("[STORAGE] Test 2: Verifying 4 KiB VFS Page Cache...");
    {
        let mut backing_page = vec![0u8; PAGE_SIZE_4K as usize];
        let payload = b"NULLOS_PAGE_CACHE_V1";
        let written = GLOBAL_PAGE_CACHE
            .write_page(
                9001,
                0,
                16,
                payload,
                |_ino, _idx, _buf| Ok(()),
                |_ino, _idx, _data| Ok(()),
            )
            .expect("PageCache write failed");
        assert_eq!(written, payload.len());
        assert!(GLOBAL_PAGE_CACHE.stats().dirty_pages >= 1);

        let mut out = vec![0u8; PAGE_SIZE_4K as usize];
        assert!(GLOBAL_PAGE_CACHE
            .read_page(9001, 0, &mut out, |_ino, _idx, _buf| Ok(()))
            .is_ok());
        assert_eq!(&out[16..16 + payload.len()], payload);

        let flushed = GLOBAL_PAGE_CACHE
            .sync_inode(9001, |_ino, _idx, data| {
                backing_page.copy_from_slice(data);
                Ok(())
            })
            .expect("PageCache sync failed");
        assert_eq!(flushed, 1);
        assert_eq!(&backing_page[16..16 + payload.len()], payload);
        GLOBAL_PAGE_CACHE.invalidate_inode(9001);
    }

    // 3. Test I/O Scheduler (Elevator SCAN, Coalescing & Deadline Anti-Starvation)
    println!("[STORAGE] Test 3: Verifying I/O Scheduler (Elevator SCAN, Merging & Deadline)...");
    {
        let sched = IoScheduler::new(IoSchedulingPolicy::ElevatorScan);
        for &sec in &[50u64, 10, 30, 20, 40] {
            sched.enqueue(IoRequestOp::Read, sec, 1, vec![], 0);
        }
        let ordered = sched.drain_scheduled(0);
        let sectors: alloc::vec::Vec<u64> = ordered.iter().map(|r| r.sector).collect();
        assert_eq!(sectors, alloc::vec![10, 20, 30, 40, 50]);

        // Test contiguous sector coalescing
        sched.enqueue(IoRequestOp::Write, 100, 1, vec![1u8; 512], 0);
        sched.enqueue(IoRequestOp::Write, 101, 1, vec![2u8; 512], 0);
        assert_eq!(sched.pending_count(), 1);
        assert_eq!(sched.stats().total_merged, 1);
        let merged = sched.dequeue_next(0).unwrap();
        assert_eq!(merged.sector_count, 2);
        assert_eq!(merged.data.len(), 1024);

        // Test Deadline expiration overrides Elevator head order
        sched.set_policy(IoSchedulingPolicy::Deadline);
        sched.enqueue(IoRequestOp::Read, 200, 1, vec![], 0); // expires at tick 50
        sched.enqueue(IoRequestOp::Write, 105, 1, vec![0u8; 512], 45); // expires at tick 245, closer to head=102
        let first = sched.dequeue_next(60).unwrap(); // tick 60 >= 50 -> sector 200 expired!
        assert_eq!(first.sector, 200);
        assert_eq!(sched.stats().deadline_overrides, 1);
    }

    // 4. Test Asynchronous Block I/O Submission/Completion Ring
    println!("[STORAGE] Test 4: Verifying Asynchronous Block I/O Engine...");
    {
        let ram = Arc::new(RamBlockDevice::new("ram-async", 32));
        let mut pattern = vec![0u8; SECTOR_SIZE];
        pattern[..13].copy_from_slice(b"ASYNC_RING_OK");

        let w_id = async_io::GLOBAL_ASYNC_IO.submit_write(ram.clone(), 4, pattern.clone(), 0x1111);
        let w_cqe = async_io::GLOBAL_ASYNC_IO.wait_for(w_id).expect("Async write completion missing");
        assert!(w_cqe.result.is_ok());
        assert_eq!(w_cqe.user_data, 0x1111);

        let f_id = async_io::GLOBAL_ASYNC_IO.submit_flush(ram.clone(), 0x2222);
        let f_cqe = async_io::GLOBAL_ASYNC_IO.wait_for(f_id).expect("Async flush completion missing");
        assert!(f_cqe.result.is_ok());

        let r_id = async_io::GLOBAL_ASYNC_IO.submit_read(ram.clone(), 4, 1, 0x3333);
        let r_cqe = async_io::GLOBAL_ASYNC_IO.wait_for(r_id).expect("Async read completion missing");
        let data = r_cqe.result.expect("Async read error");
        assert_eq!(&data[..13], b"ASYNC_RING_OK");
    }

    // 5. Test Write-Ahead Journaling & Crash Recovery
    println!("[STORAGE] Test 5: Verifying Transactional WAL Journaling & Crash Recovery...");
    {
        let disk = RamBlockDevice::new("ram-wal", 32);
        let journal = Journal::new();

        // Normal commit + checkpoint
        let tx1 = journal.begin_tx();
        let mut sec5 = [0u8; SECTOR_SIZE];
        sec5[..9].copy_from_slice(b"WAL_CKPT!");
        assert!(journal.log_write(tx1, 5, &sec5).is_ok());
        assert!(journal.commit_tx(tx1).is_ok());
        assert_eq!(journal.checkpoint(&disk).unwrap(), 1);

        let mut verify = [0u8; SECTOR_SIZE];
        disk.read_sector(5, &mut verify).unwrap();
        assert_eq!(&verify[..9], b"WAL_CKPT!");

        // Simulate crash before checkpoint:
        // tx2 = Committed (must replay), tx3 = Active uncommitted (must discard), tx4 = Corrupted CRC (must reject)
        let tx2 = journal.begin_tx();
        let mut sec7 = [0u8; SECTOR_SIZE];
        sec7[..10].copy_from_slice(b"REPLAY_OK!");
        journal.log_write(tx2, 7, &sec7).unwrap();
        journal.commit_tx(tx2).unwrap();

        let tx3 = journal.begin_tx();
        let sec8 = [0x99u8; SECTOR_SIZE];
        journal.log_write(tx3, 8, &sec8).unwrap(); // intentionally NOT committed

        let tx4 = journal.begin_tx();
        let sec9 = [0x77u8; SECTOR_SIZE];
        journal.log_write(tx4, 9, &sec9).unwrap();
        journal.corrupt_last_entry_for_test(tx4);
        journal.commit_tx(tx4).unwrap();

        let report = journal.recover(&disk).unwrap();
        assert_eq!(report.committed_replayed, 1);
        assert_eq!(report.blocks_replayed, 1);
        assert_eq!(report.uncommitted_discarded, 1);
        assert_eq!(report.corrupted_rejected, 1);

        disk.read_sector(7, &mut verify).unwrap();
        assert_eq!(&verify[..10], b"REPLAY_OK!");
        disk.read_sector(8, &mut verify).unwrap();
        assert_eq!(verify[0], 0x00, "Uncommitted tx leaked into disk!");
        disk.read_sector(9, &mut verify).unwrap();
        assert_eq!(verify[0], 0x00, "Corrupted CRC tx leaked into disk!");
    }

    // 6. Run NVMe Driver Hardware Self-Tests (if NVMe controller attached)
    println!("[STORAGE] Test 6: Verifying NVMe Hardware Controller & DMA...");
    crate::drivers::nvme::run_self_tests();

    // 7. Run Storage Benchmark Suite
    println!("[STORAGE] Test 7: Running Storage Throughput & Cache Speedup Benchmarks...");
    bench::run_benchmarks();

    println!("[STORAGE] All Milestone 35 Advanced Storage self-tests passed successfully!");
}
