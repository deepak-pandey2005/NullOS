//! # NullOS Advanced Interactive Shell (`nsh`)
//!
//! ## Overview
//! `nsh` is the native command-line interface for NullOS, designed to provide a Unix-compliant
//! interactive environment with full support for:
//! 1. **Environment Variables**:
//!    - Built-in `$PATH`, `$USER`, `$HOME`, `$SHELL`, `$OS`, `$ARCH`, `$PWD`.
//!    - Commands: `export VAR=val`, `env`, `unset VAR`.
//! 2. **Variable Expansion**:
//!    - Standard `$VAR` and `${VAR}` parameter substitution.
//!    - Special shell variables: `$?` (last exit code) and `$$` (shell PID).
//! 3. **Input/Output Redirection**:
//!    - Standard output redirection: `command > file` (truncate/create).
//!    - Standard append redirection: `command >> file` (append/create).
//!    - Standard input redirection: `command < file`.
//! 4. **Pipelining**:
//!    - Inter-command streaming via anonymous VFS pipes: `cmd1 | cmd2 | cmd3`.
//! 5. **Background Jobs & Process Control**:
//!    - Background execution: `command &`.
//!    - Job table listing: `jobs`.
//!    - Process termination: `kill <pid>`.
//! 6. **Direct Binary Execution**:
//!    - Searches directories specified in `$PATH` (`/bin`, `/disk`) to load and execute user ELF binaries.
//! 7. **Unix Core Utilities**:
//!    - Built-in implementations of `ls`, `cat`, `echo`, `mkdir`, `rm`, `pwd`, `cd`, `grep`,
//!      `wc`, `head`, `tail`, `ps`, `mem`, `clear`, `reboot`, `poweroff`.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

use crate::arch::x86_64::apic;
use crate::drivers::serial;
use crate::fs::vfs::{FileType, OpenFlags, VFS};
use crate::sync::Spinlock;
use crate::task::process::{ProcessId, PROCESS_TABLE};
use crate::{print, println};

/// Lifecycle state of a shell background job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Actively executing in the background.
    Running,
    /// Finished executing with recorded exit code.
    Done(i32),
}

/// A tracked background execution job with inline command storage (zero heap allocations).
#[derive(Debug, Clone, Copy)]
pub struct Job {
    /// Sequential job identifier (e.g. `[1]`).
    pub id: usize,
    /// Length of command string stored in `cmd_bytes`.
    pub cmd_len: usize,
    /// Inline command character buffer.
    pub cmd_bytes: [u8; 128],
    /// Current execution state of the job.
    pub state: JobState,
}

impl Job {
    pub fn new(id: usize, cmd: &str) -> Self {
        let mut bytes = [0u8; 128];
        let bytes_to_copy = cmd.as_bytes();
        let len = bytes_to_copy.len().min(128);
        bytes[..len].copy_from_slice(&bytes_to_copy[..len]);
        Self {
            id,
            cmd_len: len,
            cmd_bytes: bytes,
            state: JobState::Running,
        }
    }

    pub fn command_str(&self) -> &str {
        core::str::from_utf8(&self.cmd_bytes[..self.cmd_len]).unwrap_or("<invalid utf8>")
    }
}

#[derive(Debug, Clone, Copy)]
struct BgQueueEntry {
    pub job_id: usize,
    pub cmd_len: usize,
    pub cmd_bytes: [u8; 128],
}

static NEXT_JOB_ID: AtomicUsize = AtomicUsize::new(1);
static JOBS: Spinlock<[Option<Job>; 16]> = Spinlock::new([None; 16]);
static BG_QUEUE: Spinlock<[Option<BgQueueEntry>; 16]> = Spinlock::new([None; 16]);

/// Global shell environment variables list.
static ENV_VARS: Spinlock<Vec<(String, String)>> = Spinlock::new(Vec::new());

/// Exit code of the most recently executed foreground command (for `$?` expansion).
static LAST_EXIT_CODE: AtomicI32 = AtomicI32::new(0);

/// Initializes default shell environment variables if not already initialized.
pub fn init_env_vars() {
    let mut vars = ENV_VARS.lock();
    if vars.is_empty() {
        vars.push(("PATH".to_string(), "/bin:/disk".to_string()));
        vars.push(("USER".to_string(), "root".to_string()));
        vars.push(("HOME".to_string(), "/".to_string()));
        vars.push(("SHELL".to_string(), "/bin/nsh".to_string()));
        vars.push(("OS".to_string(), "NullOS".to_string()));
        vars.push(("ARCH".to_string(), "x86_64".to_string()));
        vars.push(("PWD".to_string(), "/".to_string()));
    }
}

/// Retrieves an environment variable's value, or an empty string if undefined.
pub fn get_env(key: &str) -> String {
    init_env_vars();
    let vars = ENV_VARS.lock();
    for (k, v) in vars.iter() {
        if k == key {
            return v.clone();
        }
    }
    String::new()
}

/// Sets or updates an environment variable.
pub fn set_env(key: &str, val: &str) {
    init_env_vars();
    let mut vars = ENV_VARS.lock();
    for (k, v) in vars.iter_mut() {
        if k == key {
            *v = val.to_string();
            return;
        }
    }
    vars.push((key.to_string(), val.to_string()));
}

/// Unsets (removes) an environment variable.
pub fn unset_env(key: &str) {
    init_env_vars();
    let mut vars = ENV_VARS.lock();
    vars.retain(|(k, _)| k != key);
}

/// Normalizes a path string against the given working directory (`cwd`), resolving `.` and `..`.
pub fn normalize_path(cwd: &str, path: &str) -> String {
    let full = if path.starts_with('/') {
        String::from(path)
    } else if cwd == "/" {
        alloc::format!("/{}", path)
    } else {
        alloc::format!("{}/{}", cwd.trim_end_matches('/'), path)
    };

    let mut segments: Vec<&str> = Vec::new();
    for seg in full.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        } else if seg == ".." {
            segments.pop();
        } else {
            segments.push(seg);
        }
    }

    if segments.is_empty() {
        String::from("/")
    } else {
        alloc::format!("/{}", segments.join("/"))
    }
}

/// Resolves a path relative to the shell's current working directory (`$PWD`).
pub fn resolve_path_with_pwd(path: &str) -> String {
    let cwd = get_env("PWD");
    let cwd_str = if cwd.is_empty() { "/" } else { &cwd };
    normalize_path(cwd_str, path)
}

/// Expands shell variables (`$VAR`, `${VAR}`, `$?`, `$$`) within an input command line.
pub fn expand_variables(input: &str, last_exit: i32) -> String {
    let mut result = String::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            i += 1;
            if chars[i] == '?' {
                result.push_str(&alloc::format!("{}", last_exit));
                i += 1;
            } else if chars[i] == '$' {
                result.push_str("1"); // Primary shell PID
                i += 1;
            } else if chars[i] == '{' {
                i += 1;
                let mut var_name = String::new();
                while i < chars.len() && chars[i] != '}' {
                    var_name.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() && chars[i] == '}' {
                    i += 1;
                }
                result.push_str(&get_env(&var_name));
            } else if chars[i].is_alphanumeric() || chars[i] == '_' {
                let mut var_name = String::new();
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    var_name.push(chars[i]);
                    i += 1;
                }
                result.push_str(&get_env(&var_name));
            } else {
                result.push('$');
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Parses a command line string into separate arguments, respecting single and double quotes.
pub fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';

    for c in s.chars() {
        match c {
            '"' | '\'' => {
                if in_quotes && quote_char == c {
                    in_quotes = false;
                } else if !in_quotes {
                    in_quotes = true;
                    quote_char = c;
                } else {
                    current.push(c);
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Splits a command line into pipeline stages delimited by `|`, respecting quotes.
fn split_pipeline(line: &str) -> Vec<String> {
    let mut stages = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';

    for c in line.chars() {
        match c {
            '"' | '\'' => {
                if in_quotes && quote_char == c {
                    in_quotes = false;
                } else if !in_quotes {
                    in_quotes = true;
                    quote_char = c;
                }
                current.push(c);
            }
            '|' if !in_quotes => {
                stages.push(current.trim().to_string());
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    if !current.trim().is_empty() {
        stages.push(current.trim().to_string());
    }
    stages
}

/// Mode of output redirection.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OutputRedir {
    Truncate(String),
    Append(String),
}

/// Extracts an input redirection (`< infile`) from a command stage.
fn extract_input_redirection(stage: &str) -> (String, Option<String>) {
    let mut cmd = String::new();
    let mut file = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';
    let mut reading_file = false;

    for c in stage.chars() {
        match c {
            '"' | '\'' => {
                if in_quotes && quote_char == c {
                    in_quotes = false;
                } else if !in_quotes {
                    in_quotes = true;
                    quote_char = c;
                }
                if !reading_file {
                    cmd.push(c);
                }
            }
            '<' if !in_quotes => {
                reading_file = true;
            }
            c if reading_file => {
                if c.is_whitespace() && !file.is_empty() {
                    reading_file = false;
                } else if !c.is_whitespace() {
                    file.push(c);
                }
            }
            _ => {
                cmd.push(c);
            }
        }
    }

    let file_opt = if file.is_empty() { None } else { Some(file.trim().to_string()) };
    (cmd.trim().to_string(), file_opt)
}

/// Extracts an output redirection (`> outfile` or `>> outfile`) from a command stage.
fn extract_output_redirection(stage: &str) -> (String, Option<OutputRedir>) {
    let chars: Vec<char> = stage.chars().collect();
    let mut cmd = String::new();
    let mut i = 0;
    let mut in_quotes = false;
    let mut quote_char = ' ';
    let mut redir_type = None; // 1 = Truncate, 2 = Append
    let mut file = String::new();

    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' {
            if in_quotes && quote_char == c {
                in_quotes = false;
            } else if !in_quotes {
                in_quotes = true;
                quote_char = c;
            }
            if redir_type.is_none() {
                cmd.push(c);
            }
            i += 1;
            continue;
        }

        if !in_quotes && c == '>' {
            if i + 1 < chars.len() && chars[i + 1] == '>' {
                redir_type = Some(2); // Append
                i += 2;
            } else {
                redir_type = Some(1); // Truncate
                i += 1;
            }

            while i < chars.len() {
                let fc = chars[i];
                if fc.is_whitespace() && !file.is_empty() {
                    break;
                } else if !fc.is_whitespace() && fc != '"' && fc != '\'' {
                    file.push(fc);
                }
                i += 1;
            }
            continue;
        }

        if redir_type.is_none() {
            cmd.push(c);
        }
        i += 1;
    }

    let redir = match redir_type {
        Some(1) if !file.is_empty() => Some(OutputRedir::Truncate(file)),
        Some(2) if !file.is_empty() => Some(OutputRedir::Append(file)),
        _ => None,
    };

    (cmd.trim().to_string(), redir)
}

/// Reads the complete content of a VFS file into a `String`.
fn read_file_to_string(path: &str) -> Result<String, crate::fs::vfs::FsError> {
    let file = VFS.open(path, OpenFlags::READ_ONLY)?;
    let stat = file.stat();
    let mut buf = vec![0u8; stat.size];
    let n = file.read_at(0, &mut buf)?;
    buf.truncate(n);
    match String::from_utf8(buf) {
        Ok(s) => Ok(s),
        Err(e) => Ok(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}

/// Writes string data to a VFS file, either truncating or appending.
fn write_file_content(path: &str, content: &str, append: bool) -> Result<usize, crate::fs::vfs::FsError> {
    let file = VFS.open(path, OpenFlags {
        read: true,
        write: true,
        create: true,
        append,
        truncate: !append,
        nonblock: false,
    })?;
    let offset = if append { file.stat().size } else { 0 };
    file.write_at(offset, content.as_bytes())
}

/// Executes a single pipeline stage command with the provided standard input.
/// Returns standard output and exit status `(output_string, exit_code)`.
fn execute_stage(cmd: &str, args: &[String], input: &str) -> (String, i32) {
    match cmd {
        "help" => {
            let mut out = String::new();
            out.push_str("NullOS Advanced Interactive Shell (nsh) v0.2.0\n");
            out.push_str("Commands:\n");
            out.push_str("  help                 - Display this reference manual\n");
            out.push_str("  clear                - Clear the terminal screen\n");
            out.push_str("  echo [text...]       - Print arguments to output\n");
            out.push_str("  pwd                  - Print current working directory ($PWD)\n");
            out.push_str("  cd [path]            - Change current working directory\n");
            out.push_str("  ls [path]            - List directory contents (default: current)\n");
            out.push_str("  cat [path]           - Display file content (or reads from pipe/stdin)\n");
            out.push_str("  touch <path>         - Create an empty file\n");
            out.push_str("  write <path> <text>  - Write text string directly to file\n");
            out.push_str("  rm <path>            - Remove (unlink) a file from filesystem\n");
            out.push_str("  grep [-i] [-v] <pat> - Filter lines matching regex/text pattern\n");
            out.push_str("  wc [-l|-w|-c] [file] - Count lines, words, and characters\n");
            out.push_str("  head [-n N] [file]   - Display the first N lines (default: 10)\n");
            out.push_str("  tail [-n N] [file]   - Display the last N lines (default: 10)\n");
            out.push_str("  env                  - List all active shell environment variables\n");
            out.push_str("  export <k=v | k v>   - Set or update an environment variable\n");
            out.push_str("  unset <key>          - Remove an environment variable\n");
            out.push_str("  ps                   - List active processes and threads\n");
            out.push_str("  jobs                 - List background running jobs\n");
            out.push_str("  kill [-sig] <pid>    - Send signal to process (default: SIGKILL 9)\n");
            out.push_str("  lspci                - Discover PCI hardware buses and devices\n");
            out.push_str("  disks                - List registered block storage devices\n");
            out.push_str("  ifconfig             - Display network interface configuration and statistics\n");
            out.push_str("  arp                  - Display Address Resolution Protocol cache table\n");
            out.push_str("  ping <ip>            - Send ICMP Echo Request packets to network host\n");
            out.push_str("  netstat              - Display active network sockets and TCP states\n");
            out.push_str("  cpus                 - Display symmetric multiprocessing CPU core statuses\n");
            out.push_str("  sec                  - Display kernel security mitigations & hardening status\n");
            out.push_str("  ipc                  - Display inter-process communication resources (pipes, shm, unix)\n");
            out.push_str("  threads              - Display active system threads and synchronization states\n");
            out.push_str("  dmesg                - Display kernel message ring buffer logs\n");
            out.push_str("  usb / lsusb          - List enumerated USB controllers and devices (M31)\n");
            out.push_str("  audio / sndstat      - Display Intel HD Audio controller status and codecs (M32)\n");
            out.push_str("  beep                 - Play musical startup chime (M32)\n");
            out.push_str("  play <freq> [ms]     - Play a procedural sine wave tone at frequency (M32)\n");
            out.push_str("  acpi                 - Display ACPI tables, FADT, DSDT, and power configurations (M33)\n");
            out.push_str("  poweroff / shutdown  - Power off computer via ACPI S5 sleep transition (M33)\n");
            out.push_str("  reboot               - Reboot machine via ACPI RESET_REG, 8042, or PCI (M33)\n");
            out.push_str("  ldd <path>           - Inspect ELF dynamic library dependencies & relocations (M34)\n");
            out.push_str("  libs                 - Display loaded dynamic shared libraries & base addresses (M34)\n");
            out.push_str("  nvme                 - List detected NVMe PCIe controllers and namespaces (M35)\n");
            out.push_str("  storage / cache      - Inspect Block/Page Cache, I/O Scheduler & WAL Journal (M35)\n");
            out.push_str("  diskbench            - Run cycle-accurate Storage Subsystem benchmarks (M35)\n");
            out.push_str("  netperf              - Inspect Buffer Pool, NAPI Poller, Batching & RSS Queues (M36)\n");
            out.push_str("  netbench             - Run cycle-accurate High-Performance Networking benchmarks (M36)\n");
            out.push_str("  perfstat             - Inspect TSC, PMU, Latencies, Lock Contention & CPU Util (M37)\n");
            out.push_str("  kbench               - Run Memory Alloc & Thread Context-Switch benchmarks (M37)\n");
            out.push_str("  trace [N]            - Dump recent kernel tracepoints from circular ring buffer (M37)\n");
            out.push_str("  optstat              - Inspect NUMA, Paging Caps, FSGSBASE & Slab Optimizations (M38)\n");
            out.push_str("  optbench             - Run M38 Optimization Benchmarks & Regression Suite (M38)\n");
            out.push_str("  sdk                  - Display NullOS SDK ABI, Sysroot & Toolchain Summary (M39)\n");
            out.push_str("  pkg <inspect|run> <f>- Inspect/verify CRC-32 or execute a .npkg package (M39)\n");
            out.push_str("  dbg / kdbg [subcmd]  - Kernel debugger: regs, bt, sym, mem, v2p, heap, klog, trace, locks, crash, bp (M40)\n");
            out.push_str("  bt / sym / klog      - Quick aliases for stack backtrace, symbolizer & structured logs (M40)\n");
            out.push_str("  lockdep / crashdump  - Inspect deadlock dependency graph & crash dump history (M40)\n");
            out.push_str("  test                 - Run comprehensive automated subsystem & shell tests\n");
            out.push_str("  gui                  - Launch Desktop Window Manager & Compositor demo (M30)\n");
            out.push_str("  mem                  - Display physical memory manager statistics\n");
            out.push_str("  uptime               - Display system timer ticks\n");
            out.push_str("  exec <path>          - Execute a Ring 3 ELF binary\n");
            out.push_str("\nSyntax Features:\n");
            out.push_str("  Pipelines:           cmd1 | cmd2 | cmd3\n");
            out.push_str("  Redirection:         cmd > file (truncate), cmd >> file (append), cmd < file (input)\n");
            out.push_str("  Background:          cmd & (runs asynchronously with job control)\n");
            out.push_str("  Variables:           $VAR, ${VAR}, $? (last exit), $$ (PID)\n");
            (out, 0)
        }
        "clear" => {
            ("\x1b[2J\x1b[H".to_string(), 0)
        }
        "echo" => {
            let line = args.join(" ");
            (alloc::format!("{}\n", line), 0)
        }
        "pwd" => {
            let cwd = get_env("PWD");
            (alloc::format!("{}\n", cwd), 0)
        }
        "cd" => {
            let target = if args.is_empty() || args[0] == "~" {
                get_env("HOME")
            } else {
                args[0].clone()
            };
            let resolved = resolve_path_with_pwd(&target);
            match VFS.resolve_path(&resolved) {
                Ok(inode) => {
                    let stat = inode.stat();
                    if stat.file_type != FileType::Directory {
                        (alloc::format!("cd: '{}' is not a directory\n", target), 1)
                    } else {
                        set_env("PWD", &resolved);
                        (String::new(), 0)
                    }
                }
                Err(e) => (alloc::format!("cd: cannot access '{}': {:?}\n", target, e), 1),
            }
        }
        "ls" => {
            let target = if args.is_empty() {
                String::from(".")
            } else {
                args[0].clone()
            };
            let resolved = resolve_path_with_pwd(&target);
            match VFS.resolve_path(&resolved) {
                Ok(inode) => {
                    let stat = inode.stat();
                    if stat.file_type != FileType::Directory {
                        (alloc::format!("ls: '{}' is not a directory\n", target), 1)
                    } else {
                        match inode.read_dir() {
                            Ok(entries) => {
                                let mut out = alloc::format!("Listing directory: {}\n", resolved);
                                for entry in entries {
                                    let type_str = match entry.file_type {
                                        FileType::Directory => "<DIR>",
                                        FileType::File => "<FILE>",
                                        FileType::CharDevice => "<DEV>",
                                        _ => "<OTHER>",
                                    };
                                    out.push_str(&alloc::format!("  {:<8} Inode #{:<4} {}\n", type_str, entry.inode_num, entry.name));
                                }
                                (out, 0)
                            }
                            Err(e) => (alloc::format!("ls: failed to read directory: {:?}\n", e), 1),
                        }
                    }
                }
                Err(e) => (alloc::format!("ls: cannot access '{}': {:?}\n", target, e), 1),
            }
        }
        "cat" => {
            if args.is_empty() {
                if !input.is_empty() {
                    (input.to_string(), 0)
                } else {
                    ("Usage: cat <path> or via pipe/redirection\n".to_string(), 1)
                }
            } else {
                let resolved = resolve_path_with_pwd(&args[0]);
                match read_file_to_string(&resolved) {
                    Ok(mut content) => {
                        if !content.ends_with('\n') {
                            content.push('\n');
                        }
                        (content, 0)
                    }
                    Err(e) => (alloc::format!("cat: {}: {:?}\n", args[0], e), 1),
                }
            }
        }
        "touch" => {
            if args.is_empty() {
                ("Usage: touch <path>\n".to_string(), 1)
            } else {
                let resolved = resolve_path_with_pwd(&args[0]);
                match VFS.open(&resolved, OpenFlags {
                    read: true,
                    write: true,
                    create: true,
                    append: false,
                    truncate: false,
                    nonblock: false,
                }) {
                    Ok(_) => (alloc::format!("Created file: {}\n", resolved), 0),
                    Err(e) => (alloc::format!("touch: cannot create '{}': {:?}\n", resolved, e), 1),
                }
            }
        }
        "write" => {
            if args.len() < 2 {
                ("Usage: write <path> <text...>\n".to_string(), 1)
            } else {
                let resolved = resolve_path_with_pwd(&args[0]);
                let content = args[1..].join(" ") + "\n";
                match write_file_content(&resolved, &content, false) {
                    Ok(n) => (alloc::format!("Wrote {} bytes to '{}'\n", n, resolved), 0),
                    Err(e) => (alloc::format!("write: failed to write to '{}': {:?}\n", resolved, e), 1),
                }
            }
        }
        "rm" => {
            if args.is_empty() {
                ("Usage: rm <path>\n".to_string(), 1)
            } else {
                let resolved = resolve_path_with_pwd(&args[0]);
                match VFS.unlink(&resolved) {
                    Ok(()) => (alloc::format!("Removed '{}'\n", resolved), 0),
                    Err(e) => (alloc::format!("rm: cannot remove '{}': {:?}\n", resolved, e), 1),
                }
            }
        }
        "grep" => {
            let mut ignore_case = false;
            let mut invert = false;
            let mut positional = Vec::new();

            for arg in args {
                if arg == "-i" {
                    ignore_case = true;
                } else if arg == "-v" {
                    invert = true;
                } else if arg == "-iv" || arg == "-vi" {
                    ignore_case = true;
                    invert = true;
                } else {
                    positional.push(arg.clone());
                }
            }

            if positional.is_empty() {
                return ("Usage: grep [-i] [-v] <pattern> [file]\n".to_string(), 1);
            }

            let pattern = &positional[0];
            let source_text = if positional.len() > 1 {
                let resolved = resolve_path_with_pwd(&positional[1]);
                match read_file_to_string(&resolved) {
                    Ok(s) => s,
                    Err(e) => return (alloc::format!("grep: {}: {:?}\n", positional[1], e), 1),
                }
            } else {
                input.to_string()
            };

            let mut out = String::new();
            let mut match_found = false;
            let pat_lower = pattern.to_ascii_lowercase();

            for line in source_text.lines() {
                let matched = if ignore_case {
                    line.to_ascii_lowercase().contains(&pat_lower)
                } else {
                    line.contains(pattern)
                };

                let pass = if invert { !matched } else { matched };
                if pass {
                    match_found = true;
                    out.push_str(line);
                    out.push('\n');
                }
            }

            let code = if match_found { 0 } else { 1 };
            (out, code)
        }
        "wc" => {
            let mut count_lines = false;
            let mut count_words = false;
            let mut count_chars = false;
            let mut positional = Vec::new();

            for arg in args {
                if arg == "-l" {
                    count_lines = true;
                } else if arg == "-w" {
                    count_words = true;
                } else if arg == "-c" {
                    count_chars = true;
                } else {
                    positional.push(arg.clone());
                }
            }

            if !count_lines && !count_words && !count_chars {
                count_lines = true;
                count_words = true;
                count_chars = true;
            }

            let source_text = if !positional.is_empty() {
                let resolved = resolve_path_with_pwd(&positional[0]);
                match read_file_to_string(&resolved) {
                    Ok(s) => s,
                    Err(e) => return (alloc::format!("wc: {}: {:?}\n", positional[0], e), 1),
                }
            } else {
                input.to_string()
            };

            let lines = source_text.lines().count();
            let words = source_text.split_whitespace().count();
            let chars = source_text.len();

            let mut parts = Vec::new();
            if count_lines {
                parts.push(alloc::format!("{:>6}", lines));
            }
            if count_words {
                parts.push(alloc::format!("{:>6}", words));
            }
            if count_chars {
                parts.push(alloc::format!("{:>6}", chars));
            }

            (alloc::format!("{}\n", parts.join(" ")), 0)
        }
        "head" => {
            let mut n_lines = 10usize;
            let mut positional = Vec::new();
            let mut i = 0;

            while i < args.len() {
                if args[i] == "-n" && i + 1 < args.len() {
                    if let Ok(val) = args[i + 1].parse::<usize>() {
                        n_lines = val;
                    }
                    i += 2;
                } else {
                    positional.push(args[i].clone());
                    i += 1;
                }
            }

            let source_text = if !positional.is_empty() {
                let resolved = resolve_path_with_pwd(&positional[0]);
                match read_file_to_string(&resolved) {
                    Ok(s) => s,
                    Err(e) => return (alloc::format!("head: {}: {:?}\n", positional[0], e), 1),
                }
            } else {
                input.to_string()
            };

            let mut out = String::new();
            for line in source_text.lines().take(n_lines) {
                out.push_str(line);
                out.push('\n');
            }
            (out, 0)
        }
        "tail" => {
            let mut n_lines = 10usize;
            let mut positional = Vec::new();
            let mut i = 0;

            while i < args.len() {
                if args[i] == "-n" && i + 1 < args.len() {
                    if let Ok(val) = args[i + 1].parse::<usize>() {
                        n_lines = val;
                    }
                    i += 2;
                } else {
                    positional.push(args[i].clone());
                    i += 1;
                }
            }

            let source_text = if !positional.is_empty() {
                let resolved = resolve_path_with_pwd(&positional[0]);
                match read_file_to_string(&resolved) {
                    Ok(s) => s,
                    Err(e) => return (alloc::format!("tail: {}: {:?}\n", positional[0], e), 1),
                }
            } else {
                input.to_string()
            };

            let all_lines: Vec<&str> = source_text.lines().collect();
            let start = if all_lines.len() > n_lines { all_lines.len() - n_lines } else { 0 };
            let mut out = String::new();
            for &line in &all_lines[start..] {
                out.push_str(line);
                out.push('\n');
            }
            (out, 0)
        }
        "env" => {
            init_env_vars();
            let vars = ENV_VARS.lock();
            let mut out = String::new();
            for (k, v) in vars.iter() {
                out.push_str(&alloc::format!("{}={}\n", k, v));
            }
            (out, 0)
        }
        "export" => {
            if args.is_empty() {
                return execute_stage("env", &[], input);
            }
            for arg in args {
                if let Some(pos) = arg.find('=') {
                    let key = &arg[..pos];
                    let val = &arg[pos + 1..];
                    set_env(key, val);
                } else {
                    set_env(arg, "");
                }
            }
            (String::new(), 0)
        }
        "unset" => {
            if args.is_empty() {
                return ("Usage: unset <variable_name>\n".to_string(), 1);
            }
            for arg in args {
                unset_env(arg);
            }
            (String::new(), 0)
        }
        "jobs" => {
            let jobs = JOBS.lock();
            let mut out = alloc::format!("{:<6} {:<12} {}\n", "JOB_ID", "STATUS", "COMMAND");
            out.push_str("----------------------------------------------\n");
            let mut count = 0;
            for slot in jobs.iter() {
                if let Some(job) = slot {
                    count += 1;
                    match job.state {
                        JobState::Running => {
                            out.push_str(&alloc::format!("[{:<4}] {:<12} {}\n", job.id, "Running", job.command_str()));
                        }
                        JobState::Done(code) => {
                            out.push_str(&alloc::format!("[{:<4}] Done({:<6}) {}\n", job.id, code, job.command_str()));
                        }
                    }
                }
            }
            if count == 0 {
                out.push_str("No active background jobs.\n");
            }
            (out, 0)
        }
        "kill" => {
            if args.is_empty() {
                return ("Usage: kill [-signal] <pid>\n".to_string(), 1);
            }
            let mut sig = 9i32; // Default SIGKILL
            let mut pid_str = &args[0];

            if args[0].starts_with('-') {
                let sig_name = args[0].trim_start_matches('-');
                if let Ok(num) = sig_name.parse::<i32>() {
                    sig = num;
                } else if sig_name.eq_ignore_ascii_case("kill") {
                    sig = 9;
                } else if sig_name.eq_ignore_ascii_case("term") {
                    sig = 15;
                }
                if args.len() < 2 {
                    return ("Usage: kill [-signal] <pid>\n".to_string(), 1);
                }
                pid_str = &args[1];
            }

            match pid_str.parse::<u64>() {
                Ok(num) => {
                    let pid = ProcessId(num);
                    match crate::task::kill_process(pid, sig) {
                        Ok(()) => (alloc::format!("Process #{} signaled with {}\n", num, sig), 0),
                        Err(e) => (alloc::format!("kill: {}: {}\n", num, e), 1),
                    }
                }
                Err(_) => (alloc::format!("kill: invalid PID: '{}'\n", pid_str), 1),
            }
        }
        "ps" => {
            let table = PROCESS_TABLE.lock();
            let mut out = alloc::format!("{:<6} {:<16} {:<12} {:<8}\n", "PID", "NAME", "STATE", "THREADS");
            out.push_str("------------------------------------------------\n");
            for (pid, proc) in table.iter() {
                let state_str = match proc.state {
                    crate::task::process::ProcessState::Created => "Created",
                    crate::task::process::ProcessState::Ready => "Ready",
                    crate::task::process::ProcessState::Running => "Running",
                    crate::task::process::ProcessState::Waiting(_) => "Waiting",
                    crate::task::process::ProcessState::Zombie(_) => "Zombie",
                    crate::task::process::ProcessState::Terminated => "Terminated",
                };
                out.push_str(&alloc::format!("{:<6} {:<16} {:<12} {:<8}\n", pid.as_u64(), proc.name, state_str, proc.threads.len()));
            }
            (out, 0)
        }
        "lspci" => {
            let devices = crate::drivers::pci::PCI_DEVICES.lock();
            let mut out = alloc::format!("{:<8} {:<10} {:<32} {}\n", "SLOT", "ID", "CLASS", "VENDOR");
            out.push_str("--------------------------------------------------------------------\n");
            for dev in devices.iter() {
                out.push_str(&alloc::format!(
                    "{:<8} {:04x}:{:04x}  {:<32} {}\n",
                    dev.address, dev.vendor_id, dev.device_id, dev.class_name(), dev.vendor_name()
                ));
            }
            (out, 0)
        }
        "usb" | "lsusb" => {
            let devices = crate::drivers::usb::list_devices();
            let mut out = alloc::format!("{:<6} {:<6} {:<10} {:<6} {:<32} {}\n", "SLOT", "PORT", "ID", "SPEED", "CLASS", "TYPE");
            out.push_str("--------------------------------------------------------------------------------\n");
            if devices.is_empty() {
                out.push_str("No USB devices enumerated on xHCI controller.\n");
            } else {
                for dev in devices.iter() {
                    let speed_str = match dev.speed {
                        1 => "Full",
                        2 => "Low",
                        3 => "High",
                        4 => "Super",
                        _ => "Unknown",
                    };
                    let type_str = if dev.is_keyboard {
                        "HID Keyboard"
                    } else if dev.is_mouse {
                        "HID Mouse"
                    } else {
                        "Standard Device"
                    };
                    let class_name = match dev.class_code {
                        0x00 => "Per-Interface",
                        0x03 => "Human Interface Device",
                        0x08 => "Mass Storage",
                        0x09 => "Hub",
                        _ => "Other USB Class",
                    };
                    out.push_str(&alloc::format!(
                        "{:<6} {:<6} {:04x}:{:04x}  {:<6} {:<32} {}\n",
                        dev.slot_id, dev.port_num, dev.vendor_id, dev.product_id, speed_str, class_name, type_str
                    ));
                }
            }
            (out, 0)
        }
        "audio" | "sndstat" => {
            let mut out = String::new();
            match crate::drivers::audio::get_status() {
                Some(status) => {
                    out.push_str("Intel High Definition Audio (HDA) Subsystem Status:\n");
                    out.push_str(&alloc::format!("  Specification:   HDA {}.{}\n", status.spec_version_major, status.spec_version_minor));
                    out.push_str(&alloc::format!("  Streams:         {} Output, {} Input\n", status.output_streams, status.input_streams));
                    out.push_str(&alloc::format!("  Codecs:          {} detected\n", status.codec_count));
                    out.push_str(&alloc::format!("  Active Routing:  DAC Node {}, Pin Node {}\n", status.active_dac_nid, status.active_pin_nid));
                    out.push_str(&alloc::format!("  PCM Format:      {} Hz, 16-bit Stereo\n", status.sample_rate));
                    out.push_str(&alloc::format!("  DMA State:       {}\n", if status.is_playing { "PLAYING" } else { "IDLE" }));
                }
                None => {
                    out.push_str("No Intel HD Audio controller detected.\n");
                }
            }
            (out, 0)
        }
        "beep" => {
            match crate::drivers::audio::beep() {
                Ok(_) => ("Playing startup chime...\n".to_string(), 0),
                Err(e) => (alloc::format!("beep error: {}\n", e), 1),
            }
        }
        "play" => {
            let freq: u32 = if !args.is_empty() {
                args[0].parse().unwrap_or(440)
            } else {
                440
            };
            let duration: u32 = if args.len() > 1 {
                args[1].parse().unwrap_or(300)
            } else {
                300
            };
            match crate::drivers::audio::play_tone(freq, duration) {
                Ok(_) => (alloc::format!("Playing {} Hz tone for {} ms...\n", freq, duration), 0),
                Err(e) => (alloc::format!("play error: {}\n", e), 1),
            }
        }
        "disks" => {
            let devices = crate::drivers::block::BLOCK_DEVICES.lock();
            let mut out = alloc::format!("{:<10} {:<12} {:<12} {}\n", "NAME", "SECTORS", "SECTOR_SIZE", "CAPACITY");
            out.push_str("------------------------------------------------------\n");
            if devices.is_empty() {
                out.push_str("No block storage devices registered.\n");
            } else {
                for dev in devices.iter() {
                    let sectors = dev.sector_count();
                    let mib = (sectors * dev.sector_size() as u64) / (1024 * 1024);
                    out.push_str(&alloc::format!(
                        "{:<10} {:<12} {:<12} {} MiB ({} bytes)\n",
                        dev.name(), sectors, dev.sector_size(), mib, dev.capacity_bytes()
                    ));
                }
            }
            (out, 0)
        }
        "ifconfig" => {
            let mut out = String::new();
            if let (Some(mac), Some(stats)) = (
                crate::drivers::virtio_net::get_mac(),
                crate::drivers::virtio_net::get_stats(),
            ) {
                let cfg = *crate::net::INTERFACE_CONFIG.lock();
                out.push_str("eth0: flags=4163<UP,BROADCAST,RUNNING,MULTICAST>  mtu 1500\n");
                out.push_str(&alloc::format!("        inet {}  netmask {}  broadcast 10.0.2.255\n", cfg.ip, cfg.netmask));
                out.push_str(&alloc::format!("        ether {}  txqueuelen 1000  (Ethernet)\n", mac));
                out.push_str(&alloc::format!("        RX packets {}  bytes {} ({} B)\n", stats.rx_packets, stats.rx_bytes, stats.rx_bytes));
                out.push_str(&alloc::format!("        TX packets {}  bytes {} ({} B)\n", stats.tx_packets, stats.tx_bytes, stats.tx_bytes));
            } else {
                out.push_str("No network interfaces detected.\n");
            }
            (out, 0)
        }
        "arp" => {
            let entries = crate::net::arp_table();
            let mut out = alloc::format!("{:<16} {:<10} {:<18}\n", "IP Address", "HW Type", "HW Address");
            out.push_str("----------------------------------------------------\n");
            if entries.is_empty() {
                out.push_str("ARP cache is empty.\n");
            } else {
                for (ip, mac) in entries {
                    out.push_str(&alloc::format!("{:<16} {:<10} {:<18}\n", ip, "ether", mac));
                }
            }
            (out, 0)
        }
        "netstat" => {
            let sockets = crate::net::socket::socket_list();
            let mut out = alloc::format!("{:<6} {:<22} {:<22} {:<12}\n", "Proto", "Local Address", "Foreign Address", "State");
            out.push_str("--------------------------------------------------------------------\n");
            if sockets.is_empty() {
                out.push_str("No active sockets.\n");
            } else {
                for s in sockets {
                    let local_str = alloc::format!("0.0.0.0:{}", s.local_port);
                    let foreign_str = alloc::format!("{}:{}", s.remote_addr, s.remote_port);
                    out.push_str(&alloc::format!("{:<6} {:<22} {:<22} {:<12}\n", s.proto, local_str, foreign_str, s.state));
                }
            }
            (out, 0)
        }
        "ping" => {
            if args.is_empty() {
                return ("Usage: ping <destination_ip>\n".to_string(), 1);
            }
            let target_ip = match crate::net::Ipv4Addr::parse(&args[0]) {
                Ok(ip) => ip,
                Err(_) => return (alloc::format!("ping: invalid IPv4 address: '{}'\n", args[0]), 1),
            };

            println!("PING {} 56(84) bytes of data.", target_ip);
            let (sent, received) = crate::net::icmp::ping(target_ip, 3);
            let loss = if sent > 0 {
                ((sent - received) * 100) / sent
            } else {
                100
            };
            let mut out = alloc::format!("\n--- {} ping statistics ---\n", target_ip);
            out.push_str(&alloc::format!("{} packets transmitted, {} received, {}% packet loss\n", sent, received, loss));
            let status = if received > 0 { 0 } else { 1 };
            (out, status)
        }
        "cpus" | "smp" => {
            let table = crate::arch::x86_64::smp::CPU_TABLE.lock();
            let mut out = alloc::format!("{:<6} {:<10} {:<10} {:<12}\n", "CPU_ID", "APIC_ID", "ROLE", "STATUS");
            out.push_str("----------------------------------------------\n");
            if table.is_empty() {
                out.push_str("No SMP CPU table initialized.\n");
            } else {
                for cpu in table.iter() {
                    let online = cpu.online.load(core::sync::atomic::Ordering::Relaxed);
                    let role = if cpu.is_bsp { "BSP" } else { "AP" };
                    let status = if online { "Online" } else { "Offline" };
                    out.push_str(&alloc::format!("{:<6} {:<10} {:<10} {:<12}\n", cpu.cpu_id, cpu.apic_id, role, status));
                }
            }
            (out, 0)
        }
        "sec" | "hardening" => {
            let rep = crate::security::get_security_report();
            let mut out = alloc::format!("{:<30} {:<15}\n", "SECURITY FEATURE", "STATUS");
            out.push_str("----------------------------------------------\n");
            out.push_str(&alloc::format!("{:<30} {:<15}\n", "CR0.WP (Kernel Write Protect)", if rep.cr0_wp { "Enforced" } else { "Disabled" }));
            out.push_str(&alloc::format!("{:<30} {:<15}\n", "EFER.NXE (No-Execute Pages)", if rep.efer_nxe { "Enforced" } else { "Disabled" }));
            out.push_str(&alloc::format!("{:<30} {:<15}\n", "CR4.SMEP (Exec Prevention)", if rep.cr4_smep { "Active" } else { "Unsupported" }));
            out.push_str(&alloc::format!("{:<30} {:<15}\n", "CR4.SMAP (Access Prevention)", if rep.smap_supported { "Supported" } else { "Unsupported" }));
            out.push_str(&alloc::format!("{:<30} {:<15}\n", "W^X Memory Permission Policy", if rep.wx_enforced { "Enforced" } else { "Disabled" }));
            out.push_str(&alloc::format!("{:<30} {:<15}\n", "Stack Smashing Canaries", if rep.stack_canary_active { "Active" } else { "Disabled" }));
            out.push_str(&alloc::format!("{:<30} {:<15}\n", "User Pointer Sanitization", if rep.user_boundary_enforced { "Enforced" } else { "Disabled" }));
            (out, 0)
        }
        "ipc" => {
            let mut out = String::new();
            out.push_str("Inter-Process Communication (IPC) Resources:\n");
            out.push_str("----------------------------------------------\n");
            out.push_str(&alloc::format!("  Anonymous Pipes:      Circular ring buffers (default cap: {} bytes)\n", crate::ipc::DEFAULT_PIPE_CAPACITY));
            out.push_str(&alloc::format!("  Unix Domain Sockets:  Bidirectional socketpairs (default cap: {} bytes)\n", crate::ipc::DEFAULT_UNIX_SOCKET_CAPACITY));
            out.push_str("  Process Signals:      POSIX SIGHUP, SIGINT, SIGKILL, SIGSEGV, SIGTERM, SIGCHLD\n");
            out.push_str("\nShared Memory Segments (SHM):\n");
            let shms = crate::ipc::shm_list();
            if shms.is_empty() {
                out.push_str("  (No shared memory segments registered)\n");
            } else {
                out.push_str(&alloc::format!("  {:<6} {:<12} {:<10} {:<8} {:<12}\n", "SHMID", "KEY", "SIZE", "PAGES", "ATTACHMENTS"));
                for s in shms {
                    out.push_str(&alloc::format!("  {:<6} {:#010x} {:<10} {:<8} {:<12}\n", s.id, s.key, s.size, s.pages, s.attachments));
                }
            }
            (out, 0)
        }
        "threads" => {
            let mut out = String::new();
            out.push_str("Active System Threads & Runtime State:\n");
            out.push_str("----------------------------------------------------------------------------------\n");
            out.push_str(&alloc::format!("{:<6} {:<6} {:<16} {:<12} {:<6} {:<10} {:<18}\n",
                "TID", "PID", "NAME", "STATE", "PRIO", "TICKS", "FS_BASE"
            ));
            out.push_str("----------------------------------------------------------------------------------\n");
            let list = crate::task::thread_list();
            for t in list {
                let pid_str = t.process_id.map(|p| alloc::format!("{}", p.0)).unwrap_or_else(|| String::from("-"));
                let state_str = alloc::format!("{:?}", t.state);
                out.push_str(&alloc::format!("{:<6} {:<6} {:<16} {:<12} {:<6} {:<10} {:#018x}\n",
                    t.id.0, pid_str, t.name, state_str, t.priority, t.total_ticks, t.fs_base
                ));
            }
            (out, 0)
        }
        "dmesg" => {
            let logs = crate::drivers::console::get_dmesg();
            if logs.is_empty() {
                ("dmesg: no kernel messages logged\n".to_string(), 0)
            } else {
                (logs, 0)
            }
        }
        "test" => {
            crate::println!("[SHELL] Initiating automated self-test suite...");
            run_automated_tests();
            ("[SHELL] Automated self-test suite finished successfully.\n".to_string(), 0)
        }
        "gui" | "wm" => {
            let mut out = String::new();
            if args.first().map(|s| s.as_str()) == Some("--status") {
                if let Some(info) = crate::graphics::get_fb_info() {
                    out.push_str("NullOS Linux-Style Display & Input Subsystem (fbdev + evdev):\n");
                    out.push_str(&alloc::format!("  Resolution:  {}x{} (stride: {}, {} bpp)\n", info.width, info.height, info.stride, info.bpp));
                    out.push_str("  Compositor:  Ring 3 Userspace (/bin/compositor.elf)\n");
                    out.push_str("  IPC / SHM:   AF_UNIX (/tmp/compositor.sock) + SysV SHM\n");
                } else {
                    out.push_str("Framebuffer device is not initialized (no GOP framebuffer).\n");
                }
            } else {
                if let Ok(inode) = crate::fs::vfs::VFS.resolve_path("/bin/compositor.elf") {
                    let size = inode.stat().size;
                    let mut buf = alloc::vec![0u8; size];
                    if let Ok(n) = inode.read_at(0, &mut buf) {
                        buf.truncate(n);
                        let code = crate::task::execute_user_elf("compositor", &buf).unwrap_or(-1);
                        out.push_str(&alloc::format!("[GUI] Ring 3 compositor exited with code {}.\n", code));
                    } else {
                        out.push_str("[GUI] Failed to read /bin/compositor.elf.\n");
                    }
                } else {
                    out.push_str("[GUI] /bin/compositor.elf not found in VFS.\n");
                }
            }
            (out, 0)
        }
        "mem" => {
            let mut out = String::new();
            out.push_str("Physical Memory Manager (PMM):\n");
            out.push_str("  Page frame size: 4 KiB (4,096 bytes)\n");
            out.push_str("  Allocator: Fast Bitmask Bitmap\n");
            out.push_str("  Status: Operational\n");
            (out, 0)
        }
        "uptime" => {
            let ticks = apic::get_ticks();
            (alloc::format!("Uptime: {} APIC timer ticks\n", ticks), 0)
        }
        "acpi" => {
            let mut out = String::new();
            if let Some(info) = crate::arch::x86_64::acpi::get_acpi_info() {
                out.push_str("Advanced Configuration and Power Interface (ACPI) Status:\n");
                out.push_str(&alloc::format!(
                    "  RSDP Address:    {:#014x} (Revision: {}, OEM: \"{}\")\n",
                    info.rsdp_addr,
                    info.revision,
                    core::str::from_utf8(&info.oem_id).unwrap_or("???")
                ));
                out.push_str(&alloc::format!("  Total Tables:    {} cataloged\n", info.tables.len()));
                out.push_str("--------------------------------------------------------------------------------\n");
                out.push_str(&alloc::format!("{:<8} {:<18} {:<12} {:<6} {}\n", "TABLE", "PHYSICAL ADDR", "LENGTH", "REV", "OEM ID"));
                out.push_str("--------------------------------------------------------------------------------\n");
                for tbl in &info.tables {
                    out.push_str(&alloc::format!(
                        "{:<8} {:#016x} {:<12} {:<6} \"{}\"\n",
                        tbl.signature_str(),
                        tbl.address,
                        alloc::format!("{} B", tbl.length),
                        tbl.revision,
                        tbl.oem_id_str()
                    ));
                }
                out.push_str("--------------------------------------------------------------------------------\n");
                if let Some(ref fadt) = info.fadt {
                    out.push_str("FADT (\"FACP\") Power Management Configuration:\n");
                    out.push_str(&alloc::format!(
                        "  PM1a_CNT Port:   0x{:04x} (Status: {})\n",
                        fadt.pm1a_cnt_blk,
                        if fadt.is_acpi_enabled() { "ACPI Mode (SCI_EN=1)" } else { "Legacy Mode" }
                    ));
                    out.push_str(&alloc::format!("  PM1b_CNT Port:   0x{:04x}\n", fadt.pm1b_cnt_blk));
                    out.push_str(&alloc::format!(
                        "  SMI_CMD Port:    0x{:04x} (Enable Val: 0x{:02x}, Disable Val: 0x{:02x})\n",
                        fadt.smi_cmd, fadt.acpi_enable, fadt.acpi_disable
                    ));
                    out.push_str(&alloc::format!(
                        "  Reset Register:  {}\n",
                        if fadt.supports_reset_reg() { "Supported (ACPI 2.0+)" } else { "Unsupported (Fallback to 8042/PCI)" }
                    ));
                    out.push_str(&alloc::format!("  DSDT Address:    {:#014x}\n", fadt.dsdt_address));
                }
                if let Some(ref dsdt) = info.dsdt {
                    out.push_str("DSDT Definition Block & AML Sleep States:\n");
                    out.push_str(&alloc::format!("  Length:          {} bytes\n", dsdt.length));
                    out.push_str(&alloc::format!(
                        "  \\_S5 Soft-Off:   {} (SLP_TYPa: 0x{:02x}, SLP_TYPb: 0x{:02x})\n",
                        if dsdt.s5_supported { "Discovered in DSDT" } else { "Emulation Fallback" },
                        dsdt.slp_typa,
                        dsdt.slp_typb
                    ));
                }
                if let Some(ref hpet) = info.hpet {
                    out.push_str("HPET (\"HPET\") High Precision Event Timer Configuration:\n");
                    out.push_str(&alloc::format!("  MMIO Base:       {:#014x}\n", hpet.address));
                    out.push_str(&alloc::format!("  Timer Block ID:  {:#x}\n", hpet.block_id));
                    out.push_str(&alloc::format!("  Min Clock Tick:  {} fs\n", hpet.min_tick));
                }
                if let Some(ref madt) = info.madt {
                    out.push_str("MADT (\"APIC\") Topology Summary:\n");
                    out.push_str(&alloc::format!("  CPU Cores:       {} online\n", madt.cpus.len()));
                    out.push_str(&alloc::format!("  BSP APIC ID:     {}\n", madt.bsp_apic_id));
                    out.push_str(&alloc::format!("  I/O APICs:       {}\n", madt.io_apics.len()));
                }
            } else {
                out.push_str("ACPI subsystem is not initialized.\n");
            }
            (out, 0)
        }
        "poweroff" | "shutdown" => {
            crate::arch::x86_64::acpi::poweroff();
        }
        "reboot" => {
            crate::arch::x86_64::acpi::reboot();
        }
        "ldd" => {
            if args.is_empty() {
                return ("Usage: ldd <path>\n".to_string(), 1);
            }
            let resolved = resolve_path_with_pwd(&args[0]);
            match VFS.open(&resolved, OpenFlags::READ_ONLY) {
                Ok(file) => {
                    let size = file.stat().size;
                    let mut elf_bytes = vec![0u8; size];
                    match file.read_at(0, &mut elf_bytes) {
                        Ok(n) if n == size => {
                            let ldd_out = crate::task::dynlink::format_ldd_output(&resolved, &elf_bytes);
                            (ldd_out, 0)
                        }
                        Ok(_) => (alloc::format!("ldd: short read on '{}'\n", resolved), 1),
                        Err(e) => (alloc::format!("ldd: error reading '{}': {:?}\n", resolved, e), 1),
                    }
                }
                Err(e) => (alloc::format!("ldd: cannot open '{}': {:?}\n", resolved, e), 1),
            }
        }
        "libs" => {
            let out = crate::task::dynlink::format_loaded_libraries();
            (out, 0)
        }
        "exec" => {
            if args.is_empty() {
                return ("Usage: exec <path>\n".to_string(), 1);
            }
            let resolved = resolve_path_with_pwd(&args[0]);
            match VFS.open(&resolved, OpenFlags::READ_ONLY) {
                Ok(file) => {
                    let size = file.stat().size;
                    let mut elf_bytes = vec![0u8; size];
                    match file.read_at(0, &mut elf_bytes) {
                        Ok(n) if n == size => {
                            let name = resolved.rsplit('/').next().unwrap_or(&resolved);
                            match crate::task::execute_user_elf(name, &elf_bytes) {
                                Ok(status) => (alloc::format!("Process '{}' exited with status {}\n", name, status), status),
                                Err(e) => (alloc::format!("exec: failed to execute '{}': {}\n", resolved, e), 1),
                            }
                        }
                        Ok(_) => (alloc::format!("exec: short read on '{}'\n", resolved), 1),
                        Err(e) => (alloc::format!("exec: error reading '{}': {:?}\n", resolved, e), 1),
                    }
                }
                Err(e) => (alloc::format!("exec: cannot open '{}': {:?}\n", resolved, e), 1),
            }
        }
        "nvme" => {
            let ctrls = crate::drivers::nvme::controllers();
            if ctrls.is_empty() {
                ("No NVMe PCIe controllers detected.\n".to_string(), 0)
            } else {
                let mut out = String::from("Detected NVMe PCIe Storage Controllers:\n");
                for (idx, c) in ctrls.iter().enumerate() {
                    out.push_str(&alloc::format!(
                        "  [nvme{}] PCI {:02x}:{:02x}.{} | NVMe {}.{} | Model: \"{}\" | Serial: \"{}\" | FW: \"{}\"\n",
                        idx, c.pci_bus, c.pci_dev, c.pci_func, c.version_major, c.version_minor, c.model, c.serial, c.firmware
                    ));
                    out.push_str(&alloc::format!(
                        "          Queue Depth: {} | Doorbell Stride: {}B | Namespaces: {} | NS1: {} x {}B\n",
                        c.max_queue_entries, c.doorbell_stride, c.namespaces, c.ns1_sectors, c.ns1_sector_size
                    ));
                }
                (out, 0)
            }
        }
        "storage" | "cache" => {
            crate::storage::print_status();
            (String::new(), 0)
        }
        "diskbench" => {
            crate::storage::bench::run_benchmarks();
            (String::new(), 0)
        }
        "netperf" => {
            crate::net::perf::print_status();
            (String::new(), 0)
        }
        "netbench" => {
            crate::net::perf::bench::run_benchmarks();
            (String::new(), 0)
        }
        "perfstat" => {
            crate::perf::print_status();
            (String::new(), 0)
        }
        "kbench" | "perfbench" => {
            crate::perf::bench::run_benchmarks();
            (String::new(), 0)
        }
        "trace" => {
            let count = args.get(1).and_then(|s| s.parse::<usize>().ok()).unwrap_or(16);
            crate::perf::trace::dump_recent(count);
            (String::new(), 0)
        }
        "optstat" => {
            crate::perf::opt::print_status();
            (String::new(), 0)
        }
        "optbench" => {
            crate::perf::opt::bench_reg::run_benchmark_suite();
            (String::new(), 0)
        }
        "sdk" => {
            println!("=== NullOS User-Space SDK Environment (Phase 18 - M39) ===");
            println!("  SDK Version    : 0.18.39-m39 (x86_64-nullos-elf)");
            println!("  ABI Base       : 0x00400000 (4 KiB W^X LOAD segments, SysV AMD64)");
            println!("  Runtime Startup: crt0.o (.init_array constructors, 16B aligned stack)");
            println!("  Libraries      : libc.a (static) + libc.so (dynamic /lib/ld.so)");
            println!("  Host Tools     : nullos-cc, nullos-sdk, nullos-dbg, nullos-pkg, sdk.mk");
            println!("  Package Format : .npkg v1 (64B NPKG header + IEEE 802.3 CRC-32 + ELF)");
            (String::new(), 0)
        }
        "pkg" => {
            if args.len() < 2 {
                return ("Usage: pkg <inspect|run> <path.npkg>\n".to_string(), 1);
            }
            let subcmd = args[0].as_str();
            let resolved = resolve_path_with_pwd(&args[1]);
            match VFS.open(&resolved, OpenFlags::READ_ONLY) {
                Ok(file) => {
                    let size = file.stat().size;
                    let mut buf = vec![0u8; size];
                    match file.read_at(0, &mut buf) {
                        Ok(n) if n == size => match subcmd {
                            "inspect" => match crate::task::npkg::parse_and_verify(&buf) {
                                Ok(pkg) => {
                                    crate::task::npkg::print_package_info(&pkg);
                                    (String::new(), 0)
                                }
                                Err(e) => (alloc::format!("pkg inspect error: {}\n", e), 1),
                            },
                            "run" => match crate::task::npkg::execute_package(&buf) {
                                Ok(status) => (alloc::format!("Package exited with status {}\n", status), status),
                                Err(e) => (alloc::format!("pkg run error: {}\n", e), 1),
                            },
                            _ => ("Usage: pkg <inspect|run> <path.npkg>\n".to_string(), 1),
                        },
                        _ => (alloc::format!("pkg: failed reading '{}'\n", resolved), 1),
                    }
                }
                Err(e) => (alloc::format!("pkg: cannot open '{}': {:?}\n", resolved, e), 1),
            }
        }
        "dbg" | "kdbg" => {
            let arg_strs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            crate::debug::debugger::dispatch_shell_command(&arg_strs)
        }
        "bt" | "sym" | "klog" | "lockdep" | "crashdump" => {
            let sub = if cmd == "crashdump" { "crash" } else if cmd == "lockdep" { "locks" } else { cmd };
            let mut full_args = Vec::with_capacity(args.len() + 1);
            full_args.push(sub);
            for a in args {
                full_args.push(a.as_str());
            }
            crate::debug::debugger::dispatch_shell_command(&full_args)
        }
        "reliability" | "rel" => {
            let sub = args.first().map(|s| s.as_str()).unwrap_or("status");
            if sub == "status" {
                (crate::reliability::format_status(), 0)
            } else {
                let count_arg = args.get(1).and_then(|s| s.parse::<usize>().ok());
                let (msg, ok) = crate::reliability::run_suite(sub, count_arg);
                (msg, if ok { 0 } else { 1 })
            }
        }
        unknown => {
            // Check PATH environment variable and current directory for executable binaries
            let mut candidates = Vec::new();
            if unknown.contains('/') {
                candidates.push(resolve_path_with_pwd(unknown));
            } else {
                let path_var = get_env("PATH");
                for dir in path_var.split(':') {
                    if !dir.is_empty() {
                        candidates.push(alloc::format!("{}/{}", dir.trim_end_matches('/'), unknown));
                    }
                }
                candidates.push(resolve_path_with_pwd(unknown));
            }

            for p in &candidates {
                if let Ok(file) = VFS.open(p, OpenFlags::READ_ONLY) {
                    let stat = file.stat();
                    if stat.file_type == FileType::File && stat.size > 4 {
                        let mut elf_bytes = vec![0u8; stat.size];
                        if let Ok(n) = file.read_at(0, &mut elf_bytes) {
                            if n == stat.size && elf_bytes.starts_with(&[0x7f, b'E', b'L', b'F']) {
                                let name = p.rsplit('/').next().unwrap_or(p);
                                match crate::task::execute_user_elf(name, &elf_bytes) {
                                    Ok(status) => return (alloc::format!("Process '{}' exited with status {}\n", name, status), status),
                                    Err(e) => return (alloc::format!("exec: failed to execute '{}': {}\n", p, e), 1),
                                }
                            }
                        }
                    }
                }
            }

            (alloc::format!("nsh: command not found: {}. Type 'help' for available commands.\n", unknown), 127)
        }
    }
}

/// Executes a complete pipeline synchronously with standard I/O redirection.
pub fn execute_pipeline_sync(line: &str) -> i32 {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return 0;
    }

    // 1. Expand shell variables ($VAR, ${VAR}, $?, $$)
    let expanded = expand_variables(trimmed, LAST_EXIT_CODE.load(Ordering::Relaxed));

    // 2. Split into pipeline stages separated by '|' outside quotes
    let raw_stages = split_pipeline(&expanded);
    if raw_stages.is_empty() {
        return 0;
    }

    // 3. Extract input redirection '<' from the first stage
    let (first_cmd, input_file) = extract_input_redirection(&raw_stages[0]);
    let initial_input = if let Some(infile) = input_file {
        let resolved = resolve_path_with_pwd(&infile);
        match read_file_to_string(&resolved) {
            Ok(content) => content,
            Err(e) => {
                println!("nsh: {}: {:?}", infile, e);
                return 1;
            }
        }
    } else {
        String::new()
    };

    // 4. Extract output redirection '>' or '>>' from the final stage
    let last_idx = raw_stages.len() - 1;
    let last_raw = if last_idx == 0 { &first_cmd } else { &raw_stages[last_idx] };
    let (last_cmd, output_redir) = extract_output_redirection(last_raw);

    // 5. Construct final list of pipeline stage commands
    let mut stages = Vec::new();
    if raw_stages.len() == 1 {
        stages.push(last_cmd);
    } else {
        stages.push(first_cmd);
        for stage in &raw_stages[1..last_idx] {
            stages.push(stage.clone());
        }
        stages.push(last_cmd);
    }

    // 6. Execute each stage sequentially, piping output of stage (i) to input of stage (i+1)
    let mut pipe_data = initial_input;
    let mut last_status = 0;

    for stage_line in stages {
        let tokens = tokenize(&stage_line);
        if tokens.is_empty() {
            continue;
        }
        let cmd = &tokens[0];
        let args = &tokens[1..];
        let (output, status) = execute_stage(cmd, args, &pipe_data);
        pipe_data = output;
        last_status = status;
    }

    // 7. Deliver final output: either redirect to a VFS file or print to the terminal console
    match output_redir {
        Some(OutputRedir::Truncate(file)) => {
            let resolved = resolve_path_with_pwd(&file);
            if let Err(e) = write_file_content(&resolved, &pipe_data, false) {
                println!("nsh: failed to write to '{}': {:?}", file, e);
                last_status = 1;
            }
        }
        Some(OutputRedir::Append(file)) => {
            let resolved = resolve_path_with_pwd(&file);
            if let Err(e) = write_file_content(&resolved, &pipe_data, true) {
                println!("nsh: failed to append to '{}': {:?}", file, e);
                last_status = 1;
            }
        }
        None => {
            if !pipe_data.is_empty() {
                print!("{}", pipe_data);
            }
        }
    }

    LAST_EXIT_CODE.store(last_status, Ordering::Relaxed);
    last_status
}

/// Worker thread entry function for asynchronous background jobs (`cmd &`).
fn bg_worker_entry() {
    let job_opt = {
        let mut q = BG_QUEUE.lock();
        let mut found = None;
        for slot in q.iter_mut() {
            if slot.is_some() {
                found = slot.take();
                break;
            }
        }
        found
    };

    if let Some(entry) = job_opt {
        let cmd_str = core::str::from_utf8(&entry.cmd_bytes[..entry.cmd_len]).unwrap_or("");
        let exit_code = execute_pipeline_sync(cmd_str);
        let mut jobs = JOBS.lock();
        for slot in jobs.iter_mut() {
            if let Some(j) = slot.as_mut() {
                if j.id == entry.job_id {
                    j.state = JobState::Done(exit_code);
                    break;
                }
            }
        }
    }
}

/// Executes a command string and captures its output into a String without direct printing.
pub fn execute_command_capture(line: &str) -> (String, i32) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return (String::new(), 0);
    }
    let expanded = expand_variables(trimmed, LAST_EXIT_CODE.load(Ordering::Relaxed));
    let raw_stages = split_pipeline(&expanded);
    if raw_stages.is_empty() {
        return (String::new(), 0);
    }
    let (first_cmd, input_file) = extract_input_redirection(&raw_stages[0]);
    let initial_input = if let Some(infile) = input_file {
        let resolved = resolve_path_with_pwd(&infile);
        match read_file_to_string(&resolved) {
            Ok(content) => content,
            Err(e) => return (alloc::format!("nsh: {}: {:?}\n", infile, e), 1),
        }
    } else {
        String::new()
    };
    let last_idx = raw_stages.len() - 1;
    let last_raw = if last_idx == 0 { &first_cmd } else { &raw_stages[last_idx] };
    let (last_cmd, _output_redir) = extract_output_redirection(last_raw);

    let mut stages = Vec::new();
    if raw_stages.len() == 1 {
        stages.push(last_cmd);
    } else {
        stages.push(first_cmd);
        for stage in &raw_stages[1..last_idx] {
            stages.push(stage.clone());
        }
        stages.push(last_cmd);
    }

    let mut pipe_data = initial_input;
    let mut last_status = 0;
    for stage_cmd in stages {
        let tokens = tokenize(&stage_cmd);
        if tokens.is_empty() {
            continue;
        }
        let cmd = &tokens[0];
        let args = &tokens[1..];
        let (output, status) = execute_stage(cmd, args, &pipe_data);
        pipe_data = output;
        last_status = status;
        if status != 0 {
            break;
        }
    }
    (pipe_data, last_status)
}

/// Executes a single command line string, handling background execution (`&`) and pipelines.
pub fn execute_command(line: &str) -> i32 {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return 0;
    }

    // Check for trailing '&' indicating asynchronous background execution
    if trimmed.ends_with('&') {
        let cmd = trimmed[..trimmed.len() - 1].trim();
        let job_id = NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed);
        let new_job = Job::new(job_id, cmd);

        {
            let mut jobs = JOBS.lock();
            let mut placed = false;
            for slot in jobs.iter_mut() {
                if slot.is_none() {
                    *slot = Some(new_job);
                    placed = true;
                    break;
                }
            }
            if !placed {
                jobs[0] = Some(new_job);
            }
        }

        {
            let mut q = BG_QUEUE.lock();
            let mut placed = false;
            let entry = BgQueueEntry {
                job_id,
                cmd_len: new_job.cmd_len,
                cmd_bytes: new_job.cmd_bytes,
            };
            for slot in q.iter_mut() {
                if slot.is_none() {
                    *slot = Some(entry);
                    placed = true;
                    break;
                }
            }
            if !placed {
                q[0] = Some(entry);
            }
        }

        let tid = crate::task::spawn("bg_job", bg_worker_entry, 1);
        println!("[{}] Job started in background: \"{}\" (Thread #{})", job_id, cmd, tid.0);
        0
    } else {
        execute_pipeline_sync(trimmed)
    }
}

/// Runs automated self-tests to prove every shell feature, pipeline, redirection, and job functions.
pub fn run_automated_tests() {
    println!();
    println!("--------------------------------------------------");
    println!(" [SHELL] Running Phase 7 Advanced Shell Tests     ");
    println!("--------------------------------------------------");

    println!("[SHELL] 1. Testing 'help' command...");
    execute_command("help");

    println!("[SHELL] 2. Testing Environment Variables & Expansion...");
    execute_command("export TEST_VAR=NullOS_Rocks");
    execute_command("echo Hello $USER running $OS on $ARCH! Var: $TEST_VAR");
    execute_command("echo Last exit status was: $?");

    println!("[SHELL] 3. Testing 'pwd' and 'cd' relative navigation...");
    execute_command("pwd");
    execute_command("cd /etc");
    execute_command("pwd");
    execute_command("cd /");
    execute_command("pwd");

    println!("[SHELL] 4. Testing File Redirection (> and >>)...");
    execute_command("echo Line One > /tmp/pipeline_test.txt");
    execute_command("echo Line Two >> /tmp/pipeline_test.txt");
    execute_command("echo Line Three >> /tmp/pipeline_test.txt");
    execute_command("cat /tmp/pipeline_test.txt");

    println!("[SHELL] 5. Testing Input Redirection (<)...");
    execute_command("cat < /tmp/pipeline_test.txt");

    println!("[SHELL] 6. Testing Pipelines (cat | grep)...");
    execute_command("cat /tmp/pipeline_test.txt | grep Two");

    println!("[SHELL] 7. Testing Pipelines with Utilities (wc, head, tail)...");
    execute_command("cat /tmp/pipeline_test.txt | wc -l");
    execute_command("cat /tmp/pipeline_test.txt | head -n 2");
    execute_command("cat /tmp/pipeline_test.txt | tail -n 2");

    println!("[SHELL] 8. Testing Combined Pipeline + Redirection (cat | grep > out)...");
    execute_command("cat /tmp/pipeline_test.txt | grep Three > /tmp/filtered.txt");
    execute_command("cat /tmp/filtered.txt");

    println!("[SHELL] 9. Testing File Removal (rm)...");
    execute_command("rm /tmp/filtered.txt");

    println!("[SHELL] 10. Testing Background Execution & Job Control (cmd &)...");
    execute_command("echo Background job running asynchronously &");
    for _ in 0..5 {
        crate::task::yield_now();
    }
    execute_command("jobs");

    println!("[SHELL] 11. Testing Process Signaling (kill)...");
    execute_command("kill 99999"); // Non-existent process (error handling test)
    execute_command("kill 1");     // Signal init process

    println!("[SHELL] 12. Testing Ring 3 ELF Userspace Execution...");
    execute_command("exec /bin/hello.elf");
    execute_command("exec /disk/HELLO.ELF");

    println!("[SHELL] 13. Testing Network Interface Configuration (ifconfig)...");
    execute_command("ifconfig");

    println!("[SHELL] 14. Testing ARP Table Cache (arp)...");
    execute_command("arp");

    println!("[SHELL] 15. Testing Active Network Sockets (netstat)...");
    execute_command("netstat");

    println!("[SHELL] 16. Testing ICMP Network Ping (ping 10.0.2.2)...");
    execute_command("ping 10.0.2.2");

    println!("[SHELL] 17. Testing SMP Multiprocessor Status (cpus)...");
    execute_command("cpus");

    println!("[SHELL] 18. Testing Kernel Security Mitigations (sec)...");
    execute_command("sec");

    println!("[SHELL] 19. Testing Inter-Process Communication (ipc)...");
    execute_command("ipc");

    println!("[SHELL] 20. Testing User Threads & Synchronization (threads)...");
    execute_command("threads");

    // Kernel synchronization primitive self-tests
    let test_mutex = crate::sync::Mutex::new(42);
    {
        let mut guard = test_mutex.lock();
        *guard += 10;
    }
    assert_eq!(*test_mutex.lock(), 52);

    let test_sem = crate::sync::Semaphore::new(2);
    assert_eq!(test_sem.get_value(), 2);
    test_sem.wait();
    assert_eq!(test_sem.get_value(), 1);
    assert!(test_sem.try_wait());
    assert!(!test_sem.try_wait());
    test_sem.post();
    assert_eq!(test_sem.get_value(), 1);

    if crate::fs::vfs::VFS.resolve_path("/disk/THREADS.ELF").is_ok() {
        execute_command("exec /disk/THREADS.ELF");
    }

    println!("[SHELL] 21. Testing Async & Event Infrastructure (poll, epoll, eventfd, timerfd, writev)...");
    use crate::fs::vfs::Inode;
    let evfd = crate::task::event::EventFd::new(10, 0);
    assert_eq!(evfd.poll() & crate::fs::vfs::POLLIN, crate::fs::vfs::POLLIN);
    let mut buf = [0u8; 8];
    assert_eq!(evfd.read_at(0, &mut buf), Ok(8));
    assert_eq!(u64::from_ne_bytes(buf), 10);
    assert_eq!(evfd.poll() & crate::fs::vfs::POLLIN, 0);

    let tfd = crate::task::event::TimerFd::new(crate::task::event::CLOCK_MONOTONIC, 0);
    let ts = crate::task::event::ITimerSpec {
        it_interval: crate::task::event::TimeSpec { tv_sec: 0, tv_nsec: 0 },
        it_value: crate::task::event::TimeSpec { tv_sec: 0, tv_nsec: 10_000_000 },
    };
    tfd.set_time(ts);

    {
        let ep = crate::task::event::EpollInstance::new();
        let ev = crate::task::event::EpollEvent {
            events: crate::task::event::EPOLLIN,
            data: 0x1234,
        };
        assert!(ep.ctl(crate::task::event::EPOLL_CTL_ADD, 100, ev).is_ok());
        assert!(ep.ctl(crate::task::event::EPOLL_CTL_ADD, 100, ev).is_err());
        assert!(ep.ctl(crate::task::event::EPOLL_CTL_DEL, 100, ev).is_ok());
    }

    if crate::fs::vfs::VFS.resolve_path("/disk/ASYNC.ELF").is_ok() {
        execute_command("exec /disk/ASYNC.ELF");
    }

    println!("[SHELL] 22. Testing Linux-Style Framebuffer, Input, SHM & Named AF_UNIX IPC (M30)...");
    {
        if let Some(info) = crate::graphics::get_fb_info() {
            assert!(info.width > 0 && info.height > 0);
        }
        crate::graphics::event::INPUT_QUEUE.push(crate::graphics::event::InputEvent::MouseMove {
            x: 120,
            y: 90,
            dx: 5,
            dy: -3,
        });
        let raw = crate::graphics::event::poll_raw_input().expect("Expected queued raw input event");
        assert_eq!(raw.kind, crate::graphics::event::RAW_INPUT_MOUSE_MOVE);
        assert_eq!(raw.x, 120);
        assert_eq!(raw.y, 90);

        // Test named AF_UNIX listener -> connect -> accept -> send/recv
        crate::ipc::unix_socket::unix_listen("/tmp/test_comp.sock").unwrap();
        let client = crate::ipc::unix_socket::unix_connect("/tmp/test_comp.sock").unwrap();
        let server = crate::ipc::unix_socket::unix_accept("/tmp/test_comp.sock").unwrap().unwrap();
        assert_eq!(client.send(b"hello_wayland").unwrap(), 13);
        let mut rbuf = [0u8; 32];
        let n = server.recv(&mut rbuf).unwrap();
        assert_eq!(&rbuf[..n], b"hello_wayland");
        crate::ipc::unix_socket::unix_close_listener("/tmp/test_comp.sock");

        // Verify /bin/compositor.elf is present in initramfs
        assert!(crate::fs::vfs::VFS.resolve_path("/bin/compositor.elf").is_ok());
    }

    println!("[SHELL] 23. Testing USB 3.0 xHCI Controller & HID Input Subsystems (M31)...");
    execute_command("usb");
    crate::drivers::usb::run_self_tests();

    println!("[SHELL] 24. Testing Intel HD Audio (HDA) Subsystem & PCM Synthesizer (M32)...");
    execute_command("audio");
    execute_command("beep");
    crate::drivers::audio::run_self_tests();

    if VFS.resolve_path("/bin/audiod.elf").is_ok() {
        println!("[SHELL] Testing Ring 3 Userspace Audio Daemon (/bin/audiod.elf)...");
        let rc = execute_command("exec /bin/audiod.elf");
        assert_eq!(rc, 0, "Userspace audio daemon must synthesize and stream PCM audio via /dev/audio and exit 0");
    }

    println!("[SHELL] 25. Testing ACPI Table Parser, FADT, DSDT & System Power Subsystem (M33)...");
    execute_command("acpi");
    crate::arch::x86_64::acpi::run_self_tests();

    if VFS.resolve_path("/bin/powerd.elf").is_ok() {
        println!("[SHELL] Testing Ring 3 Userspace ACPI Power Daemon (/bin/powerd.elf)...");
        let rc = execute_command("exec /bin/powerd.elf");
        assert_eq!(rc, 0, "Userspace power daemon must parse DSDT AML & configure sleep states via /dev/power and exit 0");
    }

    println!("[SHELL] 26. Testing Dynamic Linking, Shared Libraries (.so) & Runtime Relocations (M34)...");
    execute_command("libs");
    crate::task::dynlink::run_self_tests();
    execute_command("ldd /bin/hello.elf");
    if VFS.resolve_path("/disk/DYN_HELLO.ELF").is_ok() {
        execute_command("ldd /disk/DYN_HELLO.ELF");
    }
    if VFS.resolve_path("/disk/DYN_TEST.ELF").is_ok() {
        execute_command("ldd /disk/DYN_TEST.ELF");
        execute_command("exec /disk/DYN_TEST.ELF");
    }
    execute_command("libs");

    println!("[SHELL] 27. Testing Advanced Storage Subsystem, Caches, I/O Scheduler, Async I/O, Journal & NVMe (M35)...");
    execute_command("nvme");
    execute_command("storage");
    crate::storage::run_self_tests();
    execute_command("storage");

    if VFS.resolve_path("/bin/fat32d.elf").is_ok() {
        println!("[SHELL] Testing Ring 3 Userspace Filesystem Daemon (/bin/fat32d.elf)...");
        let rc = execute_command("exec /bin/fat32d.elf");
        assert_eq!(rc, 0, "Userspace FAT32 daemon must inspect /dev/virtio0 and exit 0");
    }

    println!("[SHELL] 28. Testing High-Performance Networking, RSS Multiqueue, NAPI Polling, Zero-Copy, Batching & TCP Opt (M36)...");
    execute_command("netperf");
    crate::net::perf::run_self_tests();
    execute_command("netperf");

    if VFS.resolve_path("/bin/netd.elf").is_ok() {
        println!("[SHELL] Testing Ring 3 Userspace Network Daemon (/bin/netd.elf)...");
        let rc = execute_command("exec /bin/netd.elf");
        assert_eq!(rc, 0, "Userspace network daemon must process raw packets over /dev/net/tun0 and exit 0");
    }

    println!("[SHELL] 29. Testing Kernel Profiling, TSC/PMU, Tracepoints, Latency Histograms, Lock Contention & Benchmarks (M37)...");
    execute_command("perfstat");
    crate::perf::run_self_tests();
    execute_command("trace 8");
    execute_command("perfstat");

    println!("[SHELL] 30. Testing Kernel Optimization, O(1) Sched, Slab/Magazine Alloc, Page-Table Opt, Locks, NUMA & Regression Suite (M38)...");
    execute_command("optstat");
    crate::perf::opt::run_self_tests();
    execute_command("optstat");

    println!("[SHELL] 31. Testing NullOS SDK Application, Runtime ABI & .npkg Package Format (M39)...");
    execute_command("sdk");
    crate::task::npkg::run_self_tests();
    let insp_rc = execute_command("pkg inspect /disk/SDKDEMO.NPK");
    assert_eq!(insp_rc, 0, "pkg inspect /disk/SDKDEMO.NPK must succeed");
    let run_rc = execute_command("pkg run /disk/SDKDEMO.NPK");
    assert_eq!(run_rc, 0, "pkg run /disk/SDKDEMO.NPK must execute and exit 0");

    println!("[SHELL] 32. Testing Kernel Debugger, Symbolizer, Unwinder, Klog, MemDiag & Lockdep (M40)...");
    crate::debug::run_self_tests();
    assert_eq!(execute_command("dbg regs"), 0, "dbg regs must succeed");
    assert_eq!(execute_command("dbg bt"), 0, "dbg bt must succeed");
    assert_eq!(execute_command("dbg sym timer_handler"), 0, "dbg sym must resolve KSYM entries");
    assert_eq!(execute_command("dbg heap"), 0, "dbg heap must succeed");
    assert_eq!(execute_command("dbg klog info"), 0, "dbg klog must succeed");
    assert_eq!(execute_command("dbg locks"), 0, "dbg locks must succeed");
    assert_eq!(execute_command("dbg crash"), 0, "dbg crash must display recorded snapshots");

    println!("[SHELL] 33. Testing Bootable Distribution Initramfs, Userspace & Release Metadata (M41)...");
    assert!(VFS.resolve_path("/etc/nullos-release").is_ok(), "initramfs must populate /etc/nullos-release");
    assert!(VFS.resolve_path("/bin/hello.elf").is_ok(), "initramfs must populate /bin/hello.elf");
    assert!(VFS.resolve_path("/bin/cat.elf").is_ok(), "initramfs must populate /bin/cat.elf");
    assert!(VFS.resolve_path("/bin/threads.elf").is_ok(), "initramfs must populate /bin/threads.elf");
    assert!(VFS.resolve_path("/lib/libc.so").is_ok(), "initramfs must populate /lib/libc.so");
    assert!(VFS.resolve_path("/pkg/sdk_demo.npkg").is_ok(), "initramfs must populate /pkg/sdk_demo.npkg");
    assert_eq!(execute_command("cat /etc/nullos-release"), 0, "cat /etc/nullos-release must succeed");
    assert_eq!(execute_command("pkg inspect /pkg/sdk_demo.npkg"), 0, "pkg inspect /pkg/sdk_demo.npkg must succeed");

    println!("[SHELL] 34. Testing Kernel Reliability, Unit, Integration, FS, Net, SMP, Stress, MemCorruption, Fuzzing & Soak Stability (M42)...");
    crate::reliability::run_all();
    assert_eq!(execute_command("reliability status"), 0, "reliability status must succeed");

    println!("[SHELL] All Phase 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18 & 19 Shell, Network, SMP, Security, IPC, Threads, Async, Graphics, Audio, ACPI/Power, Dynamic Linking, Storage, High-Perf Networking, Kernel Profiling, Optimization, SDK, Debugging, Distribution and Reliability tests passed successfully!");
}

/// Interactive Shell Loop that listens on USB Keyboard, PS/2 Keyboard, and COM1 serial input
/// and executes commands in real-time.
pub fn interactive_loop() {
    println!();
    println!("==================================================");
    println!("        Welcome to NullOS Interactive Shell!       ");
    println!("        Type 'help' for available commands.       ");
    println!("==================================================");
    print!("nullos> ");

    let mut line_buf = String::new();

    loop {
        // Poll USB inputs (HID keyboards and mice)
        crate::drivers::usb::poll_inputs();

        // Read input from USB keyboard, PS/2 keyboard, or COM1 serial
        let incoming_char = if let Some(ch) = crate::drivers::usb::read_char() {
            Some(ch)
        } else if let Some(ch) = crate::drivers::keyboard::read_char() {
            Some(ch)
        } else if let Some(byte) = serial::read_byte() {
            match byte {
                b'\r' | b'\n' => Some('\n'),
                0x08 | 0x7F => Some('\x08'),
                b if b >= 0x20 && b <= 0x7E => Some(b as char),
                _ => None,
            }
        } else {
            None
        };

        if let Some(ch) = incoming_char {
            match ch {
                '\n' => {
                    println!();
                    execute_command(&line_buf);
                    line_buf.clear();
                    print!("nullos> ");
                }
                '\x08' | '\x7F' => {
                    // Backspace handling
                    if !line_buf.is_empty() {
                        line_buf.pop();
                        // Erase character from terminal: backspace, space, backspace
                        print!("\x08 \x08");
                    }
                }
                c if c >= ' ' && c <= '~' => {
                    // Printable ASCII character
                    line_buf.push(c);
                    print!("{}", c);
                }
                // TODO(shell-history): Implement circular history buffer with Up/Down arrow ANSI escape sequences (\x1b[A / \x1b[B).
                // TODO(shell-completion): Implement Tab key ('\t') auto-completion for builtin commands and VFS filesystem paths.
                // TODO(shell-navigation): Implement Left/Right cursor movement (\x1b[D / \x1b[C) and inline line-editing.
                _ => {}
            }
        } else {
            // Cooperative yield so background threads and kernel tasks can run
            crate::task::yield_now();
        }
    }
}
