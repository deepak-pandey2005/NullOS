//! # Storage Subsystem Benchmarking Suite (Phase 15 - M35)
//!
//! ## Overview
//! Measures I/O throughput, CPU cycle latency (`rdtsc`), cache hit speedups, and
//! scheduler performance across raw hardware block devices (VirtIO-blk, NVMe)
//! and caching layers (`CachedBlockDevice`, `PageCache`).

use alloc::sync::Arc;
use alloc::vec;

use crate::drivers::block::{self, BlockDevice, BlockError, SECTOR_SIZE};
use crate::println;
use crate::storage::block_cache::{CachePolicy, CachedBlockDevice};
use crate::sync::Spinlock;

/// In-memory synthetic block device used for deterministic baseline benchmarks and testing
/// when running without dedicated scratch disks.
pub struct RamBlockDevice {
    name: &'static str,
    sector_count: u64,
    data: Spinlock<alloc::vec::Vec<u8>>,
}

impl RamBlockDevice {
    /// Creates a zero-initialized in-memory block device with `sector_count` 512-byte sectors.
    pub fn new(name: &'static str, sector_count: u64) -> Self {
        let total = (sector_count as usize) * SECTOR_SIZE;
        Self {
            name,
            sector_count,
            data: Spinlock::new(vec![0u8; total]),
        }
    }
}

impl BlockDevice for RamBlockDevice {
    fn name(&self) -> &str {
        self.name
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }

    fn read_sector(&self, sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if sector >= self.sector_count {
            return Err(BlockError::InvalidSector);
        }
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::BufferTooSmall);
        }
        let guard = self.data.lock();
        let off = (sector as usize) * SECTOR_SIZE;
        buf[..SECTOR_SIZE].copy_from_slice(&guard[off..off + SECTOR_SIZE]);
        Ok(())
    }

    fn write_sector(&self, sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        if sector >= self.sector_count {
            return Err(BlockError::InvalidSector);
        }
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::BufferTooSmall);
        }
        let mut guard = self.data.lock();
        let off = (sector as usize) * SECTOR_SIZE;
        guard[off..off + SECTOR_SIZE].copy_from_slice(&buf[..SECTOR_SIZE]);
        Ok(())
    }
}

/// Reads the 64-bit x86 Time-Stamp Counter (`rdtsc`).
#[inline]
pub fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Result metrics captured for a single storage benchmark phase.
#[derive(Debug, Clone, Copy)]
pub struct BenchResult {
    pub ops: u64,
    pub total_bytes: u64,
    pub total_cycles: u64,
    pub cycles_per_op: u64,
}

impl BenchResult {
    fn compute(ops: u64, bytes: u64, cycles: u64) -> Self {
        let cycles_per_op = if ops > 0 { cycles / ops } else { 0 };
        Self {
            ops,
            total_bytes: bytes,
            total_cycles: cycles,
            cycles_per_op,
        }
    }
}

/// Runs sequential read benchmark on `dev`.
pub fn bench_seq_read(dev: &dyn BlockDevice, num_sectors: u64) -> BenchResult {
    let count = core::cmp::min(num_sectors, dev.sector_count());
    let mut buf = [0u8; SECTOR_SIZE];
    let start = read_tsc();
    for s in 0..count {
        let _ = dev.read_sector(s, &mut buf);
    }
    let end = read_tsc();
    BenchResult::compute(count, count * SECTOR_SIZE as u64, end.saturating_sub(start))
}

/// Runs pseudo-random read benchmark on `dev` using a deterministic LCG PRNG.
pub fn bench_rand_read(dev: &dyn BlockDevice, iterations: u64) -> BenchResult {
    let max_sec = core::cmp::max(1, dev.sector_count());
    let mut buf = [0u8; SECTOR_SIZE];
    let mut rng = 0x1234_5678_9ABC_DEF0u64;

    let start = read_tsc();
    for _ in 0..iterations {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let lba = rng % max_sec;
        let _ = dev.read_sector(lba, &mut buf);
    }
    let end = read_tsc();
    BenchResult::compute(
        iterations,
        iterations * SECTOR_SIZE as u64,
        end.saturating_sub(start),
    )
}

/// Runs sequential write-back benchmark on a cached device (flushing at the end).
pub fn bench_cached_write_back(dev: &CachedBlockDevice, num_sectors: u64) -> BenchResult {
    let count = core::cmp::min(num_sectors, dev.sector_count());
    let buf = [0x5Au8; SECTOR_SIZE];
    let start = read_tsc();
    for s in 0..count {
        let _ = dev.write_sector(s, &buf);
    }
    let _ = dev.flush();
    let end = read_tsc();
    BenchResult::compute(count, count * SECTOR_SIZE as u64, end.saturating_sub(start))
}

/// Executes the full storage benchmark suite across detected block devices and cache layers.
pub fn run_benchmarks() {
    println!();
    println!("================================================================");
    println!("        NullOS Storage Subsystem Benchmark Suite (M35)          ");
    println!("================================================================");

    let devices = block::all_devices();
    for dev in devices.iter() {
        let seq_res = bench_seq_read(dev.as_ref(), 64);
        let rand_res = bench_rand_read(dev.as_ref(), 64);

        println!(
            "[BENCH] Device '{}' ({} sectors):",
            dev.name(),
            dev.sector_count()
        );
        println!(
            "  Raw Seq Read  (64x 512B = {} KiB): {} cycles total ({} cycles/op)",
            seq_res.total_bytes / 1024,
            seq_res.total_cycles,
            seq_res.cycles_per_op
        );
        println!(
            "  Raw Rand Read (64x 512B = {} KiB): {} cycles total ({} cycles/op)",
            rand_res.total_bytes / 1024,
            rand_res.total_cycles,
            rand_res.cycles_per_op
        );

        // Layer CachedBlockDevice over hardware device to measure hot-cache read speedup
        let cached = CachedBlockDevice::new(dev.clone(), 128, CachePolicy::WriteThrough);
        // Warm up 64 sectors
        let _ = bench_seq_read(&cached, 64);
        // Measure hot reads
        let hot_res = bench_seq_read(&cached, 64);
        let stats = cached.stats();
        let speedup = if hot_res.cycles_per_op > 0 {
            seq_res.cycles_per_op / core::cmp::max(1, hot_res.cycles_per_op)
        } else {
            1
        };
        println!(
            "  Cached Seq Read (Warm Hit): {} cycles total ({} cycles/op, ~{}x speedup, hits={})",
            hot_res.total_cycles,
            hot_res.cycles_per_op,
            speedup,
            stats.hits
        );
    }

    // Also run Write-Back vs Write-Through benchmark on a dedicated 256-sector RAM disk
    let ram_disk = Arc::new(RamBlockDevice::new("ram-bench0", 256));
    let wt_cache = CachedBlockDevice::new(ram_disk.clone(), 128, CachePolicy::WriteThrough);
    let wb_cache = CachedBlockDevice::new(ram_disk, 128, CachePolicy::WriteBack);

    let wt_res = bench_cached_write_back(&wt_cache, 64);
    let wb_res = bench_cached_write_back(&wb_cache, 64);

    println!("[BENCH] Write Policy Comparison (64 sectors = 32 KiB):");
    println!(
        "  Write-Through: {} cycles/op | Write-Back + Flush: {} cycles/op",
        wt_res.cycles_per_op, wb_res.cycles_per_op
    );
    println!("================================================================");
}

