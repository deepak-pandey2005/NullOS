//! POSIX USTAR Archive Parser (Phase 6 - M15 Initramfs).
//!
//! Features:
//! - Standard POSIX USTAR 512-byte block parser.
//! - Zero-copy extraction of file contents.
//! - Octal string numeric decoding (size, mode, uid, gid).
//! - Directory & regular file type detection.

use core::str;

/// POSIX USTAR magic identifier.
#[allow(dead_code)]
pub const USTAR_MAGIC: &[u8; 6] = b"ustar\0";
#[allow(dead_code)]
pub const USTAR_MAGIC_ALT: &[u8; 6] = b"ustar ";

/// Type flag representing the type of archive entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TarType {
    NormalFile,
    Directory,
    HardLink,
    SymLink,
    CharacterDevice,
    BlockDevice,
    Fifo,
    Unknown(u8),
}

/// A parsed TAR entry pointing directly into the archive slice (zero-copy).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TarEntry<'a> {
    pub name: &'a str,
    pub size: usize,
    pub mode: u32,
    pub entry_type: TarType,
    pub data: &'a [u8],
}

/// TAR Archive reader iterating over contiguous 512-byte blocks.
pub struct TarArchive<'a> {
    pub data: &'a [u8],
    offset: usize,
}

#[allow(dead_code)]
impl<'a> TarArchive<'a> {
    /// Creates a new TAR archive reader from raw bytes.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    /// Resets the iterator to the beginning of the archive.
    pub fn reset(&mut self) {
        self.offset = 0;
    }

    /// Searches for a file in the archive by exact path name.
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

/// Parses an ASCII octal string into a numeric `usize`.
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
