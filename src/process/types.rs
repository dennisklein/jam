// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

use serde::{Deserialize, Serialize};

/// Process state as reported in /proc/[pid]/stat
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessState {
    Running,
    Sleeping,
    DiskSleep,
    Stopped,
    Zombie,
    Dead,
    Idle,
    Unknown,
}

impl From<char> for ProcessState {
    fn from(c: char) -> Self {
        match c {
            'R' => ProcessState::Running,
            'S' => ProcessState::Sleeping,
            'D' => ProcessState::DiskSleep,
            'T' | 't' => ProcessState::Stopped,
            'Z' => ProcessState::Zombie,
            'X' | 'x' => ProcessState::Dead,
            'I' => ProcessState::Idle,
            _ => ProcessState::Unknown,
        }
    }
}

impl ProcessState {
    pub fn as_char(&self) -> char {
        match self {
            ProcessState::Running => 'R',
            ProcessState::Sleeping => 'S',
            ProcessState::DiskSleep => 'D',
            ProcessState::Stopped => 'T',
            ProcessState::Zombie => 'Z',
            ProcessState::Dead => 'X',
            ProcessState::Idle => 'I',
            ProcessState::Unknown => '?',
        }
    }
}

/// Information about a single process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    /// Process ID
    pub pid: u32,
    /// Parent process ID
    pub ppid: u32,
    /// Process name (comm)
    pub name: String,
    /// Username of the process owner
    pub user: String,
    /// Process state
    pub state: ProcessState,
    /// User CPU time in clock ticks
    pub utime: u64,
    /// System CPU time in clock ticks
    pub stime: u64,
    /// Resident set size in bytes
    pub rss: u64,
    /// Virtual memory size in bytes
    pub vsize: u64,
    /// Number of threads
    pub num_threads: u32,
    /// CPU percentage (calculated between samples)
    pub cpu_percent: f32,
    /// Block I/O read bytes (from /proc/[pid]/io)
    pub io_read_bytes: u64,
    /// Block I/O write bytes (from /proc/[pid]/io)
    pub io_write_bytes: u64,
    /// Block I/O read rate in bytes/sec (calculated between samples)
    pub io_read_rate: f32,
    /// Block I/O write rate in bytes/sec (calculated between samples)
    pub io_write_rate: f32,
}

impl Process {
    /// Total CPU time in clock ticks
    #[allow(dead_code)]
    pub fn total_time(&self) -> u64 {
        self.utime + self.stime
    }
}

/// A node in the process tree (within a cgroup)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ProcessNode {
    pub process: Process,
    pub children: Vec<ProcessNode>,
}

#[allow(dead_code)]
impl ProcessNode {
    pub fn new(process: Process) -> Self {
        Self {
            process,
            children: Vec::new(),
        }
    }
}

/// A tree of processes built from PPID relationships
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ProcessTree {
    /// Root processes (those whose PPID is not in this tree)
    pub roots: Vec<ProcessNode>,
}

#[allow(dead_code)]
impl ProcessTree {
    /// Build a process tree from a flat list of processes
    pub fn from_processes(processes: Vec<Process>) -> Self {
        use std::collections::{HashMap, HashSet};

        if processes.is_empty() {
            return Self::default();
        }

        // Collect all PIDs in this set
        let pids: HashSet<u32> = processes.iter().map(|p| p.pid).collect();

        // Create nodes for all processes
        let mut nodes: HashMap<u32, ProcessNode> = processes
            .into_iter()
            .map(|p| (p.pid, ProcessNode::new(p)))
            .collect();

        // Find roots (processes whose parent is not in our set)
        let root_pids: Vec<u32> = nodes
            .values()
            .filter(|n| !pids.contains(&n.process.ppid))
            .map(|n| n.process.pid)
            .collect();

        // Build tree recursively
        fn build_subtree(
            pid: u32,
            nodes: &mut HashMap<u32, ProcessNode>,
            pids: &HashSet<u32>,
        ) -> Option<ProcessNode> {
            let mut node = nodes.remove(&pid)?;

            // Find children (processes whose PPID is this PID)
            let child_pids: Vec<u32> = nodes
                .values()
                .filter(|n| n.process.ppid == pid)
                .map(|n| n.process.pid)
                .collect();

            for child_pid in child_pids {
                if let Some(child) = build_subtree(child_pid, nodes, pids) {
                    node.children.push(child);
                }
            }

            // Sort children by PID for consistent display
            node.children.sort_by_key(|n| n.process.pid);

            Some(node)
        }

        let mut roots = Vec::new();
        for root_pid in root_pids {
            if let Some(root) = build_subtree(root_pid, &mut nodes, &pids) {
                roots.push(root);
            }
        }

        // Sort roots by PID
        roots.sort_by_key(|n| n.process.pid);

        Self { roots }
    }
}
