// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::process::types::Process;

/// Metrics from cgroup controllers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgroupMetrics {
    /// Memory current usage in bytes (from memory.current)
    pub memory_current: Option<u64>,
    /// Memory limit in bytes (from memory.max, None if "max")
    pub memory_max: Option<u64>,
    /// CPU usage in microseconds (from cpu.stat usage_usec)
    pub cpu_usage_usec: Option<u64>,
    /// Number of processes (from pids.current)
    pub pids_current: Option<u32>,
    /// CPU percentage (calculated between samples, NAN if no baseline yet)
    pub cpu_percent: f32,
    /// Block I/O read bytes (from io.stat)
    pub io_read_bytes: Option<u64>,
    /// Block I/O write bytes (from io.stat)
    pub io_write_bytes: Option<u64>,
    /// Block I/O read rate in bytes/sec (calculated between samples, NAN if no baseline)
    pub io_read_rate: f32,
    /// Block I/O write rate in bytes/sec (calculated between samples, NAN if no baseline)
    pub io_write_rate: f32,
}

impl Default for CgroupMetrics {
    fn default() -> Self {
        Self {
            memory_current: None,
            memory_max: None,
            cpu_usage_usec: None,
            pids_current: None,
            // Use NAN for rates to indicate "no baseline yet" (first sample)
            // This allows UI to distinguish "pending" from "actually zero"
            cpu_percent: f32::NAN,
            io_read_bytes: None,
            io_write_bytes: None,
            io_read_rate: f32::NAN,
            io_write_rate: f32::NAN,
        }
    }
}

/// A node in the cgroup hierarchy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgroupNode {
    /// Full path to the cgroup directory
    pub path: PathBuf,
    /// Name of this cgroup (last path component)
    pub name: String,
    /// Child cgroups
    pub children: Vec<CgroupNode>,
    /// Processes directly in this cgroup (from cgroup.procs)
    pub processes: Vec<Process>,
    /// Resource metrics from cgroup controllers
    pub metrics: CgroupMetrics,
    /// Depth in the tree (0 = root)
    pub depth: usize,
}

impl CgroupNode {
    pub fn new(path: PathBuf, depth: usize) -> Self {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());

        Self {
            path,
            name,
            children: Vec::new(),
            processes: Vec::new(),
            metrics: CgroupMetrics::default(),
            depth,
        }
    }

    /// Check if this is a leaf node (no child cgroups)
    #[allow(dead_code)]
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    /// Total number of processes in this subtree
    #[allow(dead_code)]
    pub fn total_processes(&self) -> usize {
        self.processes.len()
            + self
                .children
                .iter()
                .map(|c| c.total_processes())
                .sum::<usize>()
    }
}
