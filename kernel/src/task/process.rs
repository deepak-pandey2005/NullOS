//! Process Management (Phase 5 - M14).
//!
//! Subsystem features:
//! - Thread-safe PID allocator (ProcessId starting from 1).
//! - Process Control Block (PCB): Process structure with PID, state, parent/child relationships,
//!   dedicated AddressSpace (PML4), thread collection, and exit codes.
//! - Global thread-safe ProcessTable.
//! - Process creation (spawn_process / create_process) loading ELF executables.
//! - Process lifecycle management (Created, Ready, Running, Waiting, Zombie, Terminated).
//! - Parent/child waiting primitives (waitpid) and clean zombie harvesting.
//! - First user process ("init" / PID 1) launch.

use crate::memory::AddressSpace;
use crate::println;
use crate::sync::Spinlock;
use crate::task::thread::ThreadId;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// A unique 64-bit Process Identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId(pub u64);

impl ProcessId {
    pub const INIT: ProcessId = ProcessId(1);

    #[inline]
    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

/// Allocates a new globally unique Process Identifier.
pub fn allocate_pid() -> ProcessId {
    ProcessId(NEXT_PID.fetch_add(1, Ordering::SeqCst))
}

/// The lifecycle state of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Created,
    Ready,
    Running,
    Waiting(Option<ProcessId>), // Blocked waiting for a child process to exit
    Zombie(i32),                // Terminated, holding exit status until parent reaps it
    Terminated,                 // Fully reaped and deallocated
}

/// Process Control Block (PCB) representing an isolated program with its own address space.
pub struct Process {
    pub pid: ProcessId,
    pub name: String,
    pub state: ProcessState,
    pub parent_pid: Option<ProcessId>,
    pub children: Vec<ProcessId>,
    pub threads: Vec<ThreadId>,
    pub address_space: AddressSpace,
    pub fd_table: crate::fs::fd::FileDescriptorTable,
    pub exit_code: Option<i32>,
}

impl Process {
    /// Creates a new process with its own isolated address space.
    pub fn new(name: &str, parent_pid: Option<ProcessId>) -> Result<Self, &'static str> {
        let pid = allocate_pid();
        let address_space = AddressSpace::new()?;

        Ok(Self {
            pid,
            name: String::from(name),
            state: ProcessState::Created,
            parent_pid,
            children: Vec::new(),
            threads: Vec::new(),
            address_space,
            fd_table: crate::fs::fd::FileDescriptorTable::new(),
            exit_code: None,
        })
    }
}

/// Global Process Table storing all active and zombie processes in the operating system.
pub struct ProcessTable {
    processes: BTreeMap<ProcessId, Process>,
}

impl ProcessTable {
    pub const fn new() -> Self {
        Self {
            processes: BTreeMap::new(),
        }
    }

    /// Registers a new process into the table.
    pub fn insert(&mut self, proc: Process) {
        if let Some(parent_id) = proc.parent_pid {
            if let Some(parent) = self.processes.get_mut(&parent_id) {
                parent.children.push(proc.pid);
            }
        }
        self.processes.insert(proc.pid, proc);
    }

    /// Retrieves an immutable reference to a process by PID.
    pub fn get(&self, pid: ProcessId) -> Option<&Process> {
        self.processes.get(&pid)
    }

    /// Retrieves a mutable reference to a process by PID.
    pub fn get_mut(&mut self, pid: ProcessId) -> Option<&mut Process> {
        self.processes.get_mut(&pid)
    }

    /// Removes a terminated process from the table (reaping a zombie).
    pub fn remove(&mut self, pid: ProcessId) -> Option<Process> {
        self.processes.remove(&pid)
    }

    /// Returns the number of processes currently in the table.
    pub fn count(&self) -> usize {
        self.processes.len()
    }
}

/// Global thread-safe Process Table instance protected by Spinlock.
pub static PROCESS_TABLE: Spinlock<ProcessTable> = Spinlock::new(ProcessTable::new());

/// Creates and registers a new process in the global process table.
pub fn create_process(name: &str, parent_pid: Option<ProcessId>) -> Result<ProcessId, &'static str> {
    let proc = Process::new(name, parent_pid)?;
    let pid = proc.pid;
    println!("[PROCESS] Created process #{} \"{}\" with dedicated AddressSpace (PML4 {:#x})",
        pid.0,
        proc.name,
        proc.address_space.pml4_frame().start_address().as_u64()
    );

    PROCESS_TABLE.lock().insert(proc);
    Ok(pid)
}

/// Marks a process as exited (Zombie state) and stores its exit code.
pub fn exit_process(pid: ProcessId, exit_code: i32) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(proc) = table.get_mut(pid) {
        println!("[PROCESS] Process #{} \"{}\" exited with code: {}. Transitioning to Zombie state.",
            pid.0, proc.name, exit_code
        );
        proc.state = ProcessState::Zombie(exit_code);
        proc.exit_code = Some(exit_code);
    }
}

/// Waits for a child process to terminate and reaps its exit status (reaping a zombie).
/// If `target` is `None`, waits for any child. If `Some(pid)`, waits specifically for that child.
pub fn waitpid(parent_pid: ProcessId, target: Option<ProcessId>) -> Result<(ProcessId, i32), &'static str> {
    loop {
        let (found_pid, exit_code) = {
            let mut table = PROCESS_TABLE.lock();
            let parent = table.get(parent_pid).ok_or("Parent process not found in table")?;
            let children: Vec<ProcessId> = parent.children.clone();

            let mut reaped: Option<(ProcessId, i32)> = None;

            for child_id in children {
                if let Some(target_id) = target {
                    if child_id != target_id {
                        continue;
                    }
                }

                if let Some(child) = table.get(child_id) {
                    if let ProcessState::Zombie(code) = child.state {
                        reaped = Some((child_id, code));
                        break;
                    }
                }
            }

            if let Some((child_id, code)) = reaped {
                if let Some(parent) = table.get_mut(parent_pid) {
                    parent.children.retain(|&id| id != child_id);
                }
                table.remove(child_id);
                (child_id, code)
            } else if table.get(parent_pid).map_or(true, |p| p.children.is_empty()) {
                return Err("No child processes to wait for (ECHILD)");
            } else {
                // Children exist, but none have exited yet. Drop lock and yield CPU.
                drop(table);
                super::yield_now();
                continue;
            }
        };

        println!("[PROCESS] Parent #{} reaped zombie child #{}, exit status: {}", parent_pid.0, found_pid.0, exit_code);
        return Ok((found_pid, exit_code));
    }
}

/// Spawns the first user process ("init" / PID 1) from an ELF binary image.
pub fn spawn_init_process(elf_bytes: &[u8]) -> Result<ProcessId, &'static str> {
    println!();
    println!("----------------------------------------");
    println!(" [PROCESS] Spawning First User Process  ");
    println!("----------------------------------------");

    let pid = create_process("init", None)?;
    assert_eq!(pid, ProcessId::INIT, "First process must have PID 1");

    // Load ELF into the active process address space
    let mut table = PROCESS_TABLE.lock();
    let proc = table.get_mut(pid).ok_or("Failed to retrieve init process")?;

    unsafe {
        // Activate the init process's address space
        proc.address_space.activate();
    }

    let loaded_elf = super::elf::load_user_elf(elf_bytes)?;
    println!(
        "[PROCESS] Loaded init ELF binary: entry {:#x}, memory size {} bytes",
        loaded_elf.entry_point,
        loaded_elf.total_memory_size
    );

    // Map user stack page (0x7000_0000)
    let user_stack_vaddr = crate::memory::VirtAddr::new(0x7000_0000);
    let stack_page = crate::memory::Page::containing_address(user_stack_vaddr);
    let stack_frame = crate::memory::alloc_frame().ok_or("Out of memory for init user stack")?;

    crate::memory::paging::map_page(
        stack_page,
        stack_frame,
        crate::memory::PageTableFlags::PRESENT
            | crate::memory::PageTableFlags::WRITABLE
            | crate::memory::PageTableFlags::USER_ACCESSIBLE
            | crate::memory::PageTableFlags::NO_EXECUTE,
    )?;

    proc.state = ProcessState::Ready;

    println!("[PROCESS] First user process \"init\" (PID 1) successfully prepared.");
    Ok(pid)
}

/// Returns the number of processes currently tracked in the process table.
pub fn process_count() -> usize {
    PROCESS_TABLE.lock().count()
}
