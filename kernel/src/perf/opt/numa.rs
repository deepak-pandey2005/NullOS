//! # NUMA Awareness: ACPI SRAT/SLIT Discovery & Locality Policies (Phase 17 - M38)
//!
//! ## Overview
//! Discovers Non-Uniform Memory Access (NUMA) proximity domains from ACPI firmware tables:
//! - **SRAT (`b"SRAT"`) — System Resource Affinity Table**: Maps Local APIC IDs (CPU cores)
//!   and physical RAM ranges to integer Proximity Domains (NUMA Nodes `0..MAX_NUMA_NODES`).
//! - **SLIT (`b"SLIT"`) — System Locality Information Table**: Encodes normalized pairwise
//!   memory access cost (`10` = local access, `>= 20` = remote inter-socket hop).
//! - **UMA Fallback**: On single-node hardware or QEMU without explicit `-numa` flags, falls
//!   back cleanly to **Node 0 UMA** covering all online SMP cores and detected RAM.

use crate::arch::x86_64::{acpi, apic, smp};
use crate::sync::Spinlock;

/// Maximum NUMA proximity domains tracked by NullOS.
pub const MAX_NUMA_NODES: usize = 8;

/// Maximum logical CPUs mapped to NUMA domains.
pub const MAX_NUMA_CPUS: usize = 16;

/// Descriptor for a single NUMA Proximity Domain.
#[derive(Debug, Clone, Copy)]
pub struct NumaNode {
    pub node_id: u8,
    pub active: bool,
    pub cpu_mask: u64,
    pub mem_base: u64,
    pub mem_bytes: u64,
}

impl NumaNode {
    pub const fn empty(node_id: u8) -> Self {
        Self {
            node_id,
            active: false,
            cpu_mask: 0,
            mem_base: 0,
            mem_bytes: 0,
        }
    }
}

/// System-wide NUMA domain topology and inter-node distance matrix.
#[derive(Debug, Clone, Copy)]
pub struct NumaTopology {
    pub srat_found: bool,
    pub slit_found: bool,
    pub node_count: usize,
    pub nodes: [NumaNode; MAX_NUMA_NODES],
    /// Indexed by APIC/CPU ID -> NUMA Node ID (`0..node_count`).
    pub cpu_to_node: [u8; MAX_NUMA_CPUS],
    /// Distance matrix (`distance[src][dst]`, `10` = local, `20+` = remote).
    pub distances: [[u8; MAX_NUMA_NODES]; MAX_NUMA_NODES],
}

impl NumaTopology {
    pub const fn new_uma_default() -> Self {
        let nodes = [
            NumaNode::empty(0),
            NumaNode::empty(1),
            NumaNode::empty(2),
            NumaNode::empty(3),
            NumaNode::empty(4),
            NumaNode::empty(5),
            NumaNode::empty(6),
            NumaNode::empty(7),
        ];
        let mut distances = [[20u8; MAX_NUMA_NODES]; MAX_NUMA_NODES];
        let mut i = 0;
        while i < MAX_NUMA_NODES {
            distances[i][i] = 10;
            i += 1;
        }
        Self {
            srat_found: false,
            slit_found: false,
            node_count: 1,
            nodes,
            cpu_to_node: [0u8; MAX_NUMA_CPUS],
            distances,
        }
    }

    /// Returns the NUMA Proximity Domain ID owning `apic_id`.
    #[inline]
    pub fn node_for_cpu(&self, apic_id: u8) -> u8 {
        let idx = apic_id as usize;
        if idx < MAX_NUMA_CPUS {
            self.cpu_to_node[idx]
        } else {
            0
        }
    }

    /// Returns normalized ACPI SLIT latency cost (`10` = local, `>10` = remote) between nodes.
    #[inline]
    pub fn distance(&self, src_node: u8, dst_node: u8) -> u8 {
        let s = (src_node as usize).min(MAX_NUMA_NODES - 1);
        let d = (dst_node as usize).min(MAX_NUMA_NODES - 1);
        self.distances[s][d]
    }
}

pub static NUMA_TOPOLOGY: Spinlock<NumaTopology> = Spinlock::new(NumaTopology::new_uma_default());

/// Probes ACPI `SRAT` and `SLIT` physical tables to populate [`NUMA_TOPOLOGY`].
pub fn init() {
    let mut topo = NumaTopology::new_uma_default();

    // Parse ACPI SRAT (Static Resource Affinity Table, Header = 36B SDT + 12B reserved = 48B)
    if let Some(srat_addr) = acpi::find_table(b"SRAT") {
        let length = unsafe { core::ptr::read_unaligned((srat_addr + 4) as *const u32) } as usize;
        if length >= 48 {
            topo.srat_found = true;
            let mut offset = 48usize;
            let mut max_node_seen = 0usize;
            while offset + 2 <= length {
                let entry_type = unsafe { *((srat_addr + offset as u64) as *const u8) };
                let entry_len = unsafe { *((srat_addr + offset as u64 + 1) as *const u8) } as usize;
                if entry_len < 2 || offset + entry_len > length {
                    break;
                }
                match entry_type {
                    // Type 0: Processor Local APIC/SAPIC Affinity Structure (>= 16 bytes)
                    0 if entry_len >= 16 => {
                        let lo_dom = unsafe { *((srat_addr + offset as u64 + 2) as *const u8) };
                        let apic_id = unsafe { *((srat_addr + offset as u64 + 3) as *const u8) };
                        let flags = unsafe {
                            core::ptr::read_unaligned((srat_addr + offset as u64 + 4) as *const u32)
                        };
                        if (flags & 1) != 0 {
                            let dom = (lo_dom as usize).min(MAX_NUMA_NODES - 1);
                            topo.nodes[dom].active = true;
                            max_node_seen = core::cmp::max(max_node_seen, dom);
                            if (apic_id as usize) < MAX_NUMA_CPUS {
                                topo.cpu_to_node[apic_id as usize] = dom as u8;
                                topo.nodes[dom].cpu_mask |= 1u64 << (apic_id & 63);
                            }
                        }
                    }
                    // Type 1: Memory Affinity Structure (>= 40 bytes)
                    1 if entry_len >= 40 => {
                        let dom = unsafe {
                            core::ptr::read_unaligned((srat_addr + offset as u64 + 2) as *const u32)
                        } as usize;
                        let base_lo = unsafe {
                            core::ptr::read_unaligned((srat_addr + offset as u64 + 8) as *const u64)
                        };
                        let len_lo = unsafe {
                            core::ptr::read_unaligned((srat_addr + offset as u64 + 16) as *const u64)
                        };
                        let flags = unsafe {
                            core::ptr::read_unaligned((srat_addr + offset as u64 + 28) as *const u32)
                        };
                        if (flags & 1) != 0 && dom < MAX_NUMA_NODES {
                            topo.nodes[dom].active = true;
                            max_node_seen = core::cmp::max(max_node_seen, dom);
                            if topo.nodes[dom].mem_bytes == 0 {
                                topo.nodes[dom].mem_base = base_lo;
                            }
                            topo.nodes[dom].mem_bytes = topo.nodes[dom].mem_bytes.saturating_add(len_lo);
                        }
                    }
                    _ => {}
                }
                offset += entry_len;
            }
            topo.node_count = max_node_seen + 1;
        }
    }

    // Parse ACPI SLIT (System Locality Information Table: 36B SDT + 8B Number of Localities)
    if let Some(slit_addr) = acpi::find_table(b"SLIT") {
        let length = unsafe { core::ptr::read_unaligned((slit_addr + 4) as *const u32) } as usize;
        if length >= 44 {
            topo.slit_found = true;
            let locs = unsafe { core::ptr::read_unaligned((slit_addr + 36) as *const u64) } as usize;
            let n = core::cmp::min(locs, MAX_NUMA_NODES);
            for i in 0..n {
                for j in 0..n {
                    let off = 44 + i * locs + j;
                    if off < length {
                        topo.distances[i][j] = unsafe { *((slit_addr + off as u64) as *const u8) };
                    }
                }
            }
        }
    }

    // If no SRAT table exists (e.g., standard single-socket UMA hardware / default QEMU),
    // populate Node 0 cleanly with all online CPUs and system memory size.
    if !topo.srat_found {
        topo.node_count = 1;
        topo.nodes[0].active = true;
        let online = core::cmp::min(smp::online_cpu_count(), 64);
        topo.nodes[0].cpu_mask = if online == 64 { u64::MAX } else { (1u64 << online) - 1 };
        if let Some(mst) = crate::memory::stats() {
            topo.nodes[0].mem_bytes = (mst.total_frames as u64) * 4096;
        }
    }

    *NUMA_TOPOLOGY.lock() = topo;
}

/// Returns the NUMA Proximity Domain of the currently executing CPU core.
#[inline]
pub fn current_node() -> u8 {
    let apic_id = apic::current_apic_id();
    NUMA_TOPOLOGY.lock().node_for_cpu(apic_id)
}

/// Returns a thread-safe snapshot of discovered NUMA topology.
pub fn snapshot() -> NumaTopology {
    *NUMA_TOPOLOGY.lock()
}

