//! # High-Performance Networking Subsystem (Phase 16 - M36)
//!
//! ## Overview
//! Coordinates all Phase 16 Milestone 36 high-throughput / low-latency networking components:
//! - [`pool`]: Cache-aligned O(1) pre-allocated 2 KiB [`NetBufPool`] (`GLOBAL_NET_POOL`).
//! - [`rss`]: Microsoft Toeplitz RSS hashing, 128-entry indirection table & 8-queue SMP steering.
//! - [`batch`]: Zero-copy headroom framing (`build_udp_ipv4_zerocopy`) & batched dispatch (`PacketBatch`).
//! - [`polling`]: NAPI-style budgeted adaptive RX polling (`GLOBAL_NAPI_POLLER`).
//! - [`tcp_opt`]: Nagle, Delayed ACK, RFC 7323 Window Scaling, RFC 2018 SACK, RFC 6298 RTT/RTO & Reno CC.
//! - [`bench`]: Cycle-accurate `RDTSC` comparative performance benchmarks (`netbench`).

#![allow(dead_code)]

pub mod batch;
pub mod bench;
pub mod pool;
pub mod polling;
pub mod rss;
pub mod tcp_opt;

use crate::drivers::virtio_net::MacAddress;
use crate::net::ipv4::Ipv4Addr;
use crate::println;

/// Initializes all M36 High-Performance Networking subsystems (pool, RSS multiqueue, NAPI poller).
pub fn init() {
    // 1. Pre-allocate global zero-copy packet slab (128 x 2 KiB buffers)
    pool::GLOBAL_NET_POOL.init(pool::DEFAULT_POOL_CAPACITY);

    // 2. Configure multiqueue RSS across SMP CPU count (default 4 queues on NullOS SMP)
    let cpus = core::cmp::max(1, crate::arch::x86_64::smp::online_cpu_count());
    rss::GLOBAL_MULTIQUEUE.configure(cpus);

    // 3. Configure NAPI adaptive poll budget
    polling::GLOBAL_NAPI_POLLER.set_budget(polling::DEFAULT_NAPI_BUDGET);

    let pst = pool::GLOBAL_NET_POOL.stats();
    println!(
        "[NETPERF] High-Performance Networking initialized: pool={}x2KB queues={} napi_budget={}",
        pst.capacity,
        rss::GLOBAL_MULTIQUEUE.active_queues(),
        polling::DEFAULT_NAPI_BUDGET
    );
}

/// Prints a formatted diagnostic summary of M36 buffer pool, NAPI poller, batching, and RSS queues.
pub fn print_status() {
    let pst = pool::GLOBAL_NET_POOL.stats();
    let nst = polling::GLOBAL_NAPI_POLLER.stats();
    let bst = batch::batch_stats();
    let qstats = rss::GLOBAL_MULTIQUEUE.all_queue_stats();

    println!("=== High-Performance Networking Status (M36) ===");
    println!(
        "  NetBufPool  : cap={} free={} in_use={} peak={} hits={} misses={} frees={}",
        pst.capacity, pst.free_count, pst.in_use, pst.peak_in_use, pst.alloc_hits, pst.alloc_misses, pst.frees
    );
    println!(
        "  NAPI Poller : mode={:?} budget={} polls={} pkts={} budget_hits={} empty={}",
        nst.mode, nst.budget, nst.poll_cycles, nst.packets_polled, nst.budget_exhausted, nst.empty_polls
    );
    println!(
        "  Batch/ZCopy : batches={} batched_pkts={} zerocopy_frames={} bytes_saved={}B",
        bst.total_batches, bst.total_packets_batched, bst.zerocopy_frames_built, bst.bytes_saved_from_copy
    );
    println!("  RSS Queues  : {} active queues (CPU-affine):", qstats.len());
    for q in qstats {
        println!(
            "    Queue #{}: cpu={} rx_pkts={} rx_bytes={} tx_pkts={} tx_bytes={}",
            q.queue_id, q.cpu_affinity, q.rx_packets, q.rx_bytes, q.tx_packets, q.tx_bytes
        );
    }
}

/// Runs automated deterministic self-tests verifying every M36 High-Performance Networking feature.
pub fn run_self_tests() {
    println!("[NETPERF] Running M36 High-Performance Networking self-tests...");

    // 1. Test NetBufPool O(1) alloc/free & zero-copy headroom push/pull
    init();
    let before = pool::GLOBAL_NET_POOL.stats();
    let mut buf = pool::GLOBAL_NET_POOL
        .alloc()
        .expect("GLOBAL_NET_POOL should yield NetBuf");
    assert_eq!(buf.headroom(), pool::NET_BUF_HEADROOM);
    buf.put_payload(b"HELLO_ZEROCOPY")
        .expect("put_payload must succeed");
    {
        let hdr = buf.push_header(8).expect("push_header must succeed in headroom");
        hdr.copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x0E, 0x12, 0x34]);
    }
    assert_eq!(buf.data_len, 8 + 14);
    let pulled = buf.pull_header(8).expect("pull_header must succeed");
    assert_eq!(&pulled[0..4], &[0xDE, 0xAD, 0xBE, 0xEF]);
    assert_eq!(buf.data(), b"HELLO_ZEROCOPY");
    pool::GLOBAL_NET_POOL.free(buf);
    let after = pool::GLOBAL_NET_POOL.stats();
    assert_eq!(after.free_count, before.free_count);
    println!("  [OK] NetBufPool & Zero-Copy headroom push/pull verified.");

    // 2. Test RSS Toeplitz Hashing & Multi-Queue Steering across queues
    rss::GLOBAL_MULTIQUEUE.configure(4);
    let h1 = rss::rss_hash_ipv4_4tuple(
        Ipv4Addr([10, 0, 2, 15]),
        Ipv4Addr([10, 0, 2, 2]),
        51234,
        80,
    );
    let h1_repeat = rss::rss_hash_ipv4_4tuple(
        Ipv4Addr([10, 0, 2, 15]),
        Ipv4Addr([10, 0, 2, 2]),
        51234,
        80,
    );
    assert_eq!(h1, h1_repeat, "RSS Toeplitz hash must be deterministic");
    let q_for_flow = rss::GLOBAL_MULTIQUEUE.select_queue(h1);
    assert!(q_for_flow < 4);

    // 3. Test Zero-Copy UDP/IPv4/Eth framing + PacketBatch + NAPI Poller injection
    let src_mac = MacAddress([0x52, 0x54, 0x00, 0x11, 0x22, 0x33]);
    let dst_mac = MacAddress([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    let mut batch = batch::PacketBatch::new();
    let mut sample_frame_bytes = alloc::vec::Vec::new();
    for port in 4000u16..4008u16 {
        let frame = batch::build_udp_ipv4_zerocopy(
            src_mac,
            dst_mac,
            Ipv4Addr([10, 0, 2, 15]),
            Ipv4Addr([10, 0, 2, 2]),
            port,
            9000,
            b"M36_BATCH_PAYLOAD",
        )
        .expect("build_udp_ipv4_zerocopy must succeed");
        if sample_frame_bytes.is_empty() {
            sample_frame_bytes.extend_from_slice(frame.data());
        }
        batch.push(frame).ok();
    }
    let tx_ok = batch::dispatch_tx_batch(&mut batch, |wire| {
        assert!(wire.len() == 14 + 20 + 8 + 17);
        Ok(())
    });
    assert_eq!(tx_ok, 8);
    assert!(batch.is_empty());

    // Inject frame into NAPI + RSS queue ring and drain
    let drained = polling::GLOBAL_NAPI_POLLER
        .inject_and_drain(&sample_frame_bytes)
        .expect("inject_and_drain must succeed");
    assert_eq!(drained, 1);
    println!("  [OK] RSS Toeplitz, Multiqueue, PacketBatch & NAPI Poller verified.");

    // 4. Test TCP Performance Optimizations (Nagle, Delayed ACK, WScale, SACK, SRTT/RTO, Fast Retransmit)
    let mut cfg = tcp_opt::TcpOptConfig::default();
    cfg.nodelay = false;
    let mut tcb = tcp_opt::TcpOptControlBlock::new(cfg, 1460);
    assert!(!tcb.should_send_nagle(10, 500), "Nagle should coalesce small write with data in flight");
    assert!(tcb.should_send_nagle(10, 0), "Nagle must send when no data is in flight");
    tcb.config.nodelay = true;
    assert!(tcb.should_send_nagle(10, 500), "TCP_NODELAY must bypass Nagle coalescing");

    // Delayed ACK triggers on 2nd segment
    assert!(!tcb.on_rx_in_order(100), "First segment delayed");
    assert!(tcb.on_rx_in_order(100), "Second segment triggers immediate ACK");

    // Window Scaling RFC 7323
    tcb.set_peer_wscale(6);
    assert_eq!(tcb.decode_peer_window(1024), 1024 << 6);

    // SACK Scoreboard RFC 2018 merging
    tcb.record_sack_block(2000, 3000);
    tcb.record_sack_block(3000, 4000);
    assert_eq!(tcb.sack_blocks.len(), 1);
    assert_eq!(tcb.sack_blocks[0].len(), 2000);

    // Jacobson/Karels SRTT/RTO + Fast Retransmit on 3 Dup ACKs
    tcb.update_rtt(20);
    tcb.update_rtt(30);
    assert!(tcb.rto_ms >= 200);
    let _ = tcb.on_ack(1000, 1460);
    assert!(!tcb.on_ack(1000, 0)); // Dup 1
    assert!(!tcb.on_ack(1000, 0)); // Dup 2
    assert!(tcb.on_ack(1000, 0), "3rd dup ACK must trigger Fast Retransmit");
    assert_eq!(tcb.fast_retransmits, 1);

    let pseed = tcp_opt::TcpOptControlBlock::tcp_pseudo_checksum_seed(
        Ipv4Addr([10, 0, 2, 15]),
        Ipv4Addr([10, 0, 2, 2]),
        20,
    );
    assert!(pseed != 0);
    println!("  [OK] TCP Optimizations (Nagle, Delayed ACK, WScale, SACK, RTT/RTO, Fast Retransmit) verified.");

    // 5. Run cycle-accurate RDTSC benchmarks
    bench::run_benchmarks();

    println!("[NETPERF] All M36 High-Performance Networking self-tests passed!");
}
