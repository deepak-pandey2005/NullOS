//! # Strongly-Typed Memory Addresses and Page/Frame Abstractions
//!
//! ## Overview
//! In modern 64-bit systems, raw pointer arithmetic without semantic distinction
//! between physical hardware addresses and virtual addresses easily introduces severe
//! security vulnerabilities and kernel memory corruption.
//!
//! This module provides distinct, strongly-typed zero-cost wrappers:
//! - [`PhysAddr`]: A 64-bit physical address referencing physical RAM or memory-mapped I/O (MMIO).
//! - [`VirtAddr`]: A 64-bit virtual address belonging to the CPU's canonical virtual address space.
//! - [`PhysFrame`]: A discrete 4096-byte (4 KiB) aligned physical frame of RAM.
//! - [`Page`]: A discrete 4096-byte (4 KiB) aligned virtual page of memory.
//!
//! ## Safety & Canonical Address Rules
//! - On x86_64, virtual addresses must be in **canonical form**: bits 47 through 63 must match
//!   bit 47 (sign-extension). Non-canonical addresses trigger General Protection Faults (#GP).
//! - All frames and pages are aligned to [`PAGE_SIZE_4K`] boundaries.

#![allow(dead_code)]

/// Standard base page size on x86_64 architectures (4096 bytes / 4 KiB).
pub const PAGE_SIZE_4K: u64 = 4096;

/// A 64-bit physical memory address.
///
/// Wraps a raw physical address pointing to physical RAM or hardware MMIO space.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    /// Creates a new physical address from raw integer `addr`.
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    /// Returns the underlying raw physical address as `u64`.
    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Converts the physical address to an immutable raw pointer of type `T`.
    ///
    /// # Safety
    /// Physical addresses cannot be directly dereferenced on x86_64 unless mapped into
    /// the virtual address space (e.g. via identity mapping or direct physical map).
    #[inline]
    pub const fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }

    /// Converts the physical address to a mutable raw pointer of type `T`.
    ///
    /// # Safety
    /// Same safety constraints as [`as_ptr`]. Dereferencing must obey Rust's aliasing rules.
    #[inline]
    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }

    /// Checks whether the physical address is aligned to an integer `align` boundary.
    ///
    /// `align` must be a power of two.
    #[inline]
    pub const fn is_aligned(self, align: u64) -> bool {
        (self.0 & (align - 1)) == 0
    }

    /// Rounds down the address to the nearest multiple of `align`.
    ///
    /// `align` must be a power of two.
    #[inline]
    pub const fn align_down(self, align: u64) -> Self {
        Self(self.0 & !(align - 1))
    }

    /// Rounds up the address to the nearest multiple of `align`.
    ///
    /// `align` must be a power of two.
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
///
/// Wraps a virtual address used by the CPU core when paging is enabled.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    /// Creates a new virtual address from integer `addr`.
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    /// Returns the underlying raw virtual address as `u64`.
    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Converts the virtual address to an immutable raw pointer of type `T`.
    #[inline]
    pub const fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }

    /// Converts the virtual address to a mutable raw pointer of type `T`.
    #[inline]
    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }

    /// Checks whether the virtual address is aligned to an integer `align` boundary.
    ///
    /// `align` must be a power of two.
    #[inline]
    pub const fn is_aligned(self, align: u64) -> bool {
        (self.0 & (align - 1)) == 0
    }

    /// Rounds down the address to the nearest multiple of `align`.
    ///
    /// `align` must be a power of two.
    #[inline]
    pub const fn align_down(self, align: u64) -> Self {
        Self(self.0 & !(align - 1))
    }

    /// Rounds up the address to the nearest multiple of `align`.
    ///
    /// `align` must be a power of two.
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
///
/// Represents an aligned, 4096-byte chunk of physical RAM tracked by frame index.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysFrame {
    index: u64,
}

impl PhysFrame {
    /// Returns the physical frame enclosing the given physical address.
    #[inline]
    pub const fn containing_address(address: PhysAddr) -> Self {
        Self {
            index: address.0 / PAGE_SIZE_4K,
        }
    }

    /// Returns a physical frame corresponding to the specified frame index.
    #[inline]
    pub const fn from_index(index: u64) -> Self {
        Self { index }
    }

    /// Returns the base physical start address of this frame.
    #[inline]
    pub const fn start_address(&self) -> PhysAddr {
        PhysAddr::new(self.index * PAGE_SIZE_4K)
    }

    /// Returns the sequential zero-based index of this frame in physical memory.
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
///
/// Represents an aligned, 4096-byte chunk of virtual address space.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Page {
    index: u64,
}

impl Page {
    /// Returns the virtual page enclosing the given virtual address.
    #[inline]
    pub const fn containing_address(address: VirtAddr) -> Self {
        Self {
            index: address.0 / PAGE_SIZE_4K,
        }
    }

    /// Returns a virtual page from a zero-based page index.
    #[inline]
    pub const fn from_index(index: u64) -> Self {
        Self { index }
    }

    /// Returns the base virtual start address of this page.
    #[inline]
    pub const fn start_address(&self) -> VirtAddr {
        VirtAddr::new(self.index * PAGE_SIZE_4K)
    }

    /// Returns the sequential zero-based index of this page.
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
