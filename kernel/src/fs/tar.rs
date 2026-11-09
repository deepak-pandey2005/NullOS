//! POSIX USTAR Archive Parser (Phase 6 - M15 Initramfs).
//!
//! # Architecture & Format
//!
//! The POSIX Uniform Standard Tape Archive (USTAR) format stores files sequentially
//! in 512-byte blocks. Each archive entry begins with a 512-byte header describing
//! the file (filename, size, file permissions, ownership, and entry type).
//!
//! ## Header Structure (512 bytes):
//! - `0..100`: File name path (NUL-terminated ASCII string).
//! - `100..108`: File mode permissions in octal ASCII.
//! - `108..116`: Owner user ID (UID) in octal ASCII.
//! - `116..124`: Group ID (GID) in octal ASCII.
//! - `124..136`: File size in octal ASCII bytes.
//! - `136..148`: Modification timestamp in octal ASCII.
//! - `148..156`: Checksum over entire header (computed assuming checksum field is space characters).
//! - `156`: Type flag character (`'0'` = normal file, `'5'` = directory).
//! - `257..263`: USTAR magic string (`"ustar\0"` or `"ustar "`).
//!
//! ## Zero-Copy Design
//! This parser extracts file slices directly referencing the underlying read-only
//! memory buffer, avoiding unnecessary heap allocations and buffer copies during
//! early kernel boot.

use core::str;

/// Standard POSIX USTAR magic string identifier.
#[allow(dead_code)]
pub const USTAR_MAGIC: &[u8; 6] = b"ustar\0";
/// Alternate GNU tar magic string variant.
#[allow(dead_code)]
pub const USTAR_MAGIC_ALT: &[u8; 6] = b"ustar ";

/// Type flag representing the nature of an archive entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TarType {
    /// Regular byte file content.
    NormalFile,
    /// Directory container.
    Directory,
    /// Hard link reference.
    HardLink,
    /// Symbolic link reference.
    SymLink,
    /// Character special device node.
    CharacterDevice,
    /// Block special storage device node.
    BlockDevice,
    /// FIFO / Named pipe.
    Fifo,
    /// Unrecognized or vendor-specific typeflag byte.
    Unknown(u8),
}

/// A parsed TAR entry pointing directly into the archive memory slice (zero-copy).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TarEntry<'a> {
    /// Extracted file or directory path name.
    pub name: &'a str,
    /// File size in bytes.
    pub size: usize,
    /// POSIX file permission mode.
    pub mode: u32,
    /// Classification of entry (file, directory, link, etc.).
    pub entry_type: TarType,
    /// Direct byte slice of file payload starting after the 512-byte header.
    pub data: &'a [u8],
}

/// TAR Archive reader iterating over contiguous 512-byte blocks.
pub struct TarArchive<'a> {
    /// Backing memory buffer holding the raw TAR archive.
    pub data: &'a [u8],
    /// Current byte offset into the archive.
    offset: usize,
}

#[allow(dead_code)]
impl<'a> TarArchive<'a> {
    /// Creates a new TAR archive reader wrapping raw archive bytes.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    /// Resets the iterator back to the beginning of the archive.
    pub fn reset(&mut self) {
        self.offset = 0;
    }

    /// Searches for a file in the archive by exact normalized path name.
    pub fn find(&self, path: &str) -> Option<TarEntry<'a>> {
        let mut archive = TarArchive::new(self.data);
        let trimmed_query = path.trim_start_matches('/');
        for entry in &mut archive {
            let entry_name = entry.name.trim_start_matches('/');
            if entry_name == trimmed_query || entry_name.trim_end_matches('/') == trimmed_query {
                return Some(entry);
            }
        }
        None
    }
}

impl<'a> Iterator for TarArchive<'a> {
    type Item = TarEntry<'a>;

    /// Parses the next 512-byte USTAR block into a [`TarEntry`].
    ///
    /// Skips past the file content and aligns the cursor to the subsequent 512-byte boundary.
    /// Returns `None` when encountering the end-of-archive marker (all zeroes) or EOF.
    fn next(&mut self) -> Option<Self::Item> {
        while self.offset + 512 <= self.data.len() {
            let header = &self.data[self.offset..self.offset + 512];

            // If the header is all zeroes, it marks the end of the archive (or a gap)
            if header.iter().all(|&b| b == 0) {
                // Check if following 512 bytes are also zero (standard TAR end marker)
                self.offset += 512;
                return None;
            }

            // Extract file name (bytes 0..100)
            let raw_name = &header[0..100];
            let name_len = raw_name.iter().position(|&b| b == 0).unwrap_or(100);
            let name = str::from_utf8(&raw_name[..name_len]).unwrap_or("");

            // Extract file size in octal (bytes 124..136)
            let raw_size = &header[124..136];
            let size = parse_octal(raw_size).unwrap_or(0);

            // Extract file mode (bytes 100..108)
            let mode = parse_octal(&header[100..108]).unwrap_or(0) as u32;

            // Extract typeflag (byte 156)
            let type_byte = header[156];
            let entry_type = match type_byte {
                b'0' | 0 => TarType::NormalFile,
                b'1' => TarType::HardLink,
                b'2' => TarType::SymLink,
                b'3' => TarType::CharacterDevice,
                b'4' => TarType::BlockDevice,
                b'5' => TarType::Directory,
                b'6' => TarType::Fifo,
                other => TarType::Unknown(other),
            };

            // The data content starts immediately after the 512-byte header
            let data_start = self.offset + 512;
            let data_end = data_start + size;

            if data_end > self.data.len() {
                // Corrupted archive or truncated entry
                return None;
            }

            let entry_data = &self.data[data_start..data_end];

            // In TAR, entries are padded to 512-byte block boundaries
            let padding = (512 - (size % 512)) % 512;
            self.offset = data_end + padding;

            if !name.is_empty() {
                return Some(TarEntry {
                    name,
                    size,
                    mode,
                    entry_type,
                    data: entry_data,
                });
            }
        }

        None
    }
}

/// Parses an ASCII octal string (NUL or space terminated) into a numeric `usize`.
///
/// Returns `None` if illegal characters or arithmetic overflow occur.
pub fn parse_octal(bytes: &[u8]) -> Option<usize> {
    let mut val: usize = 0;
    let mut started = false;

    for &b in bytes {
        if b == 0 || b == b' ' {
            if started {
                break;
            } else {
                continue;
            }
        }
        if (b'0'..=b'7').contains(&b) {
            started = true;
            val = val.checked_mul(8)?.checked_add((b - b'0') as usize)?;
        } else {
            return None;
        }
    }

    if started {
        Some(val)
    } else {
        Some(0)
    }
}
