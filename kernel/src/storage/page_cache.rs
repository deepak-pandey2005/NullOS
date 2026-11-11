//! # File-Backed VFS Page Cache (Phase 15 - M35)
//!
//! ## Overview
//! Implements a 4 KiB (`PAGE_SIZE_4K`) kernel page cache indexed by `(inode_id, page_index)`.
//! The Page Cache sits between VFS read/write operations and underlying filesystem drivers,
//! allowing repeated file reads, sequential writes, and `mmap`-style page lookups to be
//! served directly from RAM without redundant disk transfers.
//!
//! ### Cache Coherency & Eviction
//! - **Dirty Tracking**: Modified file pages are marked `dirty = true` and buffered in RAM.
//! - **LRU Replacement**: When the cache reaches `max_pages`, the least-recently-accessed
//!   unpinned page is selected for eviction; if dirty, it is written back before removal.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::drivers::block::BlockError;
use crate::memory::PAGE_SIZE_4K;
use crate::sync::Spinlock;

const PAGE_SIZE: usize = PAGE_SIZE_4K as usize;

/// Unique key identifying a 4 KiB file page in the page cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PageKey {
    pub inode_id: u64,
    pub page_index: u64,
}

/// Statistics snapshot of the global Page Cache.
#[derive(Debug, Clone, Copy, Default)]
pub struct PageCacheStats {
    pub capacity_pages: usize,
    pub cached_pages: usize,
    pub dirty_pages: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub writebacks: u64,
}

struct CachedPage {
    data: Vec<u8>,
    dirty: bool,
    last_access: u64,
    pin_count: u32,
}

struct PageCacheInner {
    capacity_pages: usize,
    pages: BTreeMap<PageKey, CachedPage>,
    access_seq: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    writebacks: u64,
}

/// Global thread-safe 4 KiB File Page Cache.
pub struct PageCache {
    inner: Spinlock<PageCacheInner>,
}

impl PageCache {
    /// Creates a new Page Cache holding up to `capacity_pages` 4 KiB pages.
    pub const fn new_empty(capacity_pages: usize) -> Self {
        Self {
            inner: Spinlock::new(PageCacheInner {
                capacity_pages,
                pages: BTreeMap::new(),
                access_seq: 0,
                hits: 0,
                misses: 0,
                evictions: 0,
                writebacks: 0,
            }),
        }
    }

    /// Reads an entire 4 KiB page for `(inode_id, page_index)`.
    ///
    /// On a cache hit, copies from RAM into `out_buf` and refreshes the LRU timestamp.
    /// On a cache miss, calls `loader` to fetch the page contents, caches it, and copies to `out_buf`.
    pub fn read_page<F>(
        &self,
        inode_id: u64,
        page_index: u64,
        out_buf: &mut [u8],
        mut loader: F,
    ) -> Result<(), BlockError>
    where
        F: FnMut(u64, u64, &mut [u8]) -> Result<(), BlockError>,
    {
        if out_buf.len() < PAGE_SIZE {
            return Err(BlockError::BufferTooSmall);
        }

        let key = PageKey { inode_id, page_index };
        let mut guard = self.inner.lock();
        guard.access_seq += 1;
        let seq = guard.access_seq;

        if let Some(page) = guard.pages.get_mut(&key) {
            page.last_access = seq;
            out_buf[..PAGE_SIZE].copy_from_slice(&page.data[..PAGE_SIZE]);
            guard.hits += 1;
            return Ok(());
        }

        guard.misses += 1;
        let mut page_buf = vec![0u8; PAGE_SIZE];
        loader(inode_id, page_index, &mut page_buf)?;

        Self::evict_clean_if_full(&mut guard);
        out_buf[..PAGE_SIZE].copy_from_slice(&page_buf);
        guard.pages.insert(
            key,
            CachedPage {
                data: page_buf,
                dirty: false,
                last_access: seq,
                pin_count: 0,
            },
        );
        Ok(())
    }

    /// Writes a byte slice into a cached 4 KiB page at `page_offset`, marking the page `dirty`.
    ///
    /// If the page is not currently cached, `loader` initializes the uncached page first
    /// (unless the write overwrites the entire 4 KiB page).
    pub fn write_page<L, W>(
        &self,
        inode_id: u64,
        page_index: u64,
        page_offset: usize,
        src: &[u8],
        mut loader: L,
        mut writeback_fn: W,
    ) -> Result<usize, BlockError>
    where
        L: FnMut(u64, u64, &mut [u8]) -> Result<(), BlockError>,
        W: FnMut(u64, u64, &[u8]) -> Result<(), BlockError>,
    {
        if page_offset >= PAGE_SIZE {
            return Err(BlockError::InvalidSector);
        }
        let write_len = core::cmp::min(src.len(), PAGE_SIZE - page_offset);
        let key = PageKey { inode_id, page_index };

        let mut guard = self.inner.lock();
        guard.access_seq += 1;
        let seq = guard.access_seq;

        if let Some(page) = guard.pages.get_mut(&key) {
            page.data[page_offset..page_offset + write_len].copy_from_slice(&src[..write_len]);
            page.dirty = true;
            page.last_access = seq;
            guard.hits += 1;
            return Ok(write_len);
        }

        guard.misses += 1;
        // Evict LRU entry (writing back if victim is dirty)
        if guard.pages.len() >= guard.capacity_pages {
            if let Some((victim_key, victim_page)) = Self::pop_lru_victim(&mut guard) {
                if victim_page.dirty {
                    writeback_fn(victim_key.inode_id, victim_key.page_index, &victim_page.data)?;
                    guard.writebacks += 1;
                }
            }
        }

        let mut page_buf = vec![0u8; PAGE_SIZE];
        if write_len < PAGE_SIZE {
            // Partial page write: read-modify-write via loader (ignore IoError for new pages)
            let _ = loader(inode_id, page_index, &mut page_buf);
        }
        page_buf[page_offset..page_offset + write_len].copy_from_slice(&src[..write_len]);

        guard.pages.insert(
            key,
            CachedPage {
                data: page_buf,
                dirty: true,
                last_access: seq,
                pin_count: 0,
            },
        );
        Ok(write_len)
    }

    /// Flushes all dirty pages belonging to `inode_id` using `writeback_fn`.
    pub fn sync_inode<W>(&self, inode_id: u64, mut writeback_fn: W) -> Result<usize, BlockError>
    where
        W: FnMut(u64, u64, &[u8]) -> Result<(), BlockError>,
    {
        let mut guard = self.inner.lock();
        let keys: Vec<PageKey> = guard
            .pages
            .iter()
            .filter_map(|(&k, p)| if k.inode_id == inode_id && p.dirty { Some(k) } else { None })
            .collect();

        let mut flushed = 0usize;
        for k in keys {
            if let Some(page) = guard.pages.get_mut(&k) {
                writeback_fn(k.inode_id, k.page_index, &page.data)?;
                page.dirty = false;
                guard.writebacks += 1;
                flushed += 1;
            }
        }
        Ok(flushed)
    }

    /// Flushes all dirty pages across every cached inode using `writeback_fn`.
    pub fn sync_all<W>(&self, mut writeback_fn: W) -> Result<usize, BlockError>
    where
        W: FnMut(u64, u64, &[u8]) -> Result<(), BlockError>,
    {
        let mut guard = self.inner.lock();
        let keys: Vec<PageKey> = guard
            .pages
            .iter()
            .filter_map(|(&k, p)| if p.dirty { Some(k) } else { None })
            .collect();

        let mut flushed = 0usize;
        for k in keys {
            if let Some(page) = guard.pages.get_mut(&k) {
                writeback_fn(k.inode_id, k.page_index, &page.data)?;
                page.dirty = false;
                guard.writebacks += 1;
                flushed += 1;
            }
        }
        Ok(flushed)
    }

    /// Invalidates all cached pages associated with `inode_id` (e.g., on file unlink or truncate).
    pub fn invalidate_inode(&self, inode_id: u64) -> usize {
        let mut guard = self.inner.lock();
        let keys: Vec<PageKey> = guard
            .pages
            .keys()
            .copied()
            .filter(|k| k.inode_id == inode_id)
            .collect();
        let count = keys.len();
        for k in keys {
            guard.pages.remove(&k);
        }
        count
    }

    /// Pins a cached page in memory so it will not be evicted during LRU replacement.
    pub fn pin_page(&self, inode_id: u64, page_index: u64) -> bool {
        let mut guard = self.inner.lock();
        if let Some(page) = guard.pages.get_mut(&PageKey { inode_id, page_index }) {
            page.pin_count = page.pin_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Releases a page pin count.
    pub fn unpin_page(&self, inode_id: u64, page_index: u64) {
        let mut guard = self.inner.lock();
        if let Some(page) = guard.pages.get_mut(&PageKey { inode_id, page_index }) {
            page.pin_count = page.pin_count.saturating_sub(1);
        }
    }

    /// Clears the entire page cache (used between tests or memory pressure shrinkers).
    pub fn clear(&self) {
        let mut guard = self.inner.lock();
        guard.pages.clear();
    }

    /// Returns telemetry statistics for the Page Cache.
    pub fn stats(&self) -> PageCacheStats {
        let guard = self.inner.lock();
        let dirty_pages = guard.pages.values().filter(|p| p.dirty).count();
        PageCacheStats {
            capacity_pages: guard.capacity_pages,
            cached_pages: guard.pages.len(),
            dirty_pages,
            hits: guard.hits,
            misses: guard.misses,
            evictions: guard.evictions,
            writebacks: guard.writebacks,
        }
    }

    fn pop_lru_victim(guard: &mut PageCacheInner) -> Option<(PageKey, CachedPage)> {
        let mut victim: Option<PageKey> = None;
        let mut oldest = u64::MAX;
        for (&k, p) in guard.pages.iter() {
            if p.pin_count == 0 && p.last_access < oldest {
                oldest = p.last_access;
                victim = Some(k);
            }
        }
        victim.and_then(|k| {
            guard.evictions += 1;
            guard.pages.remove(&k).map(|p| (k, p))
        })
    }

    fn evict_clean_if_full(guard: &mut PageCacheInner) {
        if guard.pages.len() < guard.capacity_pages {
            return;
        }
        let mut victim: Option<PageKey> = None;
        let mut oldest = u64::MAX;
        for (&k, p) in guard.pages.iter() {
            if p.pin_count == 0 && !p.dirty && p.last_access < oldest {
                oldest = p.last_access;
                victim = Some(k);
            }
        }
        if let Some(k) = victim {
            guard.evictions += 1;
            guard.pages.remove(&k);
        }
    }
}

/// Global VFS 4 KiB Page Cache instance (capacity: 256 pages = 1 MiB).
pub static GLOBAL_PAGE_CACHE: PageCache = PageCache::new_empty(256);
