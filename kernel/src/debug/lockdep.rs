//! # Kernel Deadlock Detection & Lock-Order Validator (Phase 18 - M40)
//!
//! ## Overview
//! Implements a runtime lock-dependency validator (`lockdep`-inspired) to detect
//! synchronization hazards deterministically before hard hangs occur:
//! 1. **Recursive Self-Deadlock Detection (`A -> A`)**: Identifies attempts to re-acquire
//!    a non-recursive lock class already held on the active CPU core.
//! 2. **Lock-Order Inversion & Transitive Cycle Detection (`A -> B` vs `B -> ... -> A`)**:
//!    Maintains a directed bitmask adjacency graph over registered lock classes and runs
//!    a fast DFS reachability probe whenever a new nesting edge `prev -> next` is observed.
//! 3. **Spin-Wait Watchdog Telemetry**: Tracks long lock-wait incidents and active per-CPU
//!    held-lock stacks for crash dumps and interactive `dbg locks` inspection.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::arch::x86_64::apic;
use crate::sync::Spinlock;

/// Maximum lock classes tracked in the directed dependency graph (fits cleanly in `u64` bitmask).
pub const MAX_LOCK_CLASSES: usize = 64;
/// Maximum nested locks held concurrently per CPU core.
pub const MAX_HELD_PER_CPU: usize = 16;
/// Maximum logical CPU slots tracked for held-lock stacks.
pub const MAX_LOCKDEP_CPUS: usize = 8;

/// Numeric handle referencing a registered [`LockGraph`] class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockClassId(pub usize);

/// Classified deadlock or lock-order anomaly detected by the validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlockKind {
    /// Same lock class acquired recursively while already held on this CPU.
    RecursiveSelfDeadlock,
    /// Acquiring `next` while holding `prev` closes a dependency cycle in the lock graph.
    OrderInversion,
    /// Spin-wait exceeded watchdog cycle budget (possible external stall or missing unlock).
    SpinTimeout,
}

/// Captured violation diagnostic record.
#[derive(Debug, Clone, Copy)]
pub struct DeadlockViolation {
    pub kind: DeadlockKind,
    pub cpu_id: u16,
    pub held_class: usize,
    pub target_class: usize,
    pub wait_cycles: u64,
}

#[derive(Clone, Copy)]
struct LockClassMeta {
    name: &'static str,
    acquires: u64,
    spin_timeouts: u64,
}

impl LockClassMeta {
    const fn empty() -> Self {
        Self {
            name: "",
            acquires: 0,
            spin_timeouts: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct HeldStack {
    len: usize,
    classes: [usize; MAX_HELD_PER_CPU],
}

impl HeldStack {
    const fn empty() -> Self {
        Self {
            len: 0,
            classes: [0; MAX_HELD_PER_CPU],
        }
    }
}

/// Reusable directed lock-dependency graph & per-CPU lock stack engine.
///
/// Both the global runtime tracker (`GLOBAL_LOCKDEP`) and M40 verification self-tests
/// share this identical validation state machine without duplicating graph logic.
pub struct LockGraph {
    num_classes: usize,
    classes: [LockClassMeta; MAX_LOCK_CLASSES],
    /// Directed adjacency bitmask: bit `j` set in `edges[i]` means `i` was held while acquiring `j`.
    edges: [u64; MAX_LOCK_CLASSES],
    held: [HeldStack; MAX_LOCKDEP_CPUS],
    violations_total: u64,
    last_violation: Option<DeadlockViolation>,
}

impl LockGraph {
    /// Creates an empty lock dependency graph.
    pub const fn new() -> Self {
        Self {
            num_classes: 0,
            classes: [LockClassMeta::empty(); MAX_LOCK_CLASSES],
            edges: [0; MAX_LOCK_CLASSES],
            held: [HeldStack::empty(); MAX_LOCKDEP_CPUS],
            violations_total: 0,
            last_violation: None,
        }
    }

    /// Registers (or retrieves existing) [`LockClassId`] for a named lock site.
    pub fn register_class(&mut self, name: &'static str) -> Option<LockClassId> {
        for i in 0..self.num_classes {
            if self.classes[i].name == name {
                return Some(LockClassId(i));
            }
        }
        if self.num_classes >= MAX_LOCK_CLASSES {
            return None;
        }
        let id = self.num_classes;
        self.classes[id].name = name;
        self.num_classes += 1;
        Some(LockClassId(id))
    }

    /// Returns `true` if a directed path exists from `src` to `dst` in the dependency graph.
    fn can_reach(&self, src: usize, dst: usize) -> bool {
        if src == dst {
            return true;
        }
        let mut visited = 0u64;
        let mut stack = [0usize; MAX_LOCK_CLASSES];
        let mut sp = 0usize;
        stack[sp] = src;
        sp += 1;
        visited |= 1u64 << src;

        while sp > 0 {
            sp -= 1;
            let cur = stack[sp];
            let adj = self.edges[cur];
            if (adj & (1u64 << dst)) != 0 {
                return true;
            }
            for nxt in 0..self.num_classes {
                if (adj & (1u64 << nxt)) != 0 && (visited & (1u64 << nxt)) == 0 {
                    visited |= 1u64 << nxt;
                    stack[sp] = nxt;
                    sp += 1;
                }
            }
        }
        false
    }

    /// Validates acquiring `class_id` on `cpu_id`, pushing it onto the held stack and recording
    /// nesting edges. Returns `Err(violation)` on AA self-deadlock or AB-BA / transitive cycle.
    pub fn try_acquire(&mut self, cpu_id: usize, class_id: LockClassId) -> Result<(), DeadlockViolation> {
        let c = class_id.0;
        let cpu = cpu_id % MAX_LOCKDEP_CPUS;
        if c >= self.num_classes {
            return Ok(());
        }

        let stack = self.held[cpu];
        // 1. Check recursive self-deadlock (AA) against all held locks on this CPU
        for i in 0..stack.len {
            if stack.classes[i] == c {
                let viol = DeadlockViolation {
                    kind: DeadlockKind::RecursiveSelfDeadlock,
                    cpu_id: cpu as u16,
                    held_class: c,
                    target_class: c,
                    wait_cycles: 0,
                };
                self.violations_total += 1;
                self.last_violation = Some(viol);
                return Err(viol);
            }
        }

        // 2. Validate against cycles and record edge from innermost held lock (`prev -> c`)
        for i in 0..stack.len {
            let prev = stack.classes[i];
            if (self.edges[prev] & (1u64 << c)) == 0 {
                if self.can_reach(c, prev) {
                    let viol = DeadlockViolation {
                        kind: DeadlockKind::OrderInversion,
                        cpu_id: cpu as u16,
                        held_class: prev,
                        target_class: c,
                        wait_cycles: 0,
                    };
                    self.violations_total += 1;
                    self.last_violation = Some(viol);
                    return Err(viol);
                }
                self.edges[prev] |= 1u64 << c;
            }
        }

        self.classes[c].acquires += 1;
        let slot = &mut self.held[cpu];
        if slot.len < MAX_HELD_PER_CPU {
            slot.classes[slot.len] = c;
            slot.len += 1;
        }
        Ok(())
    }

    /// Pops `class_id` from `cpu_id`'s active held-lock stack upon unlock.
    pub fn release(&mut self, cpu_id: usize, class_id: LockClassId) {
        let cpu = cpu_id % MAX_LOCKDEP_CPUS;
        let slot = &mut self.held[cpu];
        for i in (0..slot.len).rev() {
            if slot.classes[i] == class_id.0 {
                for j in i..slot.len.saturating_sub(1) {
                    slot.classes[j] = slot.classes[j + 1];
                }
                slot.len -= 1;
                break;
            }
        }
    }

    /// Records a spin-wait watchdog event when wait cycles exceed expected bounds.
    pub fn record_spin_timeout(&mut self, cpu_id: usize, class_id: LockClassId, wait_cycles: u64) {
        if class_id.0 < self.num_classes {
            self.classes[class_id.0].spin_timeouts += 1;
        }
        self.violations_total += 1;
        self.last_violation = Some(DeadlockViolation {
            kind: DeadlockKind::SpinTimeout,
            cpu_id: (cpu_id % MAX_LOCKDEP_CPUS) as u16,
            held_class: class_id.0,
            target_class: class_id.0,
            wait_cycles,
        });
    }

    /// Returns the name registered for `id`.
    pub fn class_name(&self, id: usize) -> &'static str {
        if id < self.num_classes {
            self.classes[id].name
        } else {
            "<unknown>"
        }
    }
}

static GLOBAL_LOCKDEP: Spinlock<LockGraph> = Spinlock::new(LockGraph::new());

/// Registers core subsystem lock classes for live runtime deadlock telemetry.
pub fn init() {
    let mut g = GLOBAL_LOCKDEP.lock();
    let _ = g.register_class("SCHEDULER");
    let _ = g.register_class("PROCESS_TABLE");
    let _ = g.register_class("FRAME_ALLOCATOR");
    let _ = g.register_class("HEAP_ALLOCATOR");
    let _ = g.register_class("VFS_MOUNT");
    let _ = g.register_class("KLOG_RING");
    let _ = g.register_class("CRASH_HISTORY");
}

/// Registers a named lock class in the global runtime dependency graph.
pub fn register_class(name: &'static str) -> Option<LockClassId> {
    GLOBAL_LOCKDEP.lock().register_class(name)
}

/// Validates and records acquiring `class_id` on the current CPU core.
///
/// Emits a structured `ERROR` log + trace marker when a deadlock hazard is observed.
pub fn lock_acquire(class_id: LockClassId) -> Result<(), DeadlockViolation> {
    let cpu = apic::current_apic_id() as usize;
    let res = GLOBAL_LOCKDEP.lock().try_acquire(cpu, class_id);
    if let Err(viol) = res {
        let (prev_name, tgt_name) = {
            let g = GLOBAL_LOCKDEP.lock();
            (g.class_name(viol.held_class), g.class_name(viol.target_class))
        };
        let label = match viol.kind {
            DeadlockKind::RecursiveSelfDeadlock => "Recursive AA deadlock detected",
            DeadlockKind::OrderInversion => "Lock-order AB-BA inversion detected",
            DeadlockKind::SpinTimeout => "Lock spin-wait timeout",
        };
        super::klog::log_structured(
            super::klog::LogLevel::Error,
            "LOCKDEP",
            label,
            &[
                super::klog::LogField::str("held", prev_name),
                super::klog::LogField::str("target", tgt_name),
                super::klog::LogField::int("cpu", viol.cpu_id as i64),
            ],
        );
    }
    res
}

/// Records releasing `class_id` on the current CPU core.
pub fn lock_release(class_id: LockClassId) {
    let cpu = apic::current_apic_id() as usize;
    GLOBAL_LOCKDEP.lock().release(cpu, class_id);
}

/// Returns names of locks currently held on `cpu_id` (used in crash dumps & `dbg locks`).
pub fn held_locks_snapshot(cpu_id: usize) -> Vec<&'static str> {
    let g = GLOBAL_LOCKDEP.lock();
    let cpu = cpu_id % MAX_LOCKDEP_CPUS;
    let stack = g.held[cpu];
    let mut names = Vec::with_capacity(stack.len);
    for i in 0..stack.len {
        names.push(g.class_name(stack.classes[i]));
    }
    names
}

/// Formats registered lock classes, observed dependency edges, and deadlock violation counts.
pub fn format_status() -> String {
    let g = GLOBAL_LOCKDEP.lock();
    let mut edge_count = 0usize;
    for i in 0..g.num_classes {
        edge_count += g.edges[i].count_ones() as usize;
    }

    let mut out = format!(
        "=== Kernel Lockdep & Deadlock Detector (M40) ===\n  Registered Classes : {}/{}\n  Dependency Edges   : {}\n  Violations Caught  : {}\n",
        g.num_classes, MAX_LOCK_CLASSES, edge_count, g.violations_total
    );

    for i in 0..g.num_classes {
        let deps = g.edges[i];
        let mut dep_str = String::new();
        for j in 0..g.num_classes {
            if (deps & (1u64 << j)) != 0 {
                if !dep_str.is_empty() {
                    dep_str.push_str(", ");
                }
                dep_str.push_str(g.classes[j].name);
            }
        }
        out.push_str(&format!(
            "  [{:02}] {:<18} acq={:<4} timeouts={:<2} deps=[{}]\n",
            i,
            g.classes[i].name,
            g.classes[i].acquires,
            g.classes[i].spin_timeouts,
            if dep_str.is_empty() { "-" } else { &dep_str }
        ));
    }

    if let Some(v) = g.last_violation {
        out.push_str(&format!(
            "  Last Violation     : {:?} (cpu={}, held={}, target={})\n",
            v.kind,
            v.cpu_id,
            g.class_name(v.held_class),
            g.class_name(v.target_class)
        ));
    }

    out
}

/// Runs isolated self-tests verifying `A -> A` self-deadlock, `A -> B -> A` inversion,
/// and 3-node transitive `A -> B -> C -> A` cycle detection without tainting runtime state.
pub fn run_self_tests() {
    let mut g = LockGraph::new();
    let a = g.register_class("LOCK_A").unwrap();
    let b = g.register_class("LOCK_B").unwrap();
    let c = g.register_class("LOCK_C").unwrap();

    // Valid nested order A -> B -> C
    assert!(g.try_acquire(0, a).is_ok());
    assert!(g.try_acquire(0, b).is_ok());
    assert!(g.try_acquire(0, c).is_ok());

    // 1. Recursive AA self-deadlock must be rejected
    let aa = g.try_acquire(0, b);
    assert!(
        matches!(aa, Err(v) if v.kind == DeadlockKind::RecursiveSelfDeadlock),
        "LockGraph must reject recursive AA self-deadlock"
    );

    g.release(0, c);
    g.release(0, b);
    g.release(0, a);

    // 2. Transitive cycle C -> A (violates A -> B -> C) must be rejected
    assert!(g.try_acquire(0, c).is_ok());
    let ca = g.try_acquire(0, a);
    assert!(
        matches!(ca, Err(v) if v.kind == DeadlockKind::OrderInversion),
        "LockGraph must reject transitive C->A inversion when A->B->C was observed"
    );
    g.release(0, c);

    // Record one legitimate monitored runtime acquire/release sequence
    if let Some(klog_id) = register_class("KLOG_RING") {
        let _ = lock_acquire(klog_id);
        lock_release(klog_id);
    }
}

