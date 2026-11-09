//! # Filesystems & Storage Subsystem (Phase 6 - M15 Initramfs & M16 VFS)
//!
//! This module is the **top-level orchestrator** for all storage and filesystem
//! functionality in NullOS. It wires together several independently defined
//! sub-modules into a cohesive storage stack and provides the two kernel-facing
//! entry points that the boot sequence calls: [`init`] and [`mount_block_devices`].
//!
//! ## Sub-modules
//!
//! | Module | Role |
//! |--------|------|
//! | [`vfs`]        | Virtual File System abstraction layer: trait definitions, global mount table, path resolution |
//! | [`initramfs`]  | In-memory RamFS driver and device nodes (`/dev/console`, `/dev/null`, `/dev/zero`) |
//! | [`tar`]        | POSIX USTAR archive parser used to load the initramfs image |
//! | [`fd`]         | Per-process file descriptor table with POSIX-style seek/read/write |
//! | [`fat32`]      | FAT32 persistent block-device filesystem driver |
//! | [`ext2`]       | ext2/ext3-compatible persistent block-device filesystem driver |
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    Kernel / Syscall Layer                │
//! │   open(), read(), write(), lseek(), stat(), close()     │
//! └───────────────────────────┬─────────────────────────────┘
//!                             │  uses
//! ┌───────────────────────────▼─────────────────────────────┐
//! │             FileDescriptorTable  (fd.rs)                │
//! │  fd 0 (stdin) │ fd 1 (stdout) │ fd 2 (stderr) │ …      │
//! │  FileDescriptor { inode, flags, offset: Spinlock }      │
//! └───────────────────────────┬─────────────────────────────┘
//!                             │  Arc<dyn Inode>
//! ┌───────────────────────────▼─────────────────────────────┐
//! │               VFS  (vfs.rs)  –  static VFS singleton    │
//! │  mount table: BTreeMap<path, Arc<dyn FileSystem>>       │
//! │  resolve_path() → longest-prefix-match mount lookup     │
//! └────────┬──────────────────┬──────────────────┬──────────┘
//!          │ "/"              │ "/disk"           │ "/mnt"
//!    ┌─────▼──────┐    ┌──────▼──────┐    ┌──────▼──────┐
//!    │   RamFs    │    │ Fat32 / ext2│    │ Fat32 / ext2│
//!    │(initramfs) │    │ (persistent)│    │ (persistent)│
//!    └────────────┘    └─────────────┘    └─────────────┘
//! ```
//!
//! ## RamFS / Initramfs
//!
//! The root filesystem (`/`) is always an **in-memory RamFS** (see [`initramfs`]).
//! It is pre-populated at boot with:
//! * Standard device nodes: `/dev/console`, `/dev/null`, `/dev/zero`
//! * Textual config files: `/etc/os-release`, `/etc/hostname`
//! * Statically compiled binaries embedded via `include_bytes!`: `/bin/hello.elf`, `/lib/libc.so`
//! * A scratch area: `/tmp`
//! * Mount-point stubs for block devices: `/disk`, `/mnt`
//!
//! ## FAT32
//!
//! FAT32 (File Allocation Table, 32-bit cluster addresses) is the most common
//! filesystem on removable media and virtual QEMU disks. The kernel probes for it
//! first in [`mount_block_devices`]. If the BPB (BIOS Parameter Block) signature
//! matches, the driver is mounted at both `/disk` and `/mnt`.
//!
//! ## ext2
//!
//! ext2 is the second extended filesystem, the classic Linux filesystem. If FAT32
//! detection fails, the kernel tries ext2. If the superblock magic (`0xEF53`) is
//! found, the driver is mounted at `/disk`.
//!
//! ## Mount Table
//!
//! Mounts are stored in a `BTreeMap<String, Arc<dyn FileSystem>>` protected by a
//! `Spinlock`. Path resolution uses **longest-prefix matching**: the mount point
//! whose string is the longest prefix of the requested path wins. This mirrors how
//! Linux's `namei` layer works.
//!
//! ## Subsystem features
//! - POSIX USTAR Archive parser for Initramfs (zero-copy file extraction).
//! - Virtual File System (VFS) with Inodes, Dentries, FileSystem trait, and Mount Table.
//! - In-memory RamFS with directories, files, and character devices (`/dev/console`, `/dev/null`, `/dev/zero`).
//! - Per-process File Descriptor Table (stdin, stdout, stderr, seek, read, write).
//! - File syscalls: `open`, `close`, `read`, `write`, `stat`, `lseek`.

pub mod ext2;
pub mod fat32;
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
///
/// # Sequence of operations
///
/// 1. **Construct RamFs**: creates an empty in-memory filesystem with a root
///    [`RamDirectory`](initramfs::RamDirectory) node.
///
/// 2. **`populate_defaults()`**: populates the standard directory tree and device
///    nodes under the RamFS root. This creates:
///    - `/dev/console` – character device; writes go to UART, reads poll keyboard/serial
///    - `/dev/null`    – bit-bucket device; reads return EOF, writes are silently discarded
///    - `/dev/zero`    – zero-fill device; reads return 0x00 bytes, writes are discarded
///    - `/etc/os-release`, `/etc/hostname` – OS identity files
///    - `/bin/hello.elf` – statically embedded user-space binary (via `include_bytes!`)
///    - `/lib/libc.so`   – statically embedded C library shared object
///    - `/tmp`           – ephemeral scratch directory (mode 0777)
///    - `/disk`, `/mnt`  – empty stub directories used as mount-points later
///    - `/hello.txt`     – greeting file for self-test verification
///
/// 3. **`VFS.mount("/")`**: registers the RamFs instance in the global VFS mount
///    table under the key `"/"`. After this call every absolute path starts its
///    resolution at the RamFs root inode.
///
/// # Failure handling
///
/// If mounting fails (practically impossible for RamFs, but guarded defensively),
/// the error is logged and the function returns early. The VFS will not be usable
/// in that case.
pub fn init() {
    println!();
    println!("----------------------------------------");
    println!(" [FS] Initializing Filesystems & VFS    ");
    println!("----------------------------------------");

    // Allocate the RamFs instance on the heap and wrap it in an Arc so the VFS
    // mount table can hold a reference-counted pointer to it.
    let ramfs = Arc::new(RamFs::new());

    // Populate the standard directory hierarchy.  This call is infallible –
    // any internal allocation failure would panic, not return an Err.
    ramfs.populate_defaults();

    // Register the RamFs as the root filesystem in the global VFS.
    // After this call `VFS.resolve_path("/anything")` can find files inside RamFs.
    if let Err(e) = VFS.mount("/", ramfs) {
        println!("[FS] Failed to mount root filesystem: {:?}", e);
        return;
    }

    println!("[FS] Virtual File System (VFS) active.");
    println!("[FS] Mounted RamFS initramfs at \"/\".");
    println!("[FS] Initialized standard device nodes: /dev/console, /dev/null, /dev/zero.");
}

/// Probes registered block devices and mounts detected persistent filesystems (FAT32 / ext2).
///
/// This function is called **after** [`init`] has established the RamFs root.
/// It queries the block-device layer for the **primary** device (typically the
/// first IDE/virtio disk registered during driver initialization) and attempts to
/// identify and mount the filesystem it contains.
///
/// # Detection strategy (tried in order)
///
/// 1. **FAT32** – [`fat32::Fat32FileSystem::new`] reads and validates the BIOS
///    Parameter Block (BPB) from sector 0.  If the BPB's signature and cluster
///    count are consistent with FAT32, the filesystem is mounted at **both**
///    `/disk` and `/mnt` so that both paths can be used to access it.
///    [`verify_fat32_operations`] is then called to perform a 3-step smoke-test.
///
/// 2. **ext2** – If FAT32 detection returns an error, [`ext2::Ext2FileSystem::new`]
///    reads the superblock from byte offset 1024 (block 0 padding, then block 1).
///    If the magic number `0xEF53` matches, the filesystem is mounted at `/disk`.
///
/// 3. **Unknown** – If neither driver recognises the on-disk format, a warning
///    is emitted and the function returns without mounting anything on `/disk`.
///
/// # No block devices
///
/// If the block-device layer reports no primary device (e.g. the kernel is running
/// without a disk image), the function logs a note and exits cleanly.  This is not
/// an error – the kernel can continue to operate purely from the RamFs initramfs.
pub fn mount_block_devices() {
    println!("----------------------------------------");
    println!(" [FS] Mounting Storage Block Devices    ");
    println!("----------------------------------------");

    if let Some(block_dev) = crate::drivers::block::primary_device() {
        println!("[FS] Probing block device '{}' for filesystems...", block_dev.name());

        // 1. Try mounting as FAT32
        match fat32::Fat32FileSystem::new(block_dev.clone()) {
            Ok(fat32_fs) => {
                // fat32_fs.bpb.total_sectors is the total count of 512-byte sectors on
                // the volume.  Multiply by 512 and divide by 1024*1024 to get MiB.
                let capacity_mib = (fat32_fs.bpb.total_sectors as u64 * 512) / (1024 * 1024);
                println!(
                    "[FS] Detected FAT32 filesystem ({} MiB, root cluster: {}).",
                    capacity_mib, fat32_fs.bpb.root_cluster
                );

                // Mount the same Arc<Fat32FileSystem> at two paths.  Both mount
                // points will share the same filesystem instance – this is safe
                // because all mutable state inside is protected by Spinlocks.
                if let Err(e) = VFS.mount("/disk", fat32_fs.clone()) {
                    println!("[FS] Failed to mount FAT32 at /disk: {:?}", e);
                } else {
                    println!("[FS] Successfully mounted FAT32 filesystem at /disk.");
                }

                if let Err(e) = VFS.mount("/mnt", fat32_fs.clone()) {
                    println!("[FS] Failed to mount FAT32 at /mnt: {:?}", e);
                } else {
                    println!("[FS] Successfully mounted FAT32 filesystem at /mnt.");
                }

                // Verify FAT32 Read/Write operations
                verify_fat32_operations();
                return; // No need to try ext2 – we already found and mounted FAT32.
            }
            Err(_) => {
                // Not FAT32, try ext2
            }
        }

        // 2. Try mounting as ext2
        match ext2::Ext2FileSystem::new(block_dev.clone()) {
            Ok(ext2_fs) => {
                println!(
                    "[FS] Detected ext2 filesystem (block size: {} bytes).",
                    ext2_fs.block_size
                );

                if let Err(e) = VFS.mount("/disk", ext2_fs.clone()) {
                    println!("[FS] Failed to mount ext2 at /disk: {:?}", e);
                } else {
                    println!("[FS] Successfully mounted ext2 filesystem at /disk.");
                }
                return;
            }
            Err(_) => {
                // Not ext2
            }
        }

        println!("[FS] Warning: No supported filesystem (FAT32 or ext2) detected on '{}'.", block_dev.name());
    } else {
        println!("[FS] Note: No block devices available to mount.");
    }
}

/// Performs a 3-stage smoke-test of the FAT32 driver's read and write paths.
///
/// This function is called automatically at the end of a successful FAT32 mount.
/// It is **not** exported because it is an internal verification helper.
///
/// ## Test 1 – Read a pre-existing file (`/disk/README.TXT`)
///
/// Opens `/disk/README.TXT` with read-only flags.  This file must exist on the
/// disk image before the kernel boots (created by the host-side build scripts).
/// If it can be read successfully, the FAT32 cluster-chain traversal and sector
/// read path are confirmed to work.
///
/// ## Test 2 – Read another pre-existing file (`/disk/NOTES.TXT`)
///
/// Same as Test 1 but for a second file.  Verifying two distinct files exercises
/// the directory-entry enumeration code path with more than one entry.
///
/// ## Test 3 – Create and write a new file (`/disk/TEST_WR.TXT`)
///
/// Opens `/disk/TEST_WR.TXT` with `create | write | truncate` flags.  This
/// exercises:
/// * Directory entry allocation (finding or creating a free slot in the root dir)
/// * FAT cluster allocation (reserving a new cluster from the FAT)
/// * Sector write path back to the block device
///
/// The written string is a human-readable success banner that can be verified
/// from the host after the VM exits.
fn verify_fat32_operations() {
    println!("[FS] Verifying FAT32 persistent read/write operations...");

    // Test 1: Read pre-populated README.TXT
    // `OpenFlags::READ_ONLY` = { read: true, write: false, … }
    match VFS.open("/disk/README.TXT", OpenFlags::READ_ONLY) {
        Ok(file) => {
            let mut buf = [0u8; 128];
            match file.read_at(0, &mut buf) {
                Ok(n) => {
                    // Convert raw bytes to UTF-8; fall back to "" if the file
                    // contains non-UTF-8 data (e.g. it was written as Latin-1).
                    let content = core::str::from_utf8(&buf[..n]).unwrap_or("");
                    println!("[FS] Read {} bytes from /disk/README.TXT: {:?}", n, content.trim());
                }
                Err(e) => println!("[FS] Failed to read /disk/README.TXT: {:?}", e),
            }
        }
        Err(e) => println!("[FS] Failed to open /disk/README.TXT: {:?}", e),
    }

    // Test 2: Read pre-populated NOTES.TXT
    match VFS.open("/disk/NOTES.TXT", OpenFlags::READ_ONLY) {
        Ok(file) => {
            let mut buf = [0u8; 128];
            match file.read_at(0, &mut buf) {
                Ok(n) => {
                    let content = core::str::from_utf8(&buf[..n]).unwrap_or("");
                    println!("[FS] Read {} bytes from /disk/NOTES.TXT: {:?}", n, content.trim());
                }
                Err(e) => println!("[FS] Failed to read /disk/NOTES.TXT: {:?}", e),
            }
        }
        Err(e) => println!("[FS] Failed to open /disk/NOTES.TXT: {:?}", e),
    }

    // Test 3: Write a new file to FAT32 disk and read it back.
    // The OpenFlags here combine create (O_CREAT), read+write (O_RDWR), and
    // truncate (O_TRUNC) so the file is freshly created / emptied each boot.
    let test_data = b"NullOS FAT32 Persistent File Write Test Success!\n";
    match VFS.open("/disk/TEST_WR.TXT", OpenFlags {
        read: true,
        write: true,
        create: true,       // O_CREAT  – allocate a new dir entry if not found
        append: false,
        truncate: true,     // O_TRUNC  – reset to 0 bytes if file already existed
        nonblock: false,
    }) {
        Ok(file) => {
            match file.write_at(0, test_data) {
                Ok(written) => println!("[FS] Wrote {} bytes to /disk/TEST_WR.TXT on persistent storage.", written),
                Err(e) => println!("[FS] Failed to write to /disk/TEST_WR.TXT: {:?}", e),
            }
        }
        Err(e) => println!("[FS] Failed to create /disk/TEST_WR.TXT: {:?}", e),
    }

    println!("[FS] FAT32 verification complete.");
}

/// Runs automated verification self-tests for M15 Initramfs & M16 VFS.
///
/// This is a comprehensive integration test that covers all major subsystems of
/// the filesystem layer.  It is intended to be called once from the kernel's main
/// initialization routine after both [`init`] and [`mount_block_devices`] have
/// completed.
///
/// # Sub-tests (run in order)
///
/// | # | Function | What it tests |
/// |---|----------|---------------|
/// | 1 | [`test_tar_parser`]            | USTAR archive parsing (synthetic hand-crafted 512-byte blocks with correct checksum computation) |
/// | 2 | [`test_vfs_path_resolution`]   | VFS path traversal – reads `/hello.txt` and `/etc/os-release`, asserts content correctness |
/// | 3 | [`test_vfs_file_create_and_write`] | Dynamic file creation in `/tmp`, write, re-read and byte-for-byte comparison |
/// | 4 | [`test_file_descriptor_table`] | FD table: stdin/stdout/stderr presence, fd allocation, reading through fd, seek, close |
///
/// # Panics
///
/// Each sub-test uses `assert!` / `assert_eq!` with descriptive messages.  Any
/// assertion failure will panic the kernel, which is the desired behaviour during
/// development – a failing self-test means the kernel is in an inconsistent state
/// and should not continue booting.
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

/// Self-test 1: Validates the USTAR/TAR archive parser end-to-end.
///
/// ## Strategy
///
/// Rather than relying on an external TAR file baked into the binary, this
/// function **builds a syntactically valid synthetic TAR archive in memory** and
/// then passes it to [`TarArchive`].  This keeps the test fully self-contained
/// and verifiable without any disk I/O.
///
/// ## Synthetic archive layout
///
/// ```text
/// ┌──────────────────────────────────────────┐
/// │  Header block (512 bytes)                │
/// │   bytes   0..100  name = "test_embedded.txt" (NUL-padded) │
/// │   bytes 100..108  mode = "0000644\0"     │
/// │   bytes 108..116  uid  = "0000000\0"     │
/// │   bytes 116..124  gid  = "0000000\0"     │
/// │   bytes 124..136  size = octal(40) with trailing space │
/// │   bytes 136..148  mtime = "14000000000\0" │
/// │   byte  156       typeflag = '0' (normal file) │
/// │   bytes 257..263  magic = "ustar\0"      │
/// │   bytes 263..265  version = "00"         │
/// │   bytes 148..156  checksum (computed last, 6-octal + "\0 ") │
/// ├──────────────────────────────────────────┤
/// │  Data block: file content (40 bytes)     │
/// │  Padding to 512-byte boundary            │
/// ├──────────────────────────────────────────┤
/// │  End-of-archive marker: 1024 zero bytes  │
/// └──────────────────────────────────────────┘
/// ```
///
/// ## Checksum algorithm
///
/// The USTAR checksum is the unsigned sum of all 512 header bytes, with the
/// 8 checksum bytes (positions 148–155) treated as ASCII spaces (0x20) during
/// computation.  The result is stored back as a 6-digit octal string followed
/// by `'\0'` and `' '`.
fn test_tar_parser() {
    // Construct a synthetic 512-byte POSIX USTAR block for testing
    let mut synthetic_tar = Vec::new();
    // The content of our synthetic file – 40 bytes including the newline.
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
    // format! encodes the length as an 11-digit octal string followed by a space
    // (the TAR spec allows either NUL or space as the terminator).
    let size_octal = alloc::format!("{:011o} ", file_content.len());
    header[124..136].copy_from_slice(size_octal.as_bytes());
    // Mtime
    header[136..148].copy_from_slice(b"14000000000\0");
    // Typeflag: '0' (Normal file)
    header[156] = b'0';
    // Magic: "ustar\0"
    header[257..263].copy_from_slice(b"ustar\0");
    // Version: "00" (POSIX USTAR)
    header[263..265].copy_from_slice(b"00");

    // Compute checksum (sum of all bytes with checksum field set to spaces).
    // The checksum field (bytes 148–155) is treated as 8 spaces during summation
    // per the POSIX USTAR specification.
    let mut sum: u32 = 0;
    for (i, &b) in header.iter().enumerate() {
        if (148..156).contains(&i) {
            sum += b' ' as u32; // treat checksum field as spaces
        } else {
            sum += b as u32;
        }
    }
    // Write the computed checksum back as "OOOOOO\0 " (6 octal digits, NUL, space).
    let chksum_str = alloc::format!("{:06o}\0 ", sum);
    header[148..156].copy_from_slice(chksum_str.as_bytes());

    // Assemble the archive: header + data + padding + 2×zero blocks.
    synthetic_tar.extend_from_slice(&header);
    synthetic_tar.extend_from_slice(file_content);

    // Add padding to 512-byte boundary.
    // Formula: (512 - size % 512) % 512
    // The outer % 512 handles the special case where size is an exact multiple
    // of 512 (in which case no padding is needed, not 512 bytes of padding).
    let padding = (512 - (file_content.len() % 512)) % 512;
    synthetic_tar.extend_from_slice(&vec![0u8; padding]);

    // Add two 512-byte zero blocks (end of archive marker per POSIX USTAR spec).
    synthetic_tar.extend_from_slice(&[0u8; 1024]);

    // Parse the synthetic archive and validate the extracted entry.
    let archive = TarArchive::new(&synthetic_tar);
    let mut count = 0;
    for entry in archive {
        assert_eq!(entry.name, "test_embedded.txt");
        assert_eq!(entry.size, file_content.len());
        assert_eq!(entry.entry_type, TarType::NormalFile);
        // Zero-copy: entry.data is a slice into synthetic_tar, not a copy.
        assert_eq!(entry.data, file_content);
        count += 1;
    }
    // Exactly one entry should have been extracted.
    assert_eq!(count, 1, "TAR archive parser failed to extract test entry");
    println!("[FS] TAR Archive parser verified: extracted 1 file ({} bytes)", file_content.len());
}

/// Self-test 2: Validates VFS path traversal and file reading.
///
/// This test exercises [`vfs::Vfs::resolve_path`] by looking up two files that
/// were created by [`initramfs::RamFs::populate_defaults`] and verifying their
/// content:
///
/// * `/hello.txt` – root-level greeting file; must contain the literal string
///   `"Hello from NullOS Initramfs VFS!"`.
/// * `/etc/os-release` – OS identity file; must contain `"ID=nullos"`.
///
/// Both checks confirm that the VFS's longest-prefix mount resolution, relative
/// path computation, and directory-segment walking all work correctly for files
/// nested at different depths in the tree.
fn test_vfs_path_resolution() {
    // Verify reading /hello.txt (a root-level file in RamFs).
    let node = VFS.resolve_path("/hello.txt").expect("VFS failed to resolve /hello.txt");
    let mut buf = [0u8; 64];
    let bytes_read = node.read_at(0, &mut buf).expect("Failed to read /hello.txt");
    let content = core::str::from_utf8(&buf[..bytes_read]).unwrap();
    assert!(content.contains("Hello from NullOS Initramfs VFS!"));

    // Verify reading /etc/os-release (a file inside a subdirectory).
    let etc_node = VFS.resolve_path("/etc/os-release").expect("VFS failed to resolve /etc/os-release");
    let bytes_read_etc = etc_node.read_at(0, &mut buf).expect("Failed to read /etc/os-release");
    let etc_content = core::str::from_utf8(&buf[..bytes_read_etc]).unwrap();
    assert!(etc_content.contains("ID=nullos"));

    println!("[FS] VFS path resolution verified: read /hello.txt and /etc/os-release successfully.");
}

/// Self-test 3: Validates dynamic file creation, writing, and re-reading in RamFS.
///
/// This test exercises [`vfs::Vfs::open`] with the `create` flag set, which
/// triggers the create-on-not-found code path:
///
/// 1. The VFS first attempts to resolve `/tmp/test.txt`.  It doesn't exist yet,
///    so `resolve_path` returns `Err(FsError::NotFound)`.
/// 2. Because `flags.create == true`, the VFS looks up the **parent** directory
///    `/tmp` and calls `parent_inode.create("test.txt", FileType::File)`.
/// 3. The new `RamFile` is returned.
/// 4. We write the test payload to offset 0.  `write_at` on `RamFile` resizes the
///    backing `Vec<u8>` to accommodate the data.
/// 5. We re-resolve `/tmp/test.txt` through the VFS (a second `resolve_path` call)
///    to confirm the file is correctly registered in the directory's children map.
/// 6. We read back the data and compare byte-for-byte.
fn test_vfs_file_create_and_write() {
    // Open / create a new file in /tmp/test.txt
    let new_file = VFS
        .open("/tmp/test.txt", OpenFlags {
            read: true,
            write: true,
            create: true,   // triggers parent.create("test.txt") if not found
            append: false,
            truncate: false,
            nonblock: false,
        })
        .expect("Failed to create /tmp/test.txt");

    let write_data = b"Testing dynamic file creation and writing in NullOS RamFS!";
    let written = new_file.write_at(0, write_data).expect("Failed to write to /tmp/test.txt");
    assert_eq!(written, write_data.len());

    // Verify written content by re-reading through VFS.
    // This confirms the new inode is properly registered in the directory tree.
    let read_node = VFS.resolve_path("/tmp/test.txt").expect("Failed to re-lookup /tmp/test.txt");
    let mut read_buf = [0u8; 128];
    let n = read_node.read_at(0, &mut read_buf).expect("Failed to read /tmp/test.txt");
    assert_eq!(&read_buf[..n], write_data);

    println!("[FS] VFS file creation, write, and read verified: {} bytes.", written);
}

/// Self-test 4: Validates the FileDescriptorTable and standard stream infrastructure.
///
/// This test exercises the full lifecycle of a file descriptor:
///
/// 1. **Standard streams exist** – A newly constructed `FileDescriptorTable` must
///    have entries at indices 0 (stdin), 1 (stdout), and 2 (stderr), each backed
///    by a [`ConsoleDevice`](initramfs::ConsoleDevice).
///
/// 2. **FD allocation** – Opening `/hello.txt` and allocating a descriptor through
///    `alloc_fd` must return index 3 (the lowest unused index after 0, 1, 2).
///
/// 3. **Read through FD** – `handle.read(&mut buf)` must return 16 bytes starting
///    with `"Hello"`, confirming that read() delegates to inode.read_at() and
///    advances the offset.
///
/// 4. **Seek** – `handle.seek(0, SEEK_SET)` must reset the offset to 0 and
///    return 0.  A second read must yield the same first 16 bytes as step 3.
///
/// 5. **Close** – `fd_table.close(3)` must succeed, and a subsequent `get(3)`
///    must return `Err(BadFileDescriptor)`.
fn test_file_descriptor_table() {
    // Construct a fresh FD table.  The constructor pre-populates slots 0, 1, 2
    // with ConsoleDevice descriptors (stdin/stdout/stderr).
    let fd_table = FileDescriptorTable::new();

    // Verify standard descriptors 0, 1, 2 exist
    assert!(fd_table.get(0).is_ok(), "stdin missing");
    assert!(fd_table.get(1).is_ok(), "stdout missing");
    assert!(fd_table.get(2).is_ok(), "stderr missing");

    // Open file and allocate a new descriptor.
    // We resolve /hello.txt directly to get an Arc<dyn Inode>.
    let node = VFS.resolve_path("/hello.txt").unwrap();
    // Wrap it in a FileDescriptor with READ_WRITE access flags and an offset of 0.
    let desc = Arc::new(FileDescriptor::new(node, OpenFlags::READ_WRITE));
    // alloc_fd scans for the lowest None slot (slot 3, since 0–2 are occupied).
    let fd = fd_table.alloc_fd(desc).expect("Failed to allocate file descriptor");
    assert_eq!(fd, 3, "First allocated fd should be 3");

    // Read through the file descriptor.
    // fd_table.get(fd) returns Arc<FileDescriptor>; read() locks the offset Spinlock,
    // calls inode.read_at(current_offset, buf), then advances offset by bytes_read.
    let handle = fd_table.get(fd).unwrap();
    let mut buf = [0u8; 16];
    let n = handle.read(&mut buf).unwrap();
    assert_eq!(n, 16);
    assert_eq!(&buf[..5], b"Hello");

    // Seek back to start.
    // SEEK_SET = 0 means: new offset = `offset` argument (0 → beginning of file).
    let pos = handle.seek(0, SEEK_SET).unwrap();
    assert_eq!(pos, 0);

    // Re-read – the same 16 bytes must come back after seeking to 0.
    let n2 = handle.read(&mut buf).unwrap();
    assert_eq!(n2, 16);
    assert_eq!(&buf[..5], b"Hello");

    // Close descriptor.
    // close() sets fds[3] = None, dropping the Arc<FileDescriptor>.  If this was
    // the last Arc pointing at the descriptor, the FileDescriptor is dropped and
    // its inode Arc is decremented (but /hello.txt stays alive in RamFs).
    fd_table.close(fd).expect("Failed to close fd");
    // After close, get(3) should return Err(BadFileDescriptor).
    assert!(fd_table.get(fd).is_err(), "fd should be closed");

    println!("[FS] FileDescriptorTable verified: stdin/out/err, fd allocation, seek, and close.");
}
