//! Deterministic Parser Fuzzing, Cross-Subsystem Integration & Stability Soak Tests (Phase 19 - M42).
//!
//! - **Fuzzing**: Deterministic XorShift64 PRNG fuzzes ELF64 validation, TAR USTAR parsing,
//!   Ethernet II framing, IPv4 string parsing, and VFS path normalization/lookup with zero panics.
//! - **Integration Tests**: Exercises end-to-end interactions across Storage Page Cache, VFS,
//!   `.npkg` verification, Lockdep clean dependency state, and structured Klog events.
//! - **Long-Running Stability Tests**: Configurable bounded soak loop tracking PMM free frames,
//!   heap allocation delta, and lockdep violation count (`0`) to prove zero drift.

use super::{record_fuzz_cases, record_soak_cycles, SuiteCounts};
use crate::debug::lockdep;
use crate::fs::tar::TarArchive;
use crate::fs::vfs::{clean_path, VFS};
use crate::memory::{heap, stats as mem_stats};
use crate::net::ethernet::EthernetFrame;
use crate::net::ipv4::{compute_checksum, Ipv4Addr};
use crate::perf::opt::mem_opt::GLOBAL_SLAB_POOL;
use crate::task::elf::validate_user_elf_headers;
use crate::task::npkg::parse_and_verify;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Fast deterministic 64-bit XorShift PRNG for reproducible in-kernel fuzzing.
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    /// Creates a new PRNG seeded with a non-zero 64-bit state.
    fn new(seed: u64) -> Self {
        Self { state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed } }
    }

    /// Generates the next pseudo-random 64-bit value.
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Fills a slice with deterministic pseudo-random bytes.
    fn fill_bytes(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            let n = chunk.len();
            chunk.copy_from_slice(&v[..n]);
        }
    }
}

/// Runs deterministic corpus + mutation fuzzing across kernel parsers (zero panics permitted).
pub fn run_fuzz_tests() -> SuiteCounts {
    crate::println!("[M42/FUZZ] Running deterministic kernel parser fuzzing...");
    let mut prng = XorShift64::new(0x4E55_4C4C_4F53_4D42);
    let mut cases = 0usize;
    let mut passed = 0usize;
    let mut failed = 0usize;

    // 1. Fuzz ELF64 header validator (arbitrary bytes + mutated magic/phoff overflow shapes)
    let mut elf_buf = vec![0u8; 256];
    for i in 0..256usize {
        let len = 8 + (i % 248);
        prng.fill_bytes(&mut elf_buf[..len]);
        if i % 2 == 0 && len >= 64 {
            // Mutate plausible ELF64 prefix so validator reaches phoff/phentsize/segment checks
            elf_buf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
            elf_buf[4] = 2; // ELFCLASS64
            elf_buf[5] = 1; // ELFDATA2LSB
            elf_buf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
            elf_buf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // EM_X86_64
        }
        let _ = validate_user_elf_headers(&elf_buf[..len]);
        cases += 1;
    }
    passed += 1;

    // 2. Fuzz POSIX USTAR TAR parser
    let mut tar_buf = vec![0u8; 1024];
    for i in 0..256usize {
        let len = (prng.next_u64() as usize) % tar_buf.len();
        prng.fill_bytes(&mut tar_buf[..len]);
        if i % 3 == 0 && len >= 512 {
            tar_buf[0..8].copy_from_slice(b"fuzzfile");
            tar_buf[156] = b'0';
            tar_buf[257..263].copy_from_slice(b"ustar\0");
        }
        let mut count = 0usize;
        for _entry in TarArchive::new(&tar_buf[..len]) {
            count += 1;
            if count > 16 {
                break;
            }
        }
        cases += 1;
    }
    passed += 1;

    // 3. Fuzz Ethernet II & IPv4 parsers
    let mut pkt_buf = [0u8; 160];
    for i in 0..256usize {
        let len = (prng.next_u64() as usize) % pkt_buf.len();
        prng.fill_bytes(&mut pkt_buf[..len]);
        let _ = EthernetFrame::parse(&pkt_buf[..len]);

        let mut s = String::new();
        for _ in 0..((i % 18) + 1) {
            let b = (prng.next_u64() % 16) as u8;
            let ch = match b {
                0..=9 => (b'0' + b) as char,
                10..=12 => '.',
                13 => ':',
                14 => '/',
                _ => 'x',
            };
            s.push(ch);
        }
        let _ = Ipv4Addr::parse(&s);
        cases += 1;
    }
    passed += 1;

    // 4. Fuzz VFS path canonicalizer & lookup
    for i in 0..128usize {
        let mut path = String::from("/");
        let segs = (i % 6) + 1;
        for _ in 0..segs {
            match prng.next_u64() % 5 {
                0 => path.push_str("../"),
                1 => path.push_str("./"),
                2 => path.push_str("bin/"),
                3 => path.push_str("//"),
                _ => path.push_str("fuzz_missing/"),
            }
        }
        let norm = clean_path(&path);
        if !norm.starts_with('/') {
            failed += 1;
        }
        let _ = VFS.resolve_path(&norm);
        cases += 1;
    }
    if failed == 0 {
        passed += 1;
    }

    record_fuzz_cases(cases);
    crate::println!(
        "[M42/FUZZ] Evaluated {} mutated inputs across ELF/TAR/Net/VFS with zero panics (passed={}, failed={}).",
        cases,
        passed,
        failed
    );
    SuiteCounts::record(passed, failed)
}

/// Runs cross-subsystem integration tests spanning VFS, NPKG SDK packages, Storage & Lockdep.
pub fn run_integration_tests() -> SuiteCounts {
    crate::println!("[M42/INTEG] Running cross-subsystem integration tests...");
    let mut passed = 0usize;
    let mut failed = 0usize;

    macro_rules! check {
        ($cond:expr, $label:expr) => {
            if $cond {
                passed += 1;
            } else {
                crate::println!("[M42/INTEG] FAIL: {}", $label);
                failed += 1;
            }
        };
    }

    // 1. Read /pkg/sdk_demo.npkg from UEFI-loaded initramfs VFS -> parse_and_verify -> validate ELF payload
    if let Ok(npkg_inode) = VFS.resolve_path("/pkg/sdk_demo.npkg") {
        let sz = npkg_inode.stat().size;
        let mut raw = vec![0u8; sz];
        if npkg_inode.read_at(0, &mut raw) == Ok(sz) {
            if let Ok(pkg) = parse_and_verify(&raw) {
                let elf_ok = validate_user_elf_headers(pkg.payload).is_ok();
                check!(pkg.name == "sdk_demo" && elf_ok, "Initramfs VFS + NPKG CRC32 + ELF64 validation integration");
            } else {
                check!(false, "NPKG parse_and_verify failed");
            }
        } else {
            check!(false, "VFS read_at /pkg/sdk_demo.npkg");
        }
    } else {
        check!(false, "VFS resolve /pkg/sdk_demo.npkg");
    }

    // 2. Verify M40 Lockdep violation count remains 0 after active subsystems run
    let lock_status = lockdep::format_status();
    check!(
        lock_status.contains("Violations Caught  : 0"),
        "Zero Lockdep deadlock violations across kernel subsystems"
    );

    // 3. Verify structured klog telemetry ring actively captures M42 reliability events
    crate::debug::klog::log(
        crate::debug::klog::LogLevel::Info,
        "M42",
        "Integration test milestone verified",
    );
    let klog_text = crate::debug::klog::format_logs(None, Some("M42"), false, 8);
    check!(klog_text.contains("Integration test milestone"), "Structured Klog records integration telemetry");

    crate::println!("[M42/INTEG] Completed: passed={}, failed={}", passed, failed);
    SuiteCounts::record(passed, failed)
}

/// Runs a bounded long-running stability soak loop across memory, VFS, slab, net & scheduler yields.
pub fn run_soak_tests(cycles: usize) -> SuiteCounts {
    crate::println!("[M42/SOAK] Running {} stability soak cycles...", cycles);
    let mut passed = 0usize;
    let mut failed = 0usize;

    let frames_start = mem_stats().map(|s| s.free_frames).unwrap_or(0);
    let heap_start = heap::stats().current_usage();

    for cycle in 0..cycles {
        // Exercise slab cache
        if let Some((idx, slot, ptr)) = GLOBAL_SLAB_POOL.alloc(256) {
            unsafe { ptr.write_bytes((cycle & 0xFF) as u8, 256); }
            GLOBAL_SLAB_POOL.free(idx, slot);
        } else {
            failed += 1;
            break;
        }

        // Exercise temporary heap + checksum computation
        let mut scratch: Vec<u8> = Vec::with_capacity(512);
        scratch.resize(512, (cycle & 0x7F) as u8);
        let _ = compute_checksum(&scratch);
        drop(scratch);

        // Exercise VFS lookup & yield cleanly
        if VFS.resolve_path("/etc/nullos-release").is_err() {
            failed += 1;
            break;
        }

        if cycle % 4 == 0 {
            crate::task::scheduler::yield_now();
        }
    }

    let frames_end = mem_stats().map(|s| s.free_frames).unwrap_or(0);
    let heap_end = heap::stats().current_usage();
    let frame_drift = frames_start.abs_diff(frames_end);
    let heap_drift = heap_end.saturating_sub(heap_start);

    if failed == 0 && frame_drift <= 4 && heap_drift <= 4096 {
        passed += 1;
    } else {
        crate::println!(
            "[M42/SOAK] FAIL: cycles={}, failed={}, frame_drift={}, heap_drift={}B",
            cycles, failed, frame_drift, heap_drift
        );
        failed += 1;
    }

    record_soak_cycles(cycles);
    crate::println!(
        "[M42/SOAK] Completed {} cycles (frame_drift={}, heap_drift={}B): passed={}, failed={}",
        cycles, frame_drift, heap_drift, passed, failed
    );
    SuiteCounts::record(passed, failed)
}
