//! Strongly-typed physical and virtual memory addresses and frame abstractions.
#![allow(dead_code)]

pub const PAGE_SIZE_4K: u64 = 4096;

/// A 64-bit physical memory address.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    #[inline]
    pub const fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }

    #[inline]
    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }

    #[inline]
    pub const fn is_aligned(self, align: u64) -> bool {
        (self.0 & (align - 1)) == 0
    }

    #[inline]
    pub const fn align_down(self, align: u64) -> Self {
        Self(self.0 & !(align - 1))
    }

    #[inline]
    pub const fn align_up(self, align: u64) -> Self {
        Self((self.0 + align - 1) & !(align - 1))
    }
}

impl core::fmt::Debug for PhysAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PhysAddr({:#018x})", self.0)
    }
}

impl core::fmt::Display for PhysAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:#018x}", self.0)
    }
}

impl core::ops::Add<u64> for PhysAddr {
    type Output = Self;
    #[inline]
    fn add(self, rhs: u64) -> Self {
        Self(self.0 + rhs)
    }
}

impl core::ops::Sub<u64> for PhysAddr {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: u64) -> Self {
        Self(self.0 - rhs)
    }
}

/// A 64-bit virtual memory address.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    #[inline]
    pub const fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }

    #[inline]
    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }

    #[inline]
    pub const fn is_aligned(self, align: u64) -> bool {
        (self.0 & (align - 1)) == 0
    }

    #[inline]
    pub const fn align_down(self, align: u64) -> Self {
        Self(self.0 & !(align - 1))
    }

    #[inline]
    pub const fn align_up(self, align: u64) -> Self {
        Self((self.0 + align - 1) & !(align - 1))
    }
}

impl core::fmt::Debug for VirtAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "VirtAddr({:#018x})", self.0)
    }
}

impl core::fmt::Display for VirtAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:#018x}", self.0)
    }
}

/// A 4 KiB physical memory frame.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysFrame {
    index: u64,
}

impl PhysFrame {
    #[inline]
    pub const fn containing_address(address: PhysAddr) -> Self {
        Self {
            index: address.0 / PAGE_SIZE_4K,
        }
    }

    #[inline]
    pub const fn from_index(index: u64) -> Self {
        Self { index }
    }

    #[inline]
    pub const fn start_address(&self) -> PhysAddr {
        PhysAddr::new(self.index * PAGE_SIZE_4K)
    }

    #[inline]
    pub const fn index(&self) -> u64 {
        self.index
    }
}

impl core::fmt::Debug for PhysFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PhysFrame(#{} @ {:#x})", self.index, self.start_address().0)
    }
}

impl core::fmt::Display for PhysFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PhysFrame(#{} @ {:#x})", self.index, self.start_address().0)
    }
}

/// A 4 KiB virtual memory page.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Page {
    index: u64,
}

impl Page {
    #[inline]
    pub const fn containing_address(address: VirtAddr) -> Self {
        Self {
            index: address.0 / PAGE_SIZE_4K,
        }
    }

    #[inline]
    pub const fn from_index(index: u64) -> Self {
        Self { index }
    }

    #[inline]
    pub const fn start_address(&self) -> VirtAddr {
        VirtAddr::new(self.index * PAGE_SIZE_4K)
    }

    #[inline]
    pub const fn index(&self) -> u64 {
        self.index
    }
}

impl core::fmt::Debug for Page {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Page(#{} @ {:#x})", self.index, self.start_address().0)
    }
}
