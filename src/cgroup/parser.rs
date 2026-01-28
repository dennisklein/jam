// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

use std::fs;
use std::path::Path;

use anyhow::Result;

use super::types::CgroupNode;
use crate::process::parser::parse_processes;
use crate::process::types::Process;

/// Default cgroup v2 mount point
pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Parse the entire cgroup tree starting from the root
pub fn parse_cgroup_tree() -> Result<CgroupNode> {
    let mut tree = parse_cgroup_node(Path::new(CGROUP_ROOT), 0)?;
    aggregate_metrics(&mut tree);
    Ok(tree)
}

/// Aggregate metrics from children for nodes that don't have direct values
fn aggregate_metrics(node: &mut CgroupNode) {
    // First, recursively aggregate children
    for child in &mut node.children {
        aggregate_metrics(child);
    }

    // If this node is missing memory_current, sum from children
    if node.metrics.memory_current.is_none() && !node.children.is_empty() {
        let sum: u64 = node.children.iter()
            .filter_map(|c| c.metrics.memory_current)
            .sum();
        if sum > 0 {
            node.metrics.memory_current = Some(sum);
        }
    }

    // If this node is missing pids_current, sum from children + direct processes
    if node.metrics.pids_current.is_none() {
        let child_pids: u32 = node.children.iter()
            .filter_map(|c| c.metrics.pids_current)
            .sum();
        let direct_pids = node.processes.len() as u32;
        let total = child_pids + direct_pids;
        if total > 0 {
            node.metrics.pids_current = Some(total);
        }
    }

    // Aggregate I/O read bytes from children
    if node.metrics.io_read_bytes.is_none() && !node.children.is_empty() {
        let sum: u64 = node.children.iter()
            .filter_map(|c| c.metrics.io_read_bytes)
            .sum();
        if sum > 0 {
            node.metrics.io_read_bytes = Some(sum);
        }
    }

    // Aggregate I/O write bytes from children
    if node.metrics.io_write_bytes.is_none() && !node.children.is_empty() {
        let sum: u64 = node.children.iter()
            .filter_map(|c| c.metrics.io_write_bytes)
            .sum();
        if sum > 0 {
            node.metrics.io_write_bytes = Some(sum);
        }
    }
}

/// Check if a process is a kernel thread (kthreadd or its children)
fn is_kernel_thread(proc: &Process) -> bool {
    proc.pid == 2 || proc.ppid == 2
}

/// Parse a single cgroup node and its children recursively
fn parse_cgroup_node(path: &Path, depth: usize) -> Result<CgroupNode> {
    let mut node = CgroupNode::new(path.to_path_buf(), depth);
    let is_root = path == Path::new(CGROUP_ROOT);

    // Parse CPU usage from cpu.stat
    if let Ok(content) = fs::read_to_string(path.join("cpu.stat")) {
        for line in content.lines() {
            if let Some(value) = line.strip_prefix("usage_usec ") {
                node.metrics.cpu_usage_usec = value.trim().parse().ok();
                break;
            }
        }
    }

    // Parse memory current
    if !is_root {
        if let Ok(content) = fs::read_to_string(path.join("memory.current")) {
            node.metrics.memory_current = content.trim().parse().ok();
        }
    }

    // Parse memory max
    if !is_root {
        if let Ok(content) = fs::read_to_string(path.join("memory.max")) {
            let trimmed = content.trim();
            if trimmed != "max" {
                node.metrics.memory_max = trimmed.parse().ok();
            }
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
            node.metrics.io_read_bytes = Some(total_read);
        }
        if total_write > 0 {
            node.metrics.io_write_bytes = Some(total_write);
        }
    }

    // Parse PIDs current
    if !is_root {
        if let Ok(content) = fs::read_to_string(path.join("pids.current")) {
            node.metrics.pids_current = content.trim().parse().ok();
        }
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
            .collect();
    }

    // Find and parse child cgroups
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.filter_map(|e| e.ok()) {
            let entry_path = entry.path();
            if entry_path.is_dir() && entry_path.join("cgroup.procs").exists() {
                if let Ok(child) = parse_cgroup_node(&entry_path, depth + 1) {
                    node.children.push(child);
                }
            }
        }
    }

    node.children.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(node)
}
