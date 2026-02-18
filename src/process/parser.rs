// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

use std::collections::HashMap;
use std::fs;
use std::sync::OnceLock;

use super::types::{Process, ProcessState};

/// Cache for UID to username lookups
static USER_CACHE: OnceLock<HashMap<u32, String>> = OnceLock::new();

/// Get username from UID, with caching
fn get_username(uid: u32) -> String {
    let cache = USER_CACHE.get_or_init(|| {
        let mut map = HashMap::new();
        if let Ok(content) = fs::read_to_string("/etc/passwd") {
            for line in content.lines() {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 3 {
                    if let Ok(uid) = parts[2].parse::<u32>() {
                        map.insert(uid, parts[0].to_string());
                    }
                }
            }
        }
        map
    });

    cache.get(&uid).cloned().unwrap_or_else(|| uid.to_string())
}

/// Parse multiple processes given a list of PIDs
pub fn parse_processes(pids: &[u32]) -> Vec<Process> {
    pids.iter()
        .filter_map(|&pid| parse_process(pid))
        .collect()
}

/// Parse a single process using direct file reads
fn parse_process(pid: u32) -> Option<Process> {
    let proc_path = format!("/proc/{}", pid);

    // Read stat file (required)
    let stat_content = fs::read_to_string(format!("{}/stat", proc_path)).ok()?;
    let stat = parse_proc_stat(&stat_content)?;

    // Read cmdline for process name and full command line (optional)
    let cmdline_content = fs::read_to_string(format!("{}/cmdline", proc_path)).ok();
    let name = cmdline_content
        .as_ref()
        .and_then(|content| parse_cmdline_name(content))
        .unwrap_or_else(|| stat.comm.clone());
    let cmdline = cmdline_content.and_then(|content| parse_full_cmdline(&content));

    // Read status for UID (optional)
    let uid = fs::read_to_string(format!("{}/status", proc_path))
        .ok()
        .and_then(|content| parse_uid_from_status(&content))
        .unwrap_or(0);
    let user = get_username(uid);

    // Read I/O stats (optional, may fail due to permissions)
    let (io_read_bytes, io_write_bytes) = fs::read_to_string(format!("{}/io", proc_path))
        .ok()
        .map(|content| parse_io_stats(&content))
        .unwrap_or((0, 0));

    let page_size = 4096u64;

    Some(Process {
        pid,
        ppid: stat.ppid,
        starttime: stat.starttime,
        name,
        cmdline,
        user,
        state: ProcessState::from(stat.state),
        utime: stat.utime,
        stime: stat.stime,
        rss: stat.rss * page_size,
        vsize: stat.vsize,
        num_threads: stat.num_threads,
        // Use NAN for rates to indicate "no baseline yet" (first sample)
        // This allows UI to distinguish "pending" from "actually zero"
        cpu_percent: f32::NAN,
        io_read_bytes,
        io_write_bytes,
        io_read_rate: f32::NAN,
        io_write_rate: f32::NAN,
    })
}

struct ProcStat {
    comm: String,
    state: char,
    ppid: u32,
    utime: u64,
    stime: u64,
    vsize: u64,
    rss: u64,
    num_threads: u32,
    starttime: u64,
}

fn parse_proc_stat(content: &str) -> Option<ProcStat> {
    // Find comm between parentheses (handles names with spaces/parens)
    let start = content.find('(')?;
    let end = content.rfind(')')?;
    let comm = content[start + 1..end].to_string();

    // Parse fields after comm
    let rest = &content[end + 2..]; // Skip ") "
    let fields: Vec<&str> = rest.split_whitespace().collect();

    if fields.len() < 22 {
        return None;
    }

    Some(ProcStat {
        comm,
        state: fields[0].chars().next()?,
        ppid: fields[1].parse().ok()?,
        utime: fields[11].parse().ok()?,
        stime: fields[12].parse().ok()?,
        vsize: fields[20].parse().ok()?,
        rss: fields[21].parse().ok()?,
        num_threads: fields[17].parse().ok()?,
        starttime: fields[19].parse().ok()?,  // Field 22 in stat (0-indexed 19 after comm)
    })
}

fn parse_cmdline_name(cmdline: &str) -> Option<String> {
    let first_arg = cmdline.split('\0').next()?;
    if first_arg.is_empty() {
        return None;
    }
    let name = std::path::Path::new(first_arg)
        .file_name()?
        .to_str()?;
    Some(name.to_string())
}

fn parse_full_cmdline(cmdline: &str) -> Option<String> {
    let args: Vec<&str> = cmdline.split('\0').filter(|s| !s.is_empty()).collect();
    if args.is_empty() {
        return None;
    }
    Some(args.join(" "))
}

fn parse_uid_from_status(content: &str) -> Option<u32> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let uid_str = rest.split_whitespace().next()?;
            return uid_str.parse().ok();
        }
    }
    None
}

fn parse_io_stats(content: &str) -> (u64, u64) {
    let mut read_bytes = 0u64;
    let mut write_bytes = 0u64;

    for line in content.lines() {
        if let Some(value) = line.strip_prefix("read_bytes: ") {
            read_bytes = value.trim().parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("write_bytes: ") {
            write_bytes = value.trim().parse().unwrap_or(0);
        }
    }

    (read_bytes, write_bytes)
}
