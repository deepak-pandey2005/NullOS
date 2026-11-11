//! # Network Performance Benchmarks (`netbench`) (Phase 16 - M36)
//!
//! ## Overview
//! Cycle-accurate hardware timestamp counter (`RDTSC`) benchmarks comparing legacy
//! per-packet heap allocation & copying against the M36 high-performance networking path:
//! 1. **Buffer Pool (`GLOBAL_NET_POOL`) vs Heap `Vec<u8>`**: Pre-allocated cache-aligned slabs.
//! 2. **Zero-Copy Headroom Framing vs Buffer Reallocation**: `build_udp_ipv4_zerocopy` in-place framing.
//! 3. **Packet Batching vs Unbatched Loop**: `PacketBatch` dispatch across 32-frame bursts.
//! 4. **RSS Toeplitz Hashing Uniformity**: Distribution speed and bucket spread over 4 queues.
//! 5. **TCP Optimization Control Block**: Congestion-window & RTT state transition throughput.

use alloc::vec;
use core::hint::black_box;

use crate::drivers::virtio_net::MacAddress;
use crate::net::ipv4::Ipv4Addr;
use crate::net::perf::batch::{build_udp_ipv4_zerocopy, dispatch_tx_batch, PacketBatch, MAX_BATCH_SIZE};
use crate::net::perf::pool::GLOBAL_NET_POOL;
use crate::net::perf::rss::{rss_hash_ipv4_4tuple, GLOBAL_MULTIQUEUE};
use crate::net::perf::tcp_opt::{TcpOptConfig, TcpOptControlBlock};
use crate::println;

/// Reads the x86_64 64-bit Time Stamp Counter (`RDTSC`) directly into a `u64`.
#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC is an unprivileged x86_64 cycle counter read instruction with no side effects.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Runs the full M36 cycle-accurate network performance benchmark suite (`netbench`).
pub fn run_benchmarks() {
    println!("=== NullOS High-Performance Networking Benchmarks (M36) ===");

    // Benchmark 1: Heap Vec alloc/free vs O(1) GLOBAL_NET_POOL slab alloc/free
    let iterations = 512u64;
    let t0 = read_tsc();
    for _ in 0..iterations {
        let mut v = vec![0u8; 1536];
        v[0] = black_box(0x45);
        black_box(&v);
    }
    let heap_cycles = read_tsc().saturating_sub(t0);

    let t1 = read_tsc();
    for _ in 0..iterations {
        if let Some(mut buf) = GLOBAL_NET_POOL.alloc() {
            buf.storage[64] = black_box(0x45);
            black_box(&buf.storage);
            GLOBAL_NET_POOL.free(buf);
        }
    }
    let pool_cycles = read_tsc().saturating_sub(t1);
    println!(
        "  [1] Buffer Alloc (512 x 2KB) : Heap={} cyc ({} cyc/op) | NetBufPool={} cyc ({} cyc/op)",
        heap_cycles,
        heap_cycles / iterations,
        pool_cycles,
        pool_cycles / iterations
    );

    // Benchmark 2: Zero-Copy Headroom Packet Framing vs Copy-based framing
    let payload = [0xABu8; 256];
    let src_mac = MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let dst_mac = MacAddress([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    let src_ip = Ipv4Addr([10, 0, 2, 15]);
    let dst_ip = Ipv4Addr([10, 0, 2, 2]);

    let frame_iters = 256u64;
    let t2 = read_tsc();
    for i in 0..frame_iters {
        if let Ok(pkt) = build_udp_ipv4_zerocopy(
            src_mac,
            dst_mac,
            src_ip,
            dst_ip,
            50000 + (i as u16),
            8080,
            &payload,
        ) {
            black_box(pkt.data());
            GLOBAL_NET_POOL.free(pkt);
        }
    }
    let zc_cycles = read_tsc().saturating_sub(t2);
    println!(
        "  [2] Zero-Copy L2+L3+L4 Frame : {} frames in {} cyc ({} cyc/pkt)",
        frame_iters,
        zc_cycles,
        zc_cycles / frame_iters
    );

    // Benchmark 3: Packet Batching (32-frame batch dispatch + pool recycle)
    let mut batch = PacketBatch::new();
    for i in 0..MAX_BATCH_SIZE {
        if let Ok(pkt) = build_udp_ipv4_zerocopy(
            src_mac,
            dst_mac,
            src_ip,
            dst_ip,
            40000 + (i as u16),
            9000,
            &payload,
        ) {
            let _ = batch.push(pkt);
        }
    }
    let t3 = read_tsc();
    let dispatched = dispatch_tx_batch(&mut batch, |frame| {
        black_box(frame.len());
        Ok(())
    });
    let batch_cycles = read_tsc().saturating_sub(t3);
    println!(
        "  [3] Packet Batching TX       : {} pkts in {} cyc ({} cyc/pkt)",
        dispatched,
        batch_cycles,
        batch_cycles / core::cmp::max(1, dispatched as u64)
    );

    // Benchmark 4: RSS Toeplitz 4-Tuple Flow Classification across queues
    let flows = 1000u64;
    let active_q = GLOBAL_MULTIQUEUE.active_queues();
    let mut dist = [0u32; 8];
    let t4 = read_tsc();
    for i in 0..flows {
        let h = rss_hash_ipv4_4tuple(
            Ipv4Addr([192, 168, 1, ((i & 0xFF) as u8)]),
            dst_ip,
            1024 + ((i * 7) as u16),
            443,
        );
        let q = GLOBAL_MULTIQUEUE.select_queue(h);
        if q < dist.len() {
            dist[q] += 1;
        }
    }
    let rss_cycles = read_tsc().saturating_sub(t4);
    println!(
        "  [4] RSS Toeplitz (1000 flows): {} cyc ({} cyc/hash) | Q distribution={:?}",
        rss_cycles,
        rss_cycles / flows,
        &dist[..active_q]
    );

    // Benchmark 5: TCP Optimization Control Block ACK & RTT updates
    let mut tcb = TcpOptControlBlock::new(TcpOptConfig::default(), 1460);
    let t5 = read_tsc();
    for i in 1..=1000u32 {
        tcb.update_rtt(15 + (i % 10));
        let _ = tcb.on_ack(i * 1460, 1460);
    }
    let tcp_cycles = read_tsc().saturating_sub(t5);
    println!(
        "  [5] TCP Opt (RTT+CWND x1000) : {} cyc ({} cyc/ack) | final cwnd={}B rto={}ms",
        tcp_cycles,
        tcp_cycles / 1000,
        tcb.cwnd,
        tcb.rto_ms
    );

    println!("=== Benchmarks Complete ===");
}

