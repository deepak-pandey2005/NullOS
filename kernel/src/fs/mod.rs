//! Filesystems & Storage Subsystem (Phase 6 - M15 Initramfs & M16 VFS).
//!
//! Subsystem features:
//! - POSIX USTAR Archive parser for Initramfs (zero-copy file extraction).
//! - Virtual File System (VFS) with Inodes, Dentries, FileSystem trait, and Mount Table.
//! - In-memory RamFS with directories, files, and character devices (`/dev/console`, `/dev/null`, `/dev/zero`).
//! - Per-process File Descriptor Table (stdin, stdout, stderr, seek, read, write).
//! - File syscalls: `open`, `close`, `read`, `write`, `stat`, `lseek`.

pub mod fd;
pub mod initramfs;
pub mod tar;
pub mod vfs;

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use crate::println;
use self::fd::{FileDescriptor, FileDescriptorTable, SEEK_SET};
use self::initramfs::RamFs;
use self::tar::{TarArchive, TarType};
use self::vfs::{OpenFlags, VFS};

/// Initializes the Virtual File System and mounts the root RamFS / Initramfs.
pub fn init() {
    println!();
    println!("----------------------------------------");
    println!(" [FS] Initializing Filesystems & VFS    ");
    println!("----------------------------------------");

    let ramfs = Arc::new(RamFs::new());
    ramfs.populate_defaults();

    if let Err(e) = VFS.mount("/", ramfs) {
        println!("[FS] Failed to mount root filesystem: {:?}", e);
        return;
    }

    println!("[FS] Virtual File System (VFS) active.");
    println!("[FS] Mounted RamFS initramfs at \"/\".");
    println!("[FS] Initialized standard device nodes: /dev/console, /dev/null, /dev/zero.");
}

/// Runs automated verification self-tests for M15 Initramfs & M16 VFS.
pub fn run_self_tests() {
    println!("[FS] Running Filesystem & VFS verification self-tests...");

    // Test 1: TAR Archive Parser Verification
    test_tar_parser();

    // Test 2: VFS Path Traversal & Reading
    test_vfs_path_resolution();

    // Test 3: File Creation & Writing
    test_vfs_file_create_and_write();

    // Test 4: File Descriptor Table & Standard Streams
    test_file_descriptor_table();

    println!("[FS] All Filesystem & VFS self-tests passed successfully!");
}

fn test_tar_parser() {
    // Construct a synthetic 512-byte POSIX USTAR block for testing
    let mut synthetic_tar = Vec::new();
    let file_content = b"Kernel Initramfs Test Content 123456789\n";

    // Header block (512 bytes)
    let mut header = [0u8; 512];
    // Name: "test_embedded.txt"
    let name = b"test_embedded.txt";
    header[..name.len()].copy_from_slice(name);
    // Mode: "0000644\0"
    header[100..108].copy_from_slice(b"0000644\0");
    // UID / GID
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    // Size: 40 bytes in octal -> "00000000050\0"
    let size_octal = alloc::format!("{:011o} ", file_content.len());
    header[124..136].copy_from_slice(size_octal.as_bytes());
    // Mtime
    header[136..148].copy_from_slice(b"14000000000\0");
    // Typeflag: '0' (Normal file)
    header[156] = b'0';
    // Magic: "ustar\0"
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");

    // Compute checksum (sum of all bytes with checksum field set to spaces)
    let mut sum: u32 = 0;
    for (i, &b) in header.iter().enumerate() {
        if (148..156).contains(&i) {
            sum += b' ' as u32;
        } else {
            sum += b as u32;
        }
    }
    let chksum_str = alloc::format!("{:06o}\0 ", sum);
    header[148..156].copy_from_slice(chksum_str.as_bytes());

    synthetic_tar.extend_from_slice(&header);
    synthetic_tar.extend_from_slice(file_content);

    // Add padding to 512-byte boundary
    let padding = (512 - (file_content.len() % 512)) % 512;
    synthetic_tar.extend_from_slice(&vec![0u8; padding]);

    // Add two 512-byte zero blocks (end of archive)
    synthetic_tar.extend_from_slice(&[0u8; 1024]);

    // Parse the synthetic archive
    let archive = TarArchive::new(&synthetic_tar);
    let mut count = 0;
    for entry in archive {
        assert_eq!(entry.name, "test_embedded.txt");
        assert_eq!(entry.size, file_content.len());
        assert_eq!(entry.entry_type, TarType::NormalFile);
        assert_eq!(entry.data, file_content);
        count += 1;
    }
    assert_eq!(count, 1, "TAR archive parser failed to extract test entry");
    println!("[FS] TAR Archive parser verified: extracted 1 file ({} bytes)", file_content.len());
}

fn test_vfs_path_resolution() {
    // Verify reading /hello.txt
    let node = VFS.resolve_path("/hello.txt").expect("VFS failed to resolve /hello.txt");
    let mut buf = [0u8; 64];
    let bytes_read = node.read_at(0, &mut buf).expect("Failed to read /hello.txt");
    let content = core::str::from_utf8(&buf[..bytes_read]).unwrap();
    assert!(content.contains("Hello from NullOS Initramfs VFS!"));

    // Verify reading /etc/os-release
    let etc_node = VFS.resolve_path("/etc/os-release").expect("VFS failed to resolve /etc/os-release");
    let bytes_read_etc = etc_node.read_at(0, &mut buf).expect("Failed to read /etc/os-release");
    let etc_content = core::str::from_utf8(&buf[..bytes_read_etc]).unwrap();
    assert!(etc_content.contains("ID=nullos"));

    println!("[FS] VFS path resolution verified: read /hello.txt and /etc/os-release successfully.");
}

fn test_vfs_file_create_and_write() {
    // Open / create a new file in /tmp/test.txt
    let new_file = VFS
        .open("/tmp/test.txt", OpenFlags {
            read: true,
            write: true,
            create: true,
            append: false,
            truncate: false,
        })
        .expect("Failed to create /tmp/test.txt");

    let write_data = b"Testing dynamic file creation and writing in NullOS RamFS!";
    let written = new_file.write_at(0, write_data).expect("Failed to write to /tmp/test.txt");
    assert_eq!(written, write_data.len());

    // Verify written content by re-reading through VFS
    let read_node = VFS.resolve_path("/tmp/test.txt").expect("Failed to re-lookup /tmp/test.txt");
    let mut read_buf = [0u8; 128];
    let n = read_node.read_at(0, &mut read_buf).expect("Failed to read /tmp/test.txt");
    assert_eq!(&read_buf[..n], write_data);

    println!("[FS] VFS file creation, write, and read verified: {} bytes.", written);
}

fn test_file_descriptor_table() {
    let fd_table = FileDescriptorTable::new();

    // Verify standard descriptors 0, 1, 2 exist
    assert!(fd_table.get(0).is_ok(), "stdin missing");
    assert!(fd_table.get(1).is_ok(), "stdout missing");
    assert!(fd_table.get(2).is_ok(), "stderr missing");

    // Open file and allocate a new descriptor
    let node = VFS.resolve_path("/hello.txt").unwrap();
    let desc = Arc::new(FileDescriptor::new(node, OpenFlags::READ_WRITE));
    let fd = fd_table.alloc_fd(desc).expect("Failed to allocate file descriptor");
    assert_eq!(fd, 3, "First allocated fd should be 3");

    // Read through the file descriptor
    let handle = fd_table.get(fd).unwrap();
    let mut buf = [0u8; 16];
    let n = handle.read(&mut buf).unwrap();
    assert_eq!(n, 16);
    assert_eq!(&buf[..5], b"Hello");

    // Seek back to start
    let pos = handle.seek(0, SEEK_SET).unwrap();
    assert_eq!(pos, 0);

    // Re-read
    let n2 = handle.read(&mut buf).unwrap();
    assert_eq!(n2, 16);
    assert_eq!(&buf[..5], b"Hello");

    // Close descriptor
    fd_table.close(fd).expect("Failed to close fd");
    assert!(fd_table.get(fd).is_err(), "fd should be closed");

    println!("[FS] FileDescriptorTable verified: stdin/out/err, fd allocation, seek, and close.");
}
