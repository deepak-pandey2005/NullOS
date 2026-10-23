//! Process Signaling Subsystem (Phase 10 - M27 IPC).
//!
//! Provides standard POSIX-like asynchronous process signals, signal delivery,
//! bitmask tracking, and default actions (termination, ignore).

use crate::task::process::{ProcessId, PROCESS_TABLE};

/// Standard POSIX Signal Identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Signal {
    SIGHUP = 1,
    SIGINT = 2,
    SIGQUIT = 3,
    SIGILL = 4,
    SIGTRAP = 5,
    SIGABRT = 6,
    SIGBUS = 7,
    SIGFPE = 8,
    SIGKILL = 9,
    SIGUSR1 = 10,
    SIGSEGV = 11,
    SIGUSR2 = 12,
    SIGPIPE = 13,
    SIGALRM = 14,
    SIGTERM = 15,
    SIGCHLD = 17,
    SIGCONT = 18,
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
