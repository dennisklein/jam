// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Wire format types for collector-to-coordinator NDJSON protocol.
//!
//! These types carry raw cumulative counters (not rates) with timestamps.
//! The coordinator computes rates from consecutive snapshots.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::process::ProcessState;

/// Protocol version for backwards compatibility checks
pub const PROTOCOL_VERSION: u32 = 1;

/// A snapshot of metrics from a single node, sent as one NDJSON line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSnapshot {
    /// Message type discriminator (always "snapshot")
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Hostname of the node sending this snapshot
    pub node: String,

    /// Unix timestamp in milliseconds when metrics were sampled (collector's clock).
    ///
    /// Precision: Milliseconds provide sufficient resolution for collection intervals
    /// >= 100ms. For sub-100ms intervals, consider using microseconds to avoid
    /// elapsed time calculation errors exceeding 1%.
    ///
    /// Important: This timestamp is captured BEFORE parsing the cgroup tree to ensure
    /// accurate rate calculations. The elapsed time between samples is used to convert
    /// cumulative counters to rates.
    pub timestamp: u64,

    /// The cgroup tree with raw metrics
    pub cgroup_tree: CgroupNodeRaw,

    /// Protocol version for compatibility checking
    pub version: u32,

    /// PID of the collector process (for filtering in compact mode)
    pub collector_pid: u32,

    /// If this is the final snapshot before exit, explains why
    /// Values: "completed" | "sigterm" | "error"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_reason: Option<String>,
}

impl NodeSnapshot {
    /// Create a new snapshot with a pre-captured timestamp.
    /// The timestamp should reflect when metrics were sampled,
    /// not when the snapshot was constructed.
    pub fn with_timestamp(
        node: String,
        cgroup_tree: CgroupNodeRaw,
        collector_pid: u32,
        timestamp: u64,
    ) -> Self {
        Self {
            msg_type: "snapshot".to_string(),
            node,
            timestamp,
            cgroup_tree,
            version: PROTOCOL_VERSION,
            collector_pid,
            final_reason: None,
        }
    }

    /// Get current time as milliseconds since UNIX epoch
    pub fn now() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Mark this as the final snapshot with a reason
    pub fn with_final_reason(mut self, reason: &str) -> Self {
        self.final_reason = Some(reason.to_string());
        self
    }
}

/// A cgroup node with raw cumulative counters (no rates).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgroupNodeRaw {
    /// Full path to the cgroup directory
    pub path: PathBuf,

    /// Name of this cgroup (last path component)
    pub name: String,

    /// Child cgroups
    pub children: Vec<CgroupNodeRaw>,

    /// Processes directly in this cgroup
    pub processes: Vec<ProcessRaw>,

    /// Depth in the tree (0 = root of monitored subtree)
    pub depth: usize,

    /// Memory current usage in bytes (from memory.current)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_current: Option<u64>,

    /// Memory limit in bytes (from memory.max, None if "max")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_max: Option<u64>,

    /// CPU usage in microseconds (from cpu.stat usage_usec)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage_usec: Option<u64>,

    /// Number of processes (from pids.current)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pids_current: Option<u32>,

    /// Block I/O read bytes (from io.stat)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io_read_bytes: Option<u64>,

    /// Block I/O write bytes (from io.stat)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io_write_bytes: Option<u64>,
}

impl Default for CgroupNodeRaw {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            name: String::new(),
            children: Vec::new(),
            processes: Vec::new(),
            depth: 0,
            memory_current: None,
            memory_max: None,
            cpu_usage_usec: None,
            pids_current: None,
            io_read_bytes: None,
            io_write_bytes: None,
        }
    }
}

/// A process with raw cumulative counters (no rates).
///
/// Includes `starttime` for PID reuse detection: the coordinator matches
/// processes by (pid, starttime) tuple across snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRaw {
    /// Process ID
    pub pid: u32,

    /// Parent process ID
    pub ppid: u32,

    /// Process start time (field 22 from /proc/[pid]/stat)
    /// Used together with PID to uniquely identify a process instance
    pub starttime: u64,

    /// Process name (comm or basename of cmdline)
    pub name: String,

    /// Full command line (from /proc/[pid]/cmdline)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmdline: Option<String>,

    /// Username of the process owner
    pub user: String,

    /// Process state
    pub state: ProcessState,

    /// Number of threads
    pub num_threads: u32,

    /// Resident set size in bytes
    pub rss: u64,

    /// Virtual memory size in bytes
    pub vsize: u64,

    /// User CPU time in clock ticks
    pub utime: u64,

    /// System CPU time in clock ticks
    pub stime: u64,

    /// Block I/O read bytes (from /proc/[pid]/io)
    pub io_read_bytes: u64,

    /// Block I/O write bytes (from /proc/[pid]/io)
    pub io_write_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_snapshot_serialization() {
        let snapshot = NodeSnapshot::with_timestamp(
            "node001".to_string(),
            CgroupNodeRaw {
                path: PathBuf::from("/sys/fs/cgroup/system.slice/job_123"),
                name: "job_123".to_string(),
                children: vec![],
                processes: vec![ProcessRaw {
                    pid: 1234,
                    ppid: 1,
                    starttime: 12345678,
                    name: "python".to_string(),
                    cmdline: Some("python train.py".to_string()),
                    user: "testuser".to_string(),
                    state: ProcessState::Running,
                    num_threads: 4,
                    rss: 1024 * 1024 * 100,
                    vsize: 1024 * 1024 * 500,
                    utime: 1000,
                    stime: 500,
                    io_read_bytes: 1024 * 1024,
                    io_write_bytes: 512 * 1024,
                }],
                depth: 0,
                memory_current: Some(1024 * 1024 * 100),
                memory_max: Some(1024 * 1024 * 1024),
                cpu_usage_usec: Some(1500000),
                pids_current: Some(1),
                io_read_bytes: Some(1024 * 1024),
                io_write_bytes: Some(512 * 1024),
            },
            9999, // collector_pid
            NodeSnapshot::now(),
        );

        // Serialize to JSON
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"type\":\"snapshot\""));
        assert!(json.contains("\"node\":\"node001\""));
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"collector_pid\":9999"));

        // Deserialize back
        let parsed: NodeSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.node, "node001");
        assert_eq!(parsed.collector_pid, 9999);
        assert_eq!(parsed.cgroup_tree.processes.len(), 1);
        assert_eq!(parsed.cgroup_tree.processes[0].pid, 1234);
        assert_eq!(parsed.cgroup_tree.processes[0].starttime, 12345678);
    }

    #[test]
    fn test_final_reason() {
        let snapshot = NodeSnapshot::with_timestamp("node001".to_string(), CgroupNodeRaw::default(), 1234, NodeSnapshot::now())
            .with_final_reason("completed");

        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"final_reason\":\"completed\""));
    }

    #[test]
    fn test_optional_fields_skipped() {
        let cgroup = CgroupNodeRaw {
            path: PathBuf::from("/test"),
            name: "test".to_string(),
            children: vec![],
            processes: vec![],
            depth: 0,
            memory_current: None,
            memory_max: None,
            cpu_usage_usec: None,
            pids_current: None,
            io_read_bytes: None,
            io_write_bytes: None,
        };

        let json = serde_json::to_string(&cgroup).unwrap();
        // None fields should be skipped
        assert!(!json.contains("memory_current"));
        assert!(!json.contains("cpu_usage_usec"));
    }
}
