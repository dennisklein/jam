// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Collector mode: runs on compute nodes, outputs NDJSON metrics to stdout.
//!
//! The collector discovers its job's cgroup via /proc/self/cgroup and
//! periodically outputs snapshots containing raw cumulative counters.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use super::types::{CgroupNodeRaw, NodeSnapshot, ProcessRaw};
use crate::cgroup::parser::CGROUP_ROOT;
use crate::process::parser::parse_processes;
use crate::process::types::Process;

/// Run the collector mode: discover cgroup, output NDJSON snapshots
pub fn run_collector_mode(
    node_name: Option<String>,
    refresh_ms: u64,
    cgroup_root_override: Option<PathBuf>,
) -> Result<()> {
    // Get node name from argument or hostname
    let node = node_name.unwrap_or_else(|| {
        hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    });

    // Discover the job cgroup root
    let cgroup_root = match cgroup_root_override {
        Some(path) => path,
        None => discover_job_cgroup_root()
            .context("Failed to discover job cgroup root")?,
    };

    // Get our own PID for filtering in compact mode
    let collector_pid = std::process::id();

    eprintln!("Collector starting on node '{}' (pid {}), monitoring: {:?}", node, collector_pid, cgroup_root);

    // Set up signal handling for graceful shutdown
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    // Handle SIGTERM for graceful shutdown
    ctrlc::set_handler(move || {
        shutdown_clone.store(true, Ordering::SeqCst);
    }).ok(); // Ignore error if handler can't be set (e.g., in tests)

    let refresh_interval = Duration::from_millis(refresh_ms);
    let mut stdout = io::stdout().lock();

    loop {
        // Check for shutdown signal
        if shutdown.load(Ordering::SeqCst) {
            // Send final snapshot - capture timestamp before parsing for accurate rate calculation
            let timestamp = NodeSnapshot::now();
            let tree = parse_cgroup_tree_raw(&cgroup_root)?;
            let snapshot = NodeSnapshot::with_timestamp(node.clone(), tree, collector_pid, timestamp)
                .with_final_reason("sigterm");
            writeln!(stdout, "{}", serde_json::to_string(&snapshot)?)?;
            stdout.flush()?;
            eprintln!("Collector received shutdown signal, exiting");
            break;
        }

        // Collect and output snapshot - capture timestamp before parsing for accurate rate calculation
        let timestamp = NodeSnapshot::now();
        let tree = parse_cgroup_tree_raw(&cgroup_root)?;
        let snapshot = NodeSnapshot::with_timestamp(node.clone(), tree, collector_pid, timestamp);
        writeln!(stdout, "{}", serde_json::to_string(&snapshot)?)?;
        stdout.flush()?;

        // Sleep for refresh interval (check for shutdown more frequently)
        let sleep_chunk = Duration::from_millis(100);
        let mut slept = Duration::ZERO;
        while slept < refresh_interval {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(sleep_chunk.min(refresh_interval - slept));
            slept += sleep_chunk;
        }
    }

    Ok(())
}

/// Discover the job cgroup root via /proc/self/cgroup
///
/// For cgroup v2, reads the path from "0::" line and finds the job-level cgroup.
pub fn discover_job_cgroup_root() -> Result<PathBuf> {
    let cgroup_info = fs::read_to_string("/proc/self/cgroup")
        .context("Failed to read /proc/self/cgroup")?;

    let cgroup_path = parse_cgroup_path(&cgroup_info)?;

    // Find the job-level path (strip step_*/task_* suffixes)
    let job_root = extract_job_level_path(&cgroup_path)?;

    let mount_point = find_cgroup_mount_point()?;
    Ok(mount_point.join(job_root.trim_start_matches('/')))
}

/// Parse the cgroup path from /proc/self/cgroup content
fn parse_cgroup_path(content: &str) -> Result<String> {
    // Try cgroup v2 format first: "0::/path/to/cgroup"
    if let Some(line) = content.lines().find(|l| l.starts_with("0::")) {
        let path = line
            .strip_prefix("0::")
            .ok_or_else(|| anyhow!("Invalid cgroup v2 line format"))?;
        return Ok(path.to_string());
    }

    // Fall back to cgroup v1: look for memory or cpu controller
    for line in content.lines() {
        if line.contains(":memory:") || line.contains(":cpu:") || line.contains(":cpuacct:") {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 3 {
                return Ok(parts[2].to_string());
            }
        }
    }

    Err(anyhow!("No suitable cgroup controller found in /proc/self/cgroup"))
}

/// Extract the job-level cgroup path by stripping step/task suffixes
///
/// Typical Slurm hierarchy:
/// /system.slice/slurmstepd.scope/job_123/step_0/user/task_0
/// We want: /system.slice/slurmstepd.scope/job_123
fn extract_job_level_path(path: &str) -> Result<String> {
    // Look for job_<id> in the path
    let parts: Vec<&str> = path.split('/').collect();

    for (i, part) in parts.iter().enumerate() {
        if part.starts_with("job_") {
            // Return path up to and including job_<id>
            return Ok(parts[..=i].join("/"));
        }
    }

    // If no job_ prefix found, try common patterns
    // Pattern: uid_<uid>/job_<id>
    for (i, part) in parts.iter().enumerate() {
        if part.starts_with("uid_") && i + 1 < parts.len() && parts[i + 1].starts_with("job_") {
            return Ok(parts[..=i + 1].join("/"));
        }
    }

    // No job-level path found, use the full path
    // This might happen in non-standard configurations
    Ok(path.to_string())
}

/// Find the cgroup v2 mount point (usually /sys/fs/cgroup)
fn find_cgroup_mount_point() -> Result<PathBuf> {
    // Check standard location first
    let standard = PathBuf::from(CGROUP_ROOT);
    if standard.join("cgroup.procs").exists() || standard.join("cgroup.controllers").exists() {
        return Ok(standard);
    }

    // Parse /proc/mounts to find cgroup2 mount
    let mounts = fs::read_to_string("/proc/mounts")
        .context("Failed to read /proc/mounts")?;

    for line in mounts.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts[2] == "cgroup2" {
            return Ok(PathBuf::from(parts[1]));
        }
    }

    // Fall back to standard location
    Ok(standard)
}

/// Parse a cgroup tree starting from the given root, returning raw counters
pub fn parse_cgroup_tree_raw(root: &Path) -> Result<CgroupNodeRaw> {
    parse_cgroup_node_raw(root, 0)
}

/// Parse a single cgroup node and its children recursively
fn parse_cgroup_node_raw(path: &Path, depth: usize) -> Result<CgroupNodeRaw> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    let mut node = CgroupNodeRaw {
        path: path.to_path_buf(),
        name,
        children: Vec::new(),
        processes: Vec::new(),
        depth,
        ..Default::default()
    };

    // Parse CPU usage from cpu.stat
    if let Ok(content) = fs::read_to_string(path.join("cpu.stat")) {
        for line in content.lines() {
            if let Some(value) = line.strip_prefix("usage_usec ") {
                node.cpu_usage_usec = value.trim().parse().ok();
                break;
            }
        }
    }

    // Parse memory current
    if let Ok(content) = fs::read_to_string(path.join("memory.current")) {
        node.memory_current = content.trim().parse().ok();
    }

    // Parse memory max
    if let Ok(content) = fs::read_to_string(path.join("memory.max")) {
        let trimmed = content.trim();
        if trimmed != "max" {
            node.memory_max = trimmed.parse().ok();
        }
    }

    // Parse I/O stats from io.stat
    if let Ok(content) = fs::read_to_string(path.join("io.stat")) {
        let mut total_read: u64 = 0;
        let mut total_write: u64 = 0;
        for line in content.lines() {
            for part in line.split_whitespace() {
                if let Some(value) = part.strip_prefix("rbytes=") {
                    total_read += value.parse::<u64>().unwrap_or(0);
                } else if let Some(value) = part.strip_prefix("wbytes=") {
                    total_write += value.parse::<u64>().unwrap_or(0);
                }
            }
        }
        if total_read > 0 {
            node.io_read_bytes = Some(total_read);
        }
        if total_write > 0 {
            node.io_write_bytes = Some(total_write);
        }
    }

    // Parse PIDs current
    if let Ok(content) = fs::read_to_string(path.join("pids.current")) {
        node.pids_current = content.trim().parse().ok();
    }

    // Parse cgroup.procs and read process info
    if let Ok(content) = fs::read_to_string(path.join("cgroup.procs")) {
        let pids: Vec<u32> = content
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect();

        node.processes = parse_processes(&pids)
            .into_iter()
            .filter(|p| !is_kernel_thread(p))
            .map(process_to_raw)
            .collect();
    }

    // Find and parse child cgroups
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.filter_map(|e| e.ok()) {
            let entry_path = entry.path();
            if entry_path.is_dir() && entry_path.join("cgroup.procs").exists() {
                if let Ok(child) = parse_cgroup_node_raw(&entry_path, depth + 1) {
                    node.children.push(child);
                }
            }
        }
    }

    node.children.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(node)
}

/// Check if a process is a kernel thread
fn is_kernel_thread(proc: &Process) -> bool {
    proc.pid == 2 || proc.ppid == 2
}

/// Convert Process to ProcessRaw for serialization
fn process_to_raw(proc: Process) -> ProcessRaw {
    ProcessRaw {
        pid: proc.pid,
        ppid: proc.ppid,
        starttime: proc.starttime,
        name: proc.name,
        cmdline: proc.cmdline,
        user: proc.user,
        state: proc.state,
        num_threads: proc.num_threads,
        rss: proc.rss,
        vsize: proc.vsize,
        utime: proc.utime,
        stime: proc.stime,
        io_read_bytes: proc.io_read_bytes,
        io_write_bytes: proc.io_write_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cgroup_path_v2() {
        let content = "0::/system.slice/slurmstepd.scope/job_123/step_0/user/task_0\n";
        let path = parse_cgroup_path(content).unwrap();
        assert_eq!(path, "/system.slice/slurmstepd.scope/job_123/step_0/user/task_0");
    }

    #[test]
    fn test_parse_cgroup_path_v1() {
        let content = "11:memory:/slurm/uid_1000/job_456/step_0\n\
                       10:cpu,cpuacct:/slurm/uid_1000/job_456/step_0\n";
        let path = parse_cgroup_path(content).unwrap();
        assert_eq!(path, "/slurm/uid_1000/job_456/step_0");
    }

    #[test]
    fn test_extract_job_level_path_standard() {
        let path = "/system.slice/slurmstepd.scope/job_123/step_0/user/task_0";
        let job_path = extract_job_level_path(path).unwrap();
        assert_eq!(job_path, "/system.slice/slurmstepd.scope/job_123");
    }

    #[test]
    fn test_extract_job_level_path_uid_prefix() {
        let path = "/slurm/uid_1000/job_456/step_0";
        let job_path = extract_job_level_path(path).unwrap();
        assert_eq!(job_path, "/slurm/uid_1000/job_456");
    }

    #[test]
    fn test_extract_job_level_path_no_job() {
        let path = "/system.slice/some_service.scope";
        let job_path = extract_job_level_path(path).unwrap();
        assert_eq!(job_path, "/system.slice/some_service.scope");
    }
}
