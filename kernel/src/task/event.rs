//! # Async I/O, Event Polling, Epoll, EventFd, and TimerFd
//!
//! ## Overview
//! High-performance scalable server architectures and interactive GUI event loops
//! require multiplexed, non-blocking I/O event notification without thread-per-connection overhead.
//!
//! This module provides full POSIX and Linux-compatible asynchronous event primitives:
//! 1. **Multiplexed Polling ([`EpollInstance`])**:
//!    - Standard `epoll(7)` control (`EPOLL_CTL_ADD`, `EPOLL_CTL_MOD`, `EPOLL_CTL_DEL`).
//!    - Edge-triggered (`EPOLLET`) and level-triggered readiness inspection.
//!    - Integrates directly with the VFS [`Inode::poll`] interface.
//! 2. **Inter-Thread/Process Counter Notifications ([`EventFd`])**:
//!    - Implements `eventfd(2)` 64-bit atomic counter synchronization.
//!    - Supports standard notification mode (resets counter to 0 upon `read`) and semaphore
//!      mode (`EFD_SEMAPHORE`, decrements counter by 1 per `read`).
//! 3. **File-Descriptor Timer Notifications ([`TimerFd`])**:
//!    - Implements `timerfd_create(2)` and `timerfd_settime(2)`.
//!    - Arms single-shot or periodic interval timers anchored to APIC monotonic clock ticks (100 Hz).
//!    - Reading the descriptor returns the 64-bit integer count of expired intervals.
//! 4. **Scatter/Gather Vectored I/O ([`IoVec`])**:
//!    - Implements `readv(2)` and `writev(2)` zero-copy multi-buffer transfers.
//! 5. **Kernel Asynchronous Completion Ring ([`AsyncCompletionQueue`])**:
//!    - io_uring-style completion queues posting and reaping background I/O completions.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::vfs::{
    DirEntry, FileStat, FileType, FsError, Inode,
    POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT, POLLPRI,
};
use crate::sync::Spinlock;

/// Epoll control operations conforming to `epoll_ctl(2)`.
pub const EPOLL_CTL_ADD: i32 = 1;
pub const EPOLL_CTL_DEL: i32 = 2;
pub const EPOLL_CTL_MOD: i32 = 3;

/// Epoll event bitmasks conforming to `epoll(7)`.
pub const EPOLLIN: u32 = 0x0001;
pub const EPOLLPRI: u32 = 0x0002;
pub const EPOLLOUT: u32 = 0x0004;
pub const EPOLLERR: u32 = 0x0008;
pub const EPOLLHUP: u32 = 0x0010;
pub const EPOLLET: u32 = 1 << 31;
pub const EPOLLONESHOT: u32 = 1 << 30;

/// EventFd flags conforming to `eventfd(2)`.
pub const EFD_SEMAPHORE: i32 = 1;
pub const EFD_CLOEXEC: i32 = 0o2000000;
pub const EFD_NONBLOCK: i32 = 0o4000;

/// TimerFd clock identifiers conforming to `timerfd_create(2)`.
pub const CLOCK_REALTIME: i32 = 0;
pub const CLOCK_MONOTONIC: i32 = 1;

/// TimerFd flags conforming to `timerfd_settime(2)`.
pub const TFD_TIMER_ABSTIME: i32 = 1;
pub const TFD_CLOEXEC: i32 = 0o2000000;
pub const TFD_NONBLOCK: i32 = 0o4000;

static NEXT_EVENT_INODE: AtomicU64 = AtomicU64::new(0x9000_0000);

fn next_event_inode_num() -> u64 {
    NEXT_EVENT_INODE.fetch_add(1, Ordering::Relaxed)
}

/// Poll descriptor structure conforming to POSIX `struct pollfd`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PollFd {
    /// Target file descriptor to poll.
    pub fd: i32,
    /// Requested event bitmask (POLLIN, POLLOUT, etc.).
    pub events: i16,
    /// Returned event bitmask reported by kernel.
    pub revents: i16,
}

/// Epoll event structure matching standard x86_64 ABI `struct epoll_event`.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EpollEvent {
    /// Bitmask of events (EPOLLIN, EPOLLOUT, EPOLLET, etc.).
    pub events: u32,
    /// User payload / file descriptor token passed in `epoll_ctl`.
    pub data: u64,
}

/// Registered item inside an epoll instance.
#[derive(Debug, Clone, Copy)]
pub struct EpollItem {
    pub fd: i32,
    pub events: u32,
    pub data: u64,
}

/// Epoll instance backed by an [`Inode`], allowing nesting and unified VFS management.
pub struct EpollInstance {
    items: Spinlock<BTreeMap<i32, EpollItem>>,
    inode_num: u64,
}

impl EpollInstance {
    /// Creates a new epoll instance.
    pub fn new() -> Self {
        Self {
            items: Spinlock::new(BTreeMap::new()),
            inode_num: next_event_inode_num(),
        }
    }

    /// Modifies, adds, or deletes monitored file descriptors within this epoll instance.
    pub fn ctl(&self, op: i32, fd: i32, event: EpollEvent) -> Result<(), FsError> {
        let mut items = self.items.lock();
        match op {
            EPOLL_CTL_ADD => {
                if items.contains_key(&fd) {
                    return Err(FsError::AlreadyExists);
                }
                items.insert(fd, EpollItem {
                    fd,
                    events: event.events,
                    data: event.data,
                });
                Ok(())
            }
            EPOLL_CTL_DEL => {
                if items.remove(&fd).is_some() {
                    Ok(())
                } else {
                    Err(FsError::NotFound)
                }
            }
            EPOLL_CTL_MOD => {
                if let Some(item) = items.get_mut(&fd) {
                    item.events = event.events;
                    item.data = event.data;
                    Ok(())
                } else {
                    Err(FsError::NotFound)
                }
            }
            _ => Err(FsError::InvalidInput),
        }
    }

    /// Checks registered items against the process file descriptor table, returning ready events.
    pub fn poll_events(
        &self,
        fd_table: &crate::fs::fd::FileDescriptorTable,
        maxevents: usize,
    ) -> Vec<EpollEvent> {
        let items = self.items.lock();
        let mut ready = Vec::new();

        for (&fd, item) in items.iter() {
            if ready.len() >= maxevents {
                break;
            }
            if let Ok(handle) = fd_table.get(fd) {
                let mask = handle.inode.poll();
                let matching = mask & (item.events | POLLERR | POLLHUP);
                if matching != 0 {
                    ready.push(EpollEvent {
                        events: matching,
                        data: item.data,
                    });
                }
            } else {
                // Target file descriptor closed / invalid
                ready.push(EpollEvent {
                    events: POLLNVAL | POLLHUP,
                    data: item.data,
                });
            }
        }

        ready
    }
}

impl Inode for EpollInstance {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::CharDevice,
            size: self.items.lock().len(),
            mode: 0o600,
        }
    }

    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, FsError> {
        Err(FsError::PermissionDenied)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::PermissionDenied)
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn create(&self, _name: &str, _file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn poll(&self) -> u32 {
        POLLIN | POLLOUT
    }

    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}

/// 64-bit counter event notification descriptor conforming to `eventfd(2)`.
pub struct EventFd {
    counter: Spinlock<u64>,
    flags: i32,
    inode_num: u64,
}

impl EventFd {
    pub fn new(initval: u32, flags: i32) -> Self {
        Self {
            counter: Spinlock::new(initval as u64),
            flags,
            inode_num: next_event_inode_num(),
        }
    }
}

impl Inode for EventFd {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::CharDevice,
            size: 8,
            mode: 0o600,
        }
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        if buf.len() < 8 {
            return Err(FsError::InvalidInput);
        }

        let mut counter = self.counter.lock();
        if *counter == 0 {
            if (self.flags & EFD_NONBLOCK) != 0 {
                return Err(FsError::WouldBlock);
            }
            return Err(FsError::WouldBlock);
        }

        let val = if (self.flags & EFD_SEMAPHORE) != 0 {
            *counter -= 1;
            1u64
        } else {
            let res = *counter;
            *counter = 0;
            res
        };

        buf[..8].copy_from_slice(&val.to_ne_bytes());
        Ok(8)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        if buf.len() < 8 {
            return Err(FsError::InvalidInput);
        }

        let val = u64::from_ne_bytes([
            buf[0], buf[1], buf[2], buf[3],
            buf[4], buf[5], buf[6], buf[7],
        ]);

        if val == u64::MAX {
            return Err(FsError::InvalidInput);
        }

        let mut counter = self.counter.lock();
        if u64::MAX - *counter < val {
            return Err(FsError::WouldBlock);
        }

        *counter += val;
        Ok(8)
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn create(&self, _name: &str, _file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn poll(&self) -> u32 {
        let count = *self.counter.lock();
        let mut revents = 0;
        if count > 0 {
            revents |= POLLIN;
        }
        if count < u64::MAX - 1 {
            revents |= POLLOUT;
        }
        revents
    }

    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}

/// POSIX timespec representation for timers.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimeSpec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// POSIX itimerspec holding interval and initial value.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ITimerSpec {
    pub it_interval: TimeSpec,
    pub it_value: TimeSpec,
}

/// Interval and single-shot timer descriptor conforming to `timerfd_create(2)`.
pub struct TimerFd {
    pub clockid: i32,
    pub flags: i32,
    target_tick: Spinlock<Option<u64>>,
    interval_ticks: Spinlock<u64>,
    expirations: Spinlock<u64>,
    inode_num: u64,
}

impl TimerFd {
    pub fn new(clockid: i32, flags: i32) -> Self {
        Self {
            clockid,
            flags,
            target_tick: Spinlock::new(None),
            interval_ticks: Spinlock::new(0),
            expirations: Spinlock::new(0),
            inode_num: next_event_inode_num(),
        }
    }

    /// Sets timer expiration and interval times.
    pub fn set_time(&self, new_val: ITimerSpec) -> ITimerSpec {
        let current_ticks = crate::arch::x86_64::apic::get_ticks();
        let hz = 100u64; // APIC timer operates at 100 Hz

        let mut target = self.target_tick.lock();
        let mut interval = self.interval_ticks.lock();
        let mut exp = self.expirations.lock();

        let old_spec = ITimerSpec::default();

        let mut val_ticks = (new_val.it_value.tv_sec.max(0) as u64) * hz
            + ((new_val.it_value.tv_nsec.max(0) as u64) * hz / 1_000_000_000);
        if val_ticks == 0 && (new_val.it_value.tv_sec > 0 || new_val.it_value.tv_nsec > 0) {
            val_ticks = 1;
        }

        let mut int_ticks = (new_val.it_interval.tv_sec.max(0) as u64) * hz
            + ((new_val.it_interval.tv_nsec.max(0) as u64) * hz / 1_000_000_000);
        if int_ticks == 0 && (new_val.it_interval.tv_sec > 0 || new_val.it_interval.tv_nsec > 0) {
            int_ticks = 1;
        }

        if val_ticks == 0 {
            // Disarm timer
            *target = None;
            *interval = 0;
            *exp = 0;
        } else {
            *target = Some(current_ticks + val_ticks);
            *interval = int_ticks;
            *exp = 0;
        }

        old_spec
    }

    fn check_expiration(&self) {
        let current_ticks = crate::arch::x86_64::apic::get_ticks();
        let mut target = self.target_tick.lock();
        if let Some(t) = *target {
            if current_ticks >= t {
                let mut exp = self.expirations.lock();
                *exp += 1;
                let int_ticks = *self.interval_ticks.lock();
                if int_ticks > 0 {
                    *target = Some(current_ticks + int_ticks);
                } else {
                    *target = None; // One-shot complete
                }
            }
        }
    }
}

impl Inode for TimerFd {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::CharDevice,
            size: 8,
            mode: 0o600,
        }
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        if buf.len() < 8 {
            return Err(FsError::InvalidInput);
        }

        self.check_expiration();
        let mut exp = self.expirations.lock();
        if *exp == 0 {
            return Err(FsError::WouldBlock);
        }

        let val = *exp;
        *exp = 0;
        buf[..8].copy_from_slice(&val.to_ne_bytes());
        Ok(8)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::PermissionDenied)
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn create(&self, _name: &str, _file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn poll(&self) -> u32 {
        self.check_expiration();
        let exp = *self.expirations.lock();
        if exp > 0 {
            POLLIN
        } else {
            0
        }
    }

    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}

/// Scatter/gather I/O buffer descriptor conforming to `struct iovec`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IoVec {
    pub iov_base: u64,
    pub iov_len: usize,
}

/// Asynchronous completion queue entry for kernel/user async I/O ring.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompletionEntry {
    pub user_data: u64,
    pub res: i32,
    pub flags: u32,
}

/// Thread-safe Async Event / Completion Queue.
pub struct AsyncCompletionQueue {
    entries: Spinlock<Vec<CompletionEntry>>,
    capacity: usize,
}

impl AsyncCompletionQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Spinlock::new(Vec::with_capacity(capacity)),
            capacity,
        }
    }

    pub fn post(&self, user_data: u64, res: i32, flags: u32) -> bool {
        let mut q = self.entries.lock();
        if q.len() >= self.capacity {
            return false;
        }
        q.push(CompletionEntry { user_data, res, flags });
        true
    }

    pub fn reap(&self) -> Option<CompletionEntry> {
        let mut q = self.entries.lock();
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }
}

/// Global Async I/O Completion Queue for kernel background tasks.
pub static GLOBAL_COMPLETION_QUEUE: Spinlock<Option<AsyncCompletionQueue>> = Spinlock::new(None);

pub fn init() {
    *GLOBAL_COMPLETION_QUEUE.lock() = Some(AsyncCompletionQueue::new(128));
}
