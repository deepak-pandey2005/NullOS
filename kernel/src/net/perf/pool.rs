//! # Network Buffer Pools & Cache-Aligned Packet Slabs (Phase 16 - M36)
//!
//! ## Overview
//! Eliminates dynamic heap allocations (`malloc` / `Vec::new()`) on the packet
//! hot path by pre-allocating a contiguous slab of fixed-size, cache-line-aligned
//! (`64`-byte aligned) 2048-byte network buffers (`NetBuf`).
//!
//! ## Zero-Copy Headroom Architecture
//! Every [`NetBuf`] reserves `HEADROOM_BYTES` (64 bytes) at the front of its data array:
//! ```text
//! [0 .. headroom (64B)] [data_start .. data_end (payload)] [tailroom .. 2048B]
//!   ^ Ethernet (14B) + IPv4 (20B) + TCP (20B) prepended in-place without memcpy!
//! ```
//! Outgoing protocols simply decrement `data_start` (`push_header`) to prepend L4/L3/L2
//! headers directly before the payload, while incoming handlers increment `data_start`
//! (`pull_header`) to strip headers in $O(1)$ time with zero memory copies.

use alloc::vec::Vec;
use crate::sync::Spinlock;

/// Total byte capacity of each pre-allocated packet buffer (covers standard 1514B MTU + headroom).
pub const NET_BUF_SIZE: usize = 2048;

/// Default front headroom reserved for in-place protocol header prepending (64 bytes).
pub const NET_BUF_HEADROOM: usize = 64;

/// Default number of packet buffers pre-allocated in the global pool (128 x 2 KiB = 256 KiB).
pub const DEFAULT_POOL_CAPACITY: usize = 128;

/// Cache-line-aligned 2048-byte network packet buffer descriptor.
#[repr(C, align(64))]
#[derive(Clone)]
pub struct NetBuf {
    /// Raw backing storage for headers and packet payload.
    pub storage: [u8; NET_BUF_SIZE],
    /// Current starting byte offset of valid packet data inside `storage`.
    pub data_start: usize,
    /// Current length of valid packet data in bytes.
    pub data_len: usize,
    /// Computed RSS Toeplitz flow hash for multi-queue affinity.
    pub rss_hash: u32,
    /// Assigned NIC/software queue index.
    pub queue_id: u16,
    /// Pool slot index used to return buffer in O(1).
    pub slot_idx: usize,
}

impl NetBuf {
    /// Creates an empty packet buffer initialized with standard headroom.
    pub const fn new_zeroed(slot_idx: usize) -> Self {
        Self {
            storage: [0u8; NET_BUF_SIZE],
            data_start: NET_BUF_HEADROOM,
            data_len: 0,
            rss_hash: 0,
            queue_id: 0,
            slot_idx,
        }
    }

    /// Resets offsets to reuse this buffer cleanly without zeroing all 2 KiB.
    #[inline]
    pub fn reset(&mut self) {
        self.data_start = NET_BUF_HEADROOM;
        self.data_len = 0;
        self.rss_hash = 0;
        self.queue_id = 0;
    }

    /// Returns an immutable slice view over the active packet bytes.
    #[inline]
    pub fn data(&self) -> &[u8] {
        &self.storage[self.data_start..self.data_start + self.data_len]
    }

    /// Returns a mutable slice view over the active packet bytes.
    #[inline]
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.storage[self.data_start..self.data_start + self.data_len]
    }

    /// Writes payload bytes directly into the buffer starting at the current headroom cursor.
    pub fn put_payload(&mut self, payload: &[u8]) -> Result<(), &'static str> {
        if self.data_start + self.data_len + payload.len() > NET_BUF_SIZE {
            return Err("NetBuf tailroom exhausted");
        }
        let dst_off = self.data_start + self.data_len;
        self.storage[dst_off..dst_off + payload.len()].copy_from_slice(payload);
        self.data_len += payload.len();
        Ok(())
    }

    /// Prepends `header_len` bytes in front of existing data inside the headroom (Zero-Copy TX framing).
    ///
    /// Returns a mutable slice pointing to the newly exposed header space so L4/L3/L2 headers
    /// can be written directly in-place.
    #[inline]
    pub fn push_header(&mut self, header_len: usize) -> Result<&mut [u8], &'static str> {
        if header_len > self.data_start {
            return Err("NetBuf headroom exhausted");
        }
        self.data_start -= header_len;
        self.data_len += header_len;
        Ok(&mut self.storage[self.data_start..self.data_start + header_len])
    }

    /// Strips `header_len` bytes from the front of the packet without moving payload bytes (Zero-Copy RX parsing).
    #[inline]
    pub fn pull_header(&mut self, header_len: usize) -> Result<&[u8], &'static str> {
        if header_len > self.data_len {
            return Err("NetBuf underflow pulling header");
        }
        let start = self.data_start;
        self.data_start += header_len;
        self.data_len -= header_len;
        Ok(&self.storage[start..start + header_len])
    }

    /// Returns remaining bytes of available headroom before `data_start`.
    #[inline]
    pub fn headroom(&self) -> usize {
        self.data_start
    }

    /// Returns remaining bytes of available tailroom after `data_start + data_len`.
    #[inline]
    pub fn tailroom(&self) -> usize {
        NET_BUF_SIZE.saturating_sub(self.data_start + self.data_len)
    }
}

/// Telemetry counters for a [`NetBufPool`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PoolStats {
    pub capacity: usize,
    pub free_count: usize,
    pub in_use: usize,
    pub peak_in_use: usize,
    pub alloc_hits: u64,
    pub alloc_misses: u64,
    pub frees: u64,
}

struct PoolInner {
    initialized: bool,
    slots: Vec<Option<NetBuf>>,
    free_stack: Vec<usize>,
    capacity: usize,
    peak_in_use: usize,
    alloc_hits: u64,
    alloc_misses: u64,
    frees: u64,
}

/// Thread-safe O(1) stack-managed Network Buffer Pool.
pub struct NetBufPool {
    inner: Spinlock<PoolInner>,
}

impl NetBufPool {
    /// Constructs an uninitialized static pool ready for `init(capacity)`.
    pub const fn new_uninit() -> Self {
        Self {
            inner: Spinlock::new(PoolInner {
                initialized: false,
                slots: Vec::new(),
                free_stack: Vec::new(),
                capacity: 0,
                peak_in_use: 0,
                alloc_hits: 0,
                alloc_misses: 0,
                frees: 0,
            }),
        }
    }

    /// Pre-allocates `capacity` cache-aligned packet buffers into the pool.
    pub fn init(&self, capacity: usize) {
        let mut guard = self.inner.lock();
        if guard.initialized {
            return;
        }
        guard.slots.reserve(capacity);
        guard.free_stack.reserve(capacity);
        for i in 0..capacity {
            guard.slots.push(Some(NetBuf::new_zeroed(i)));
            guard.free_stack.push(i);
        }
        guard.capacity = capacity;
        guard.initialized = true;
    }

    /// Allocates a clean, reset [`NetBuf`] from the pool in O(1) time.
    pub fn alloc(&self) -> Option<NetBuf> {
        let mut guard = self.inner.lock();
        if !guard.initialized {
            drop(guard);
            self.init(DEFAULT_POOL_CAPACITY);
            guard = self.inner.lock();
        }
        if let Some(idx) = guard.free_stack.pop() {
            if let Some(mut buf) = guard.slots[idx].take() {
                buf.reset();
                guard.alloc_hits += 1;
                let in_use = guard.capacity.saturating_sub(guard.free_stack.len());
                if in_use > guard.peak_in_use {
                    guard.peak_in_use = in_use;
                }
                return Some(buf);
            }
        }
        guard.alloc_misses += 1;
        None
    }

    /// Returns a [`NetBuf`] back to the pool free-stack in O(1) time.
    pub fn free(&self, mut buf: NetBuf) {
        let mut guard = self.inner.lock();
        let idx = buf.slot_idx;
        if idx < guard.capacity && guard.slots[idx].is_none() {
            buf.reset();
            guard.slots[idx] = Some(buf);
            guard.free_stack.push(idx);
            guard.frees += 1;
        }
    }

    /// Returns a snapshot of pool utilization and allocation counters.
    pub fn stats(&self) -> PoolStats {
        let guard = self.inner.lock();
        let free_count = guard.free_stack.len();
        let in_use = guard.capacity.saturating_sub(free_count);
        PoolStats {
            capacity: guard.capacity,
            free_count,
            in_use,
            peak_in_use: guard.peak_in_use,
            alloc_hits: guard.alloc_hits,
            alloc_misses: guard.alloc_misses,
            frees: guard.frees,
        }
    }
}

/// Global Network Buffer Pool shared by zero-copy RX/TX queues.
pub static GLOBAL_NET_POOL: NetBufPool = NetBufPool::new_uninit();

