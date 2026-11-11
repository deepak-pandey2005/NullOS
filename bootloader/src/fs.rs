//! UEFI File System Protocols and Kernel Loading.
//!
//! # Overview
//! Uses UEFI Simple File System protocols to open the boot partition (typically a FAT32
//! EFI System Partition), query file metadata, allocate pool memory, and read raw binary
//! payloads into memory before `exit_boot_services`.

use core::slice;
use uefi::boot::{allocate_pool, image_handle, MemoryType};
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode};
use uefi::{cstr16, CStr16};

/// Reads an entire file from the boot partition volume into an allocated heap buffer.
///
/// # Workflow
/// 1. Locates the `SimpleFileSystem` protocol associated with the bootloader's image handle.
/// 2. Opens the root volume directory.
/// 3. Opens the target file in read-only mode using a UCS-2 / UTF-16 path string (`CStr16`).
/// 4. Converts the file handle to a regular file and queries `FileInfo` to ascertain exact byte size.
/// 5. Allocates a contiguous pool of `MemoryType::LOADER_DATA` matching the file size.
/// 6. Performs chunked reads until all bytes are buffered.
///
/// # Returns
/// - `Ok(&'static [u8])` containing the full file contents.
/// - `Err(&'static str)` if the file cannot be opened, has size 0, or read fails.
pub fn load_file(path: &CStr16) -> Result<&'static [u8], &'static str> {
    // Step 1: Obtain file system protocol instance associated with our loaded boot image
    let mut fs = uefi::boot::get_image_file_system(image_handle())
        .map_err(|_| "Failed to get image file system")?;

    // Step 2: Open the root directory volume of the EFI System Partition
    let mut root = fs.open_volume()
        .map_err(|_| "Failed to open root volume")?;

    // Step 3: Open the requested file path in read-only mode
    let handle = root.open(path, FileMode::Read, FileAttribute::empty())
        .map_err(|_| "Failed to open file on filesystem")?;

    // Step 4: Verify the opened handle corresponds to a regular file (not a subdirectory)
    let mut file = handle.into_regular_file()
        .ok_or("Opened file is not a regular file")?;

    // Step 5: Query file metadata to determine exact byte size
    let mut info_buf = [0u8; 512];
    let file_info = file.get_info::<FileInfo>(&mut info_buf)
        .map_err(|_| "Failed to get file info")?;

    let file_size = file_info.file_size() as usize;
    if file_size == 0 {
        return Err("File size is 0 bytes");
    }

    // Step 6: Allocate contiguous loader pool memory to store the file
    let buf_ptr = allocate_pool(MemoryType::LOADER_DATA, file_size)
        .map_err(|_| "Failed to allocate memory pool for file")?;

    let buf = unsafe { slice::from_raw_parts_mut(buf_ptr.as_ptr(), file_size) };

    // Step 7: Stream all bytes from the file into the buffer
    let mut total_read = 0;
    while total_read < file_size {
        let read = file.read(&mut buf[total_read..])
            .map_err(|_| "Failed to read file data")?;
        if read == 0 {
            break;
        }
        total_read += read;
    }

    if total_read != file_size {
        return Err("Incomplete file read");
    }

    Ok(buf)
}

/// Converts an ASCII path string into a stack-allocated UCS-2 (`CStr16`) path and loads the file.
///
/// Forward slashes (`/`) are normalized into UEFI FAT32 backslashes (`\`), and any leading
/// slash/backslash is stripped so relative ESP volume opens resolve uniformly.
pub fn load_file_ascii(path: &str) -> Result<&'static [u8], &'static str> {
    let trimmed = path.trim().trim_start_matches(['\\', '/']);
    if trimmed.is_empty() || trimmed.len() >= 127 {
        return Err("Invalid or overly long path");
    }
    let mut u16_buf = [0u16; 128];
    let mut idx = 0usize;
    for b in trimmed.bytes() {
        let ch = if b == b'/' { b'\\' } else { b };
        if !ch.is_ascii() {
            return Err("Non-ASCII path unsupported");
        }
        u16_buf[idx] = ch as u16;
        idx += 1;
    }
    u16_buf[idx] = 0;
    let cstr = CStr16::from_u16_with_nul(&u16_buf[..=idx])
        .map_err(|_| "Failed to encode UCS-2 path")?;
    load_file(cstr)
}

/// Locates and loads the kernel binary from a configured path or standard ESP locations.
///
/// # Search Hierarchy
/// 1. Optional path specified in `nullos.cfg` (`kernel=...`)
/// 2. Root volume path: `\kernel.elf`
/// 3. ESP Boot directory path: `\EFI\BOOT\kernel.elf`
///
/// # Returns
/// - `Ok(&'static [u8])` with raw kernel ELF binary data.
/// - `Err(&'static str)` if `kernel.elf` cannot be found in any search location.
pub fn load_kernel_file(preferred_path: Option<&str>) -> Result<&'static [u8], &'static str> {
    if let Some(path) = preferred_path {
        if let Ok(data) = load_file_ascii(path) {
            return Ok(data);
        }
    }
    // Check root directory first, then fallback to EFI\BOOT\
    if let Ok(data) = load_file(cstr16!("kernel.elf")) {
        return Ok(data);
    }
    if let Ok(data) = load_file(cstr16!("EFI\\BOOT\\kernel.elf")) {
        return Ok(data);
    }
    Err("Could not find kernel.elf in \\ or \\EFI\\BOOT\\")
}

/// Optionally loads an `initramfs.tar` USTAR archive from the configured path or standard ESP paths.
///
/// Returns `Some(&'static [u8])` if present on the EFI System Partition, or `None` if absent.
pub fn load_initramfs_file(preferred_path: Option<&str>) -> Option<&'static [u8]> {
    if let Some(path) = preferred_path {
        if let Ok(data) = load_file_ascii(path) {
            return Some(data);
        }
    }
    if let Ok(data) = load_file(cstr16!("initramfs.tar")) {
        return Some(data);
    }
    if let Ok(data) = load_file(cstr16!("EFI\\BOOT\\initramfs.tar")) {
        return Some(data);
    }
    None
}
