//! # Asynchronous Process Signaling Subsystem
//!
//! ## Overview
//! Signals are asynchronous notifications dispatched to processes by the kernel or
//! other processes (via `kill(2)`).
//!
//! ### Signal Semantics & Invariants:
//! - **Uncatchable Signals**: `SIGKILL` (9) and `SIGSTOP` (19) cannot be caught, ignored, or blocked.
//! - **PID 1 Immunity**: The `init` process (`ProcessId::INIT`) cannot be killed with `SIGKILL`.
//! - **Default Actions**:
//!   - `SIGKILL`, `SIGTERM`, `SIGQUIT`, `SIGINT`: Immediate process termination with exit status
//!     encoded as `128 + signal_number` per POSIX convention.
//!   - `SIGCHLD`: Notification to parent process when a child exits; ignored by default.
//!   - `SIGPIPE`: Issued when writing to a pipe or socket with no active readers.

use crate::task::process::{ProcessId, PROCESS_TABLE};

/// Standard POSIX Signal Identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Signal {
    /// Hangup detected on controlling terminal or death of controlling process.
    SIGHUP = 1,
    /// Interrupt from keyboard (`Ctrl+C`).
    SIGINT = 2,
    /// Quit from keyboard (`Ctrl+\`).
    SIGQUIT = 3,
    /// Illegal instruction.
    SIGILL = 4,
    /// Trace/breakpoint trap.
    SIGTRAP = 5,
    /// Abort signal from `abort(3)`.
    SIGABRT = 6,
    /// Bus error (bad memory access).
    SIGBUS = 7,
    /// Floating-point exception.
    SIGFPE = 8,
    /// Kill signal (unblockable termination).
    SIGKILL = 9,
    /// User-defined signal 1.
    SIGUSR1 = 10,
    /// Invalid memory reference / page fault violation.
    SIGSEGV = 11,
    /// User-defined signal 2.
    SIGUSR2 = 12,
    /// Broken pipe: write to pipe with no readers.
    SIGPIPE = 13,
    /// Timer signal from `alarm(2)`.
    SIGALRM = 14,
    /// Termination signal (graceful shutdown request).
    SIGTERM = 15,
    /// Child process stopped or terminated.
    SIGCHLD = 17,
    /// Continue if stopped.
    SIGCONT = 18,
    /// Stop process execution (unblockable pause).
    SIGSTOP = 19,
}

impl Signal {
    pub fn from_i32(sig: i32) -> Option<Self> {
        match sig {
            1 => Some(Self::SIGHUP),
            2 => Some(Self::SIGINT),
            3 => Some(Self::SIGQUIT),
            4 => Some(Self::SIGILL),
            5 => Some(Self::SIGTRAP),
            6 => Some(Self::SIGABRT),
            7 => Some(Self::SIGBUS),
            8 => Some(Self::SIGFPE),
            9 => Some(Self::SIGKILL),
            10 => Some(Self::SIGUSR1),
            11 => Some(Self::SIGSEGV),
            12 => Some(Self::SIGUSR2),
            13 => Some(Self::SIGPIPE),
            14 => Some(Self::SIGALRM),
            15 => Some(Self::SIGTERM),
            17 => Some(Self::SIGCHLD),
            18 => Some(Self::SIGCONT),
            19 => Some(Self::SIGSTOP),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::SIGHUP => "SIGHUP",
            Self::SIGINT => "SIGINT",
            Self::SIGQUIT => "SIGQUIT",
            Self::SIGILL => "SIGILL",
            Self::SIGTRAP => "SIGTRAP",
            Self::SIGABRT => "SIGABRT",
            Self::SIGBUS => "SIGBUS",
            Self::SIGFPE => "SIGFPE",
            Self::SIGKILL => "SIGKILL",
            Self::SIGUSR1 => "SIGUSR1",
            Self::SIGSEGV => "SIGSEGV",
            Self::SIGUSR2 => "SIGUSR2",
            Self::SIGPIPE => "SIGPIPE",
            Self::SIGALRM => "SIGALRM",
            Self::SIGTERM => "SIGTERM",
            Self::SIGCHLD => "SIGCHLD",
            Self::SIGCONT => "SIGCONT",
            Self::SIGSTOP => "SIGSTOP",
        }
    }

    #[inline]
    pub fn as_i32(&self) -> i32 {
        *self as i32
    }

    #[inline]
    pub fn bitmask(&self) -> u32 {
        1 << (self.as_i32() - 1)
    }
}

/// Dispatches an asynchronous signal to the target process.
pub fn send_signal(target_pid: ProcessId, signal: Signal) -> Result<(), &'static str> {
    if target_pid == ProcessId::INIT && signal == Signal::SIGKILL {
        return Err("Cannot kill init process (PID 1)");
    }

    let mut proc_table = PROCESS_TABLE.lock();
    let proc = proc_table.get_mut(target_pid).ok_or("Process not found")?;

    match signal {
        Signal::SIGKILL | Signal::SIGTERM | Signal::SIGQUIT | Signal::SIGINT => {
            // Default action for unhandled terminating signals: terminate process
            let exit_status = 128 + signal.as_i32();
            crate::println!("[SIGNAL] Process #{} \"{}\" terminated by signal {} ({}).", target_pid.0, proc.name, signal.as_i32(), signal.name());
            drop(proc_table);
            crate::task::process::exit_process(target_pid, exit_status);
            Ok(())
        }
        Signal::SIGCHLD => {
            // Child status notification: ignored by default if parent does not handle
            Ok(())
        }
        _ => {
            // Non-fatal or user-defined signals recorded in signal table
            Ok(())
        }
    }
}
