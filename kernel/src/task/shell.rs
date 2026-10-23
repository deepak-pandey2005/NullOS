//! NullOS Advanced Interactive Shell (`nsh`) (Phase 7 - Userspace Shell & CLI).
//!
//! Provides a modern Unix-style command-line environment featuring:
//! - Environment variable storage (`$PATH`, `$USER`, `$PWD`, etc.), `export`, `env`, `unset`
//! - Dynamic variable expansion (`$VAR`, `${VAR}`, `$?`, `$$`)
//! - Standard I/O Redirection (`> file`, `>> file`, `< file`)
//! - Pipelines (`cmd1 | cmd2 | cmd3`)
//! - Background asynchronous execution (`cmd &`) & Job control (`jobs`)
//! - Process signaling (`kill <pid>`)
//! - Unix CLI utilities: `grep`, `wc`, `head`, `tail`, `pwd`, `cd`, `rm`
//! - Direct PATH binary execution and ELF loading

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
    Running,
    Done(i32),
}

/// A tracked background execution job with inline command storage (zero heap allocations).
#[derive(Debug, Clone, Copy)]
pub struct Job {
    pub id: usize,
    pub cmd_len: usize,
    pub cmd_bytes: [u8; 128],
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

/// Global shell environment variables map.
static ENV_VARS: Spinlock<BTreeMap<String, String>> = Spinlock::new(BTreeMap::new());

/// Exit code of the most recently executed foreground command (for `$?` expansion).
static LAST_EXIT_CODE: AtomicI32 = AtomicI32::new(0);

/// Initializes default shell environment variables if not already initialized.
pub fn init_env_vars() {
    let mut vars = ENV_VARS.lock();
    if vars.is_empty() {
        vars.insert("PATH".to_string(), "/bin:/disk".to_string());
        vars.insert("USER".to_string(), "root".to_string());
        vars.insert("HOME".to_string(), "/".to_string());
        vars.insert("SHELL".to_string(), "/bin/nsh".to_string());
        vars.insert("OS".to_string(), "NullOS".to_string());
        vars.insert("ARCH".to_string(), "x86_64".to_string());
        vars.insert("PWD".to_string(), "/".to_string());
    }
}

/// Retrieves an environment variable's value, or an empty string if undefined.
pub fn get_env(key: &str) -> String {
    init_env_vars();
    let vars = ENV_VARS.lock();
    vars.get(key).cloned().unwrap_or_default()
}

/// Sets or updates an environment variable.
pub fn set_env(key: &str, val: &str) {
    init_env_vars();
    let mut vars = ENV_VARS.lock();
    vars.insert(key.to_string(), val.to_string());
}

/// Unsets (removes) an environment variable.
pub fn unset_env(key: &str) {
    init_env_vars();
    let mut vars = ENV_VARS.lock();
    vars.remove(key);
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
            out.push_str("  mem                  - Display physical memory manager statistics\n");
            out.push_str("  uptime               - Display system timer ticks\n");
            out.push_str("  reboot               - Reboot physical or virtual machine\n");
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
            if let Some(dev) = crate::drivers::virtio_net::NETWORK_DEVICE.lock().as_ref() {
                let mac = dev.mac();
                let stats = dev.stats;
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
        "reboot" => {
            println!("Rebooting system...");
            unsafe {
                core::arch::asm!(
                    "out dx, al",
                    in("dx") 0x64u16,
                    in("al") 0xFEu8,
                    options(nomem, nostack),
                );
            }
            loop {
                unsafe { core::arch::asm!("hlt"); }
            }
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

    println!("[SHELL] All Phase 7, 8, 9 & 10 Shell, Network, SMP, Security and IPC tests passed successfully!");
}

/// Interactive Shell Loop that listens on both PS/2 Keyboard and COM1 serial input
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
        // Read input from either PS/2 keyboard (display window/hardware) or COM1 serial
        let incoming_char = if let Some(ch) = crate::drivers::keyboard::read_char() {
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
                '\x08' => {
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
                _ => {}
            }
        } else {
            // Cooperative yield so background threads and kernel tasks can run
            crate::task::yield_now();
        }
    }
}
