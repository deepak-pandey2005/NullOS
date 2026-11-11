//! Filesystem & Network Stack Reliability Tests (Phase 19 - M42).
//!
//! - **Filesystem Tests**: RamFS multi-offset read/write/truncate/unlink/error-code tests,
//!   plus persistent FAT32 (`/disk`) file create, write, re-open, byte-exact read, and cleanup.
//! - **Network Tests**: IPv4 parsing & RFC 1071 checksums, Ethernet II roundtrip framing &
//!   truncation rejection, ARP cache insert/lookup, and loopback UDP socket bind/send/recv/close.

use super::SuiteCounts;
use crate::drivers::virtio_net::MacAddress;
use crate::fs::vfs::{FsError, OpenFlags, VFS};
use crate::net::arp::{arp_insert, arp_lookup, ArpPacket};
use crate::net::ethernet::{EtherType, EthernetFrame};
use crate::net::ipv4::{compute_checksum, Ipv4Addr};
use crate::net::socket::{bind, close, connect, recv, send, socket, SocketType};
use alloc::vec;

const CREATE_RW: OpenFlags = OpenFlags {
    read: true,
    write: true,
    create: true,
    append: false,
    truncate: false,
    nonblock: false,
};

/// Runs comprehensive RamFS and FAT32 filesystem correctness tests.
pub fn run_fs_tests() -> SuiteCounts {
    crate::println!("[M42/FS] Running VFS, RamFS & FAT32 persistence tests...");
    let mut passed = 0usize;
    let mut failed = 0usize;

    macro_rules! check {
        ($cond:expr, $label:expr) => {
            if $cond {
                passed += 1;
            } else {
                crate::println!("[M42/FS] FAIL: {}", $label);
                failed += 1;
            }
        };
    }

    // 1. RamFS error semantics
    check!(
        matches!(VFS.resolve_path("/nonexistent/file.txt"), Err(FsError::NotFound)),
        "VFS returns NotFound for missing paths"
    );
    if let Ok(bin_node) = VFS.resolve_path("/bin") {
        let mut tmp = [0u8; 4];
        check!(
            matches!(bin_node.read_at(0, &mut tmp), Err(FsError::IsADirectory)),
            "Directory read_at returns IsADirectory"
        );
    } else {
        check!(false, "/bin directory exists");
    }

    // 2. RamFS multi-offset write, partial read, truncate & unlink under /tmp
    let test_path = "/tmp/m42_fs_test.bin";
    let _ = VFS.unlink(test_path);
    if let Ok(inode) = VFS.open(test_path, CREATE_RW) {
        let payload = b"NullOS-M42-FS-Verification-Payload-1234567890";
        let w = inode.write_at(0, payload);
        check!(w == Ok(payload.len()), "RamFS write_at succeeds with exact byte count");

        let mut rbuf = vec![0u8; payload.len()];
        let r = inode.read_at(0, &mut rbuf);
        check!(r == Ok(payload.len()) && rbuf.as_slice() == payload, "RamFS read_at byte-exact match");

        // Partial offset read + truncate
        let mut tail = [0u8; 10];
        let r2 = inode.read_at(payload.len() - 10, &mut tail);
        check!(r2 == Ok(10) && &tail == b"1234567890", "RamFS offset read matches tail");
        check!(inode.truncate(6).is_ok() && inode.stat().size == 6, "RamFS truncate updates file size");

        let mut head = [0u8; 16];
        let r3 = inode.read_at(0, &mut head);
        check!(r3 == Ok(6) && &head[..6] == b"NullOS", "RamFS truncated content matches prefix");

        check!(VFS.unlink(test_path).is_ok(), "RamFS unlink removes temporary file");
        check!(matches!(VFS.resolve_path(test_path), Err(FsError::NotFound)), "Unlinked RamFS file unreachable");
    } else {
        check!(false, "Create /tmp/m42_fs_test.bin");
    }

    // 3. FAT32 persistent storage test on /disk (if VirtIO/FAT32 disk is mounted)
    let fat_test_path = "/disk/M42TEST.BIN";
    if VFS.resolve_path("/disk").is_ok() {
        let _ = VFS.unlink(fat_test_path);
        if let Ok(finode) = VFS.open(fat_test_path, CREATE_RW) {
            let pattern = b"FAT32-M42-PERSISTENCE-CHECK-AABBCCDD";
            let wr = finode.write_at(0, pattern);
            check!(wr == Ok(pattern.len()), "FAT32 persistent file write");
            if let Ok(reopened) = VFS.resolve_path(fat_test_path) {
                let mut verify = vec![0u8; pattern.len()];
                let rd = reopened.read_at(0, &mut verify);
                let ok = rd == Ok(pattern.len()) && verify.as_slice() == pattern;
                if !ok {
                    crate::println!(
                        "[M42/FS] FAT32 diag: stat_size={}, rd={:?}, prefix={:?}",
                        reopened.stat().size,
                        rd,
                        &verify[..8]
                    );
                }
                check!(ok, "FAT32 re-open byte-exact read");
            } else {
                check!(false, "FAT32 re-open after write");
            }
            let _ = VFS.unlink(fat_test_path);
        } else {
            check!(false, "Open /disk/M42TEST.BIN on mounted storage");
        }
    }

    crate::println!("[M42/FS] Completed: passed={}, failed={}", passed, failed);
    SuiteCounts::record(passed, failed)
}

/// Runs network protocol, framing, ARP, and loopback socket reliability tests.
pub fn run_net_tests() -> SuiteCounts {
    crate::println!("[M42/NET] Running network protocol & loopback socket tests...");
    let mut passed = 0usize;
    let mut failed = 0usize;

    macro_rules! check {
        ($cond:expr, $label:expr) => {
            if $cond {
                passed += 1;
            } else {
                crate::println!("[M42/NET] FAIL: {}", $label);
                failed += 1;
            }
        };
    }

    // 1. IPv4 address parsing & subnet arithmetic
    check!(Ipv4Addr::parse("10.0.2.15") == Ok(Ipv4Addr::new(10, 0, 2, 15)), "Ipv4Addr valid parse");
    check!(Ipv4Addr::parse("127.0.0.1").map(|ip| ip.is_loopback()) == Ok(true), "Ipv4Addr loopback check");
    check!(Ipv4Addr::parse("999.1.2.3").is_err(), "Ipv4Addr rejects >255 octet");
    check!(Ipv4Addr::parse("10.0.2").is_err(), "Ipv4Addr rejects truncated string");

    let mask = Ipv4Addr::new(255, 255, 255, 0);
    check!(
        Ipv4Addr::new(192, 168, 1, 10).in_same_subnet(&Ipv4Addr::new(192, 168, 1, 200), &mask),
        "IPv4 same-subnet check true"
    );
    check!(
        !Ipv4Addr::new(192, 168, 1, 10).in_same_subnet(&Ipv4Addr::new(192, 168, 2, 10), &mask),
        "IPv4 cross-subnet check false"
    );

    // 2. RFC 1071 Internet Checksum standard vector
    let rfc_hdr: [u8; 20] = [
        0x45, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0xb8, 0x61, 0xc0, 0xa8, 0x00,
        0x01, 0xc0, 0xa8, 0x00, 0xc7,
    ];
    check!(compute_checksum(&rfc_hdr) == 0, "RFC 1071 IPv4 header checksum validates to 0");

    // 3. Ethernet II frame serialization + parse roundtrip + runt-frame rejection
    let src_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let dst_mac = MacAddress::new([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    let raw_frame = EthernetFrame::build(dst_mac, src_mac, EtherType::Ipv4, b"M42_ETH_OK");
    match EthernetFrame::parse(&raw_frame) {
        Ok(parsed) => {
            check!(
                parsed.header.dst_mac() == dst_mac
                    && parsed.header.src_mac() == src_mac
                    && parsed.header.ethertype() == EtherType::Ipv4
                    && &parsed.payload[..10] == b"M42_ETH_OK",
                "Ethernet II build/parse roundtrip"
            );
        }
        Err(_) => check!(false, "Ethernet II parse must succeed"),
    }
    check!(EthernetFrame::parse(&[0u8; 10]).is_err(), "Ethernet II rejects undersized <14B frame");

    // 4. ARP packet serialization + ARP table insert/lookup roundtrip
    let test_ip = Ipv4Addr::new(10, 0, 2, 99);
    let arp_pkt = ArpPacket::new_request(src_mac, test_ip, Ipv4Addr::new(10, 0, 2, 2));
    let arp_bytes = arp_pkt.to_bytes();
    check!(arp_bytes.len() == 28 && arp_pkt.operation() == 1, "ARP request 28-byte encoding");
    arp_insert(test_ip, src_mac);
    check!(arp_lookup(test_ip) == Some(src_mac), "ARP cache insert/lookup roundtrip");

    // 5. Loopback UDP socket bind -> connect -> send -> recv -> close
    if let (Ok(rx_sock), Ok(tx_sock)) = (socket(SocketType::Datagram), socket(SocketType::Datagram)) {
        let port = 42042u16;
        let bound = bind(rx_sock, port).is_ok() && connect(tx_sock, Ipv4Addr::new(127, 0, 0, 1), port).is_ok();
        check!(bound, "Loopback UDP bind/connect");

        let sent = send(tx_sock, b"NULLOS_M42_UDP");
        check!(sent == Ok(14), "Loopback UDP send succeeds");

        let mut buf = [0u8; 32];
        let recvd = recv(rx_sock, &mut buf);
        check!(recvd == Ok(Some(14)) && &buf[..14] == b"NULLOS_M42_UDP", "Loopback UDP recv matches payload");

        let _ = close(tx_sock);
        let _ = close(rx_sock);
    } else {
        check!(false, "Allocate UDP sockets for M42 test");
    }

    crate::println!("[M42/NET] Completed: passed={}, failed={}", passed, failed);
    SuiteCounts::record(passed, failed)
}
