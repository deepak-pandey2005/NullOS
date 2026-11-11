//! # Block Cache & Write-Back Cache (Phase 15 - M35)
//!
//! ## Overview
//! Provides an LRU-evicting, sector-granular block cache layered over any [`BlockDevice`].
//! Supports both **Write-Through** and **Write-Back** caching policies:
//! - **Write-Through**: Writes update the cache entry and immediately synchronize to the
//!   underlying storage device.
//! - **Write-Back**: Writes modify cached RAM buffers and mark the block as `Dirty`.
//!   Actual hardware disk I/O is deferred until explicit `flush()` / `sync()`, or when
//!   capacity pressure forces LRU eviction of a dirty block.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::drivers::block::{BlockDevice, BlockError};
use crate::sync::Spinlock;

/// Caching policy governing how writes propagate to the backing device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePolicy {
    /// Writes immediately synchronize to underlying storage.
    WriteThrough,
    /// Writes buffer in memory marked Dirty and flush lazily or on sync/eviction.
    WriteBack,
}

/// Snapshot statistics of a [`BlockCache`] instance.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockCacheStats {
    pub capacity_blocks: usize,
    pub used_blocks: usize,
    pub dirty_blocks: usize,
    pub hits: u64,
    pub misses: u64,
    pub writebacks: u64,
    pub evictions: u64,
    pub flushes: u64,
}

/// Single cached sector entry inside the LRU block cache.
struct CachedBlock {
    data: Vec<u8>,
    dirty: bool,
    last_access: u64,
}

/// Internal state protected by a spinlock.
struct BlockCacheInner {
    capacity: usize,
    policy: CachePolicy,
    entries: BTreeMap<u64, CachedBlock>,
    access_seq: u64,
    hits: u64,
    misses: u64,
    writebacks: u64,
    evictions: u64,
    flushes: u64,
}

/// Thread-safe LRU Block & Write-Back Cache wrapping any [`BlockDevice`].
pub struct CachedBlockDevice {
    name: String,
    inner_dev: Arc<dyn BlockDevice>,
    cache: Spinlock<BlockCacheInner>,
}

impl CachedBlockDevice {
    /// Creates a new cached wrapper around `inner_dev` with the given capacity (in sectors)
    /// and caching policy.
    pub fn new(inner_dev: Arc<dyn BlockDevice>, capacity_blocks: usize, policy: CachePolicy) -> Self {
        let cap = core::cmp::max(capacity_blocks, 4);
        let name = alloc::format!("{}-cached", inner_dev.name());
        Self {
            name,
            inner_dev,
            cache: Spinlock::new(BlockCacheInner {
                capacity: cap,
                policy,
                entries: BTreeMap::new(),
                access_seq: 0,
                hits: 0,
                misses: 0,
                writebacks: 0,
                evictions: 0,
                flushes: 0,
            }),
        }
    }

    /// Dynamically updates the cache policy (flushing dirty blocks if switching to WriteThrough).
    pub fn set_policy(&self, policy: CachePolicy) -> Result<(), BlockError> {
        if policy == CachePolicy::WriteThrough {
            self.flush()?;
        }
        self.cache.lock().policy = policy;
        Ok(())
    }

    /// Returns the current cache policy.
    pub fn policy(&self) -> CachePolicy {
        self.cache.lock().policy
    }

    /// Returns current cache telemetry counters.
    pub fn stats(&self) -> BlockCacheStats {
        let guard = self.cache.lock();
        let dirty_blocks = guard.entries.values().filter(|e| e.dirty).count();
        BlockCacheStats {
            capacity_blocks: guard.capacity,
            used_blocks: guard.entries.len(),
            dirty_blocks,
            hits: guard.hits,
            misses: guard.misses,
            writebacks: guard.writebacks,
            evictions: guard.evictions,
            flushes: guard.flushes,
        }
    }

    /// Invalidates all clean blocks and clears cache statistics without writing back.
    /// Dirty blocks are written back first so no data is lost.
    pub fn purge(&self) -> Result<(), BlockError> {
        self.flush()?;
        let mut guard = self.cache.lock();
        guard.entries.clear();
        Ok(())
    }

    /// Evicts the least-recently-used entry if the cache is at full capacity.
    /// Returns an error only if flushing a dirty victim to disk fails.
    fn evict_if_full(&self, guard: &mut BlockCacheInner) -> Result<(), BlockError> {
        if guard.entries.len() < guard.capacity {
            return Ok(());
        }

        // Find LRU sector key
        let mut victim_sector: Option<u64> = None;
        let mut oldest_seq = u64::MAX;
        for (&sec, entry) in guard.entries.iter() {
            if entry.last_access < oldest_seq {
                oldest_seq = entry.last_access;
                victim_sector = Some(sec);
            }
        }

        if let Some(sec) = victim_sector {
            if let Some(entry) = guard.entries.remove(&sec) {
                guard.evictions += 1;
                if entry.dirty {
                    self.inner_dev.write_sector(sec, &entry.data)?;
                    guard.writebacks += 1;
                }
            }
        }
        Ok(())
    }
}

impl BlockDevice for CachedBlockDevice {
    fn name(&self) -> &str {
        &self.name
    }

    fn sector_size(&self) -> usize {
        self.inner_dev.sector_size()
    }

    fn sector_count(&self) -> u64 {
        self.inner_dev.sector_count()
    }

    /// Reads a sector, returning the cached copy on hit or fetching from hardware on miss.
    fn read_sector(&self, sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let sec_size = self.inner_dev.sector_size();
        if buf.len() < sec_size {
            return Err(BlockError::BufferTooSmall);
        }
        if sector >= self.inner_dev.sector_count() {
            return Err(BlockError::InvalidSector);
        }

        let mut guard = self.cache.lock();
        guard.access_seq += 1;
        let seq = guard.access_seq;

        if let Some(entry) = guard.entries.get_mut(&sector) {
            entry.last_access = seq;
            buf[..sec_size].copy_from_slice(&entry.data[..sec_size]);
            guard.hits += 1;
            return Ok(());
        }

        // Cache miss: fetch from underlying device
        guard.misses += 1;
        let mut sector_data = vec![0u8; sec_size];
        self.inner_dev.read_sector(sector, &mut sector_data)?;

        self.evict_if_full(&mut guard)?;
        buf[..sec_size].copy_from_slice(&sector_data[..sec_size]);
        guard.entries.insert(
            sector,
            CachedBlock {
                data: sector_data,
                dirty: false,
                last_access: seq,
            },
        );
        Ok(())
    }

    /// Writes a sector according to the active `CachePolicy` (WriteThrough vs WriteBack).
    fn write_sector(&self, sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        let sec_size = self.inner_dev.sector_size();
        if buf.len() < sec_size {
            return Err(BlockError::BufferTooSmall);
        }
        if sector >= self.inner_dev.sector_count() {
            return Err(BlockError::InvalidSector);
        }

        let mut guard = self.cache.lock();
        guard.access_seq += 1;
        let seq = guard.access_seq;
        let policy = guard.policy;

        if policy == CachePolicy::WriteThrough {
            self.inner_dev.write_sector(sector, &buf[..sec_size])?;
        }

        if let Some(entry) = guard.entries.get_mut(&sector) {
            entry.data[..sec_size].copy_from_slice(&buf[..sec_size]);
            entry.last_access = seq;
            if policy == CachePolicy::WriteBack {
                entry.dirty = true;
            } else {
                entry.dirty = false;
            }
            guard.hits += 1;
            return Ok(());
        }

        guard.misses += 1;
        self.evict_if_full(&mut guard)?;
        let mut sector_data = vec![0u8; sec_size];
        sector_data.copy_from_slice(&buf[..sec_size]);
        guard.entries.insert(
            sector,
            CachedBlock {
                data: sector_data,
                dirty: policy == CachePolicy::WriteBack,
                last_access: seq,
            },
        );
        Ok(())
    }

    /// Flushes all dirty blocks in the write-back cache out to the underlying block device.
    fn flush(&self) -> Result<(), BlockError> {
        let mut guard = self.cache.lock();
        let dirty_keys: Vec<u64> = guard
            .entries
            .iter()
            .filter_map(|(&k, v)| if v.dirty { Some(k) } else { None })
            .collect();

        for sec in dirty_keys {
            if let Some(entry) = guard.entries.get_mut(&sec) {
                self.inner_dev.write_sector(sec, &entry.data)?;
                entry.dirty = false;
                guard.writebacks += 1;
            }
        }
        guard.flushes += 1;
        self.inner_dev.flush()
    }
}

/// Global reference to the primary cached block device when initialized.
pub static PRIMARY_BLOCK_CACHE: Spinlock<Option<Arc<CachedBlockDevice>>> = Spinlock::new(None);
