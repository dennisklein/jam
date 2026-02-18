// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Coordinator mode: runs on login/submit node, spawns collectors and aggregates metrics.
//!
//! Single-threaded async design using tokio:
//! - Async process spawning and stdout reading
//! - Async terminal event handling
//! - Timer-based rendering at 30 FPS
//! - All multiplexed via tokio::select!

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::stream::{SelectAll, Stream, StreamExt};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::interval;

use super::slurm::{expand_nodelist, get_job_info, JobInfo};
use super::types::{CgroupNodeRaw, NodeSnapshot, ProcessRaw, PROTOCOL_VERSION};
use crate::app::{FlatCgroup, FlatProcess, SortColumn, SortDirection, TreeItem};
use crate::cgroup::CgroupNode;
use crate::process::{Process, ProcessState};

/// Status of a node's collector
#[derive(Debug, Clone)]
pub enum NodeStatus {
    /// Collector is running and sending data
    Connected,
    /// Waiting for first snapshot
    Connecting,
    /// Collector has disconnected
    Disconnected,
}

/// A received snapshot
struct ReceivedSnapshot {
    snapshot: NodeSnapshot,
}

/// Per-node statistics
#[derive(Debug, Clone, Default)]
pub struct NodeStats {
    pub bytes_received: u64,
    pub messages_received: u64,
    pub last_message: Option<Instant>,
    pub last_snapshot_ts: Option<u64>,
    pub msg_rate: f32,
    pub byte_rate: f32,
}

/// Coordinator state - owned by the single async task, no locks needed
pub struct CoordinatorState {
    pub job_info: JobInfo,
    node_snapshots: HashMap<String, ReceivedSnapshot>,
    prev_snapshots: HashMap<String, NodeSnapshot>,
    pub node_status: HashMap<String, NodeStatus>,
    pub node_stats: HashMap<String, NodeStats>,
    pub items: Vec<TreeItem>,
    pub selected: usize,
    pub scroll: usize,
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
    pub error: Option<String>,
    pub should_quit: bool,
    pub self_stats: crate::app::SelfStats,
    pub refresh_interval: Duration,
    pub last_refresh: Instant,
    connected_count: usize,
    clock_ticks: u64,
    needs_rebuild: bool,
    pub compact_mode: bool,
    collector_pids: HashMap<String, u32>,
    prev_self_cpu: Option<PrevSelfCpu>,
    last_self_stats: Instant,
}

struct PrevSelfCpu {
    total_time: u64,
    timestamp: Instant,
}

impl CoordinatorState {
    pub fn new(job_info: JobInfo, refresh_ms: u64) -> Self {
        let now = Instant::now();
        Self {
            job_info,
            node_snapshots: HashMap::new(),
            prev_snapshots: HashMap::new(),
            node_status: HashMap::new(),
            node_stats: HashMap::new(),
            items: Vec::new(),
            selected: 0,
            scroll: 0,
            sort_column: SortColumn::Cpu,
            sort_direction: SortDirection::Descending,
            error: None,
            should_quit: false,
            self_stats: crate::app::SelfStats::default(),
            refresh_interval: Duration::from_millis(refresh_ms),
            last_refresh: now,
            connected_count: 0,
            clock_ticks: 100,
            needs_rebuild: false,
            compact_mode: true,
            collector_pids: HashMap::new(),
            prev_self_cpu: None,
            last_self_stats: now,
        }
    }

    /// Process an incoming snapshot
    pub fn process_snapshot(&mut self, snapshot: NodeSnapshot, byte_count: usize) {
        let node = snapshot.node.clone();
        let now = Instant::now();
        let snapshot_ts = snapshot.timestamp;

        self.collector_pids.insert(node.clone(), snapshot.collector_pid);
        self.node_status.insert(node.clone(), NodeStatus::Connected);

        // Update stats
        let stats = self.node_stats.entry(node.clone()).or_default();
        stats.bytes_received += byte_count as u64;
        stats.messages_received += 1;

        if let Some(last_ts) = stats.last_snapshot_ts {
            let elapsed_ms = snapshot_ts.saturating_sub(last_ts);
            if elapsed_ms > 0 {
                let elapsed_secs = elapsed_ms as f32 / 1000.0;
                let alpha = 0.3;
                stats.msg_rate = alpha * (1.0 / elapsed_secs) + (1.0 - alpha) * stats.msg_rate;
                stats.byte_rate = alpha * (byte_count as f32 / elapsed_secs) + (1.0 - alpha) * stats.byte_rate;
            }
        } else {
            let refresh_secs = self.refresh_interval.as_secs_f32();
            stats.msg_rate = 1.0 / refresh_secs;
            stats.byte_rate = byte_count as f32 / refresh_secs;
        }
        stats.last_message = Some(now);
        stats.last_snapshot_ts = Some(snapshot_ts);

        self.node_snapshots.insert(
            node,
            ReceivedSnapshot { snapshot },
        );

        self.needs_rebuild = true;
        self.last_refresh = now;
    }

    pub fn rebuild_if_needed(&mut self) {
        if self.needs_rebuild {
            self.rebuild_items();
            self.needs_rebuild = false;
        }
    }

    pub fn update_self_stats(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_self_stats) >= Duration::from_millis(250) {
            if let Ok(content) = std::fs::read_to_string("/proc/self/stat") {
                if let Some((utime, stime, rss)) = parse_self_stat(&content) {
                    let page_size = 4096u64;
                    self.self_stats.memory_bytes = rss * page_size;

                    let total_time = utime + stime;
                    if let Some(ref prev) = self.prev_self_cpu {
                        let elapsed = now.duration_since(prev.timestamp).as_secs_f64();
                        if elapsed > 0.0 {
                            let delta_ticks = total_time.saturating_sub(prev.total_time);
                            let delta_secs = delta_ticks as f64 / self.clock_ticks as f64;
                            self.self_stats.cpu_percent = (delta_secs / elapsed * 100.0) as f32;
                        }
                    }

                    self.prev_self_cpu = Some(PrevSelfCpu {
                        total_time,
                        timestamp: now,
                    });
                }
            }
            self.last_self_stats = now;
        }
    }

    pub fn toggle_compact_mode(&mut self) {
        self.compact_mode = !self.compact_mode;
        self.needs_rebuild = true;
    }

    pub fn total_data_rate(&self) -> f32 {
        self.node_stats.values().map(|s| s.byte_rate).sum()
    }

    fn convert_to_display_tree(&self, raw: &CgroupNodeRaw, _node: &str) -> CgroupNode {
        self.convert_node(raw, &CgroupNodeRaw::default(), 0.0, false)
    }

    fn compute_rates(
        &self,
        current: &CgroupNodeRaw,
        prev: &CgroupNodeRaw,
        elapsed_secs: f64,
    ) -> CgroupNode {
        self.convert_node(current, prev, elapsed_secs, true)
    }

    fn convert_node(
        &self,
        raw: &CgroupNodeRaw,
        prev: &CgroupNodeRaw,
        elapsed_secs: f64,
        calc_rates: bool,
    ) -> CgroupNode {
        let mut cgroup = CgroupNode {
            path: raw.path.clone(),
            name: raw.name.clone(),
            children: Vec::new(),
            processes: Vec::new(),
            metrics: crate::cgroup::CgroupMetrics {
                memory_current: raw.memory_current,
                memory_max: raw.memory_max,
                cpu_usage_usec: raw.cpu_usage_usec,
                pids_current: raw.pids_current,
                io_read_bytes: raw.io_read_bytes,
                io_write_bytes: raw.io_write_bytes,
                ..Default::default()
            },
            depth: raw.depth,
        };

        if calc_rates && elapsed_secs > 0.0 {
            if let (Some(curr_cpu), Some(prev_cpu)) = (raw.cpu_usage_usec, prev.cpu_usage_usec) {
                let delta_usec = curr_cpu.saturating_sub(prev_cpu);
                let delta_secs = delta_usec as f64 / 1_000_000.0;
                cgroup.metrics.cpu_percent = (delta_secs / elapsed_secs * 100.0) as f32;
            }

            if let (Some(curr_read), Some(prev_read)) = (raw.io_read_bytes, prev.io_read_bytes) {
                cgroup.metrics.io_read_rate = (curr_read.saturating_sub(prev_read) as f64 / elapsed_secs) as f32;
            }
            if let (Some(curr_write), Some(prev_write)) = (raw.io_write_bytes, prev.io_write_bytes) {
                cgroup.metrics.io_write_rate = (curr_write.saturating_sub(prev_write) as f64 / elapsed_secs) as f32;
            }
        }

        let prev_proc_map: HashMap<(u32, u64), &ProcessRaw> = prev
            .processes
            .iter()
            .map(|p| ((p.pid, p.starttime), p))
            .collect();

        for proc_raw in &raw.processes {
            let prev_proc = prev_proc_map.get(&(proc_raw.pid, proc_raw.starttime));
            let process = self.convert_process(proc_raw, prev_proc.copied(), elapsed_secs, calc_rates);
            cgroup.processes.push(process);
        }

        let prev_child_map: HashMap<&PathBuf, &CgroupNodeRaw> =
            prev.children.iter().map(|c| (&c.path, c)).collect();

        let default_cgroup = CgroupNodeRaw::default();
        for child_raw in &raw.children {
            let prev_child = prev_child_map
                .get(&child_raw.path)
                .copied()
                .unwrap_or(&default_cgroup);
            let child = self.convert_node(child_raw, prev_child, elapsed_secs, calc_rates);
            cgroup.children.push(child);
        }

        cgroup
    }

    fn convert_process(
        &self,
        raw: &ProcessRaw,
        prev: Option<&ProcessRaw>,
        elapsed_secs: f64,
        calc_rates: bool,
    ) -> Process {
        // Use NAN for rates when no baseline exists yet (first sample)
        // This allows UI to distinguish "pending" from "actually zero"
        let (cpu_percent, io_read_rate, io_write_rate) = if calc_rates && elapsed_secs > 0.0 {
            if let Some(prev) = prev {
                let curr_cpu = raw.utime + raw.stime;
                let prev_cpu = prev.utime + prev.stime;
                let delta_ticks = curr_cpu.saturating_sub(prev_cpu);
                let delta_secs = delta_ticks as f64 / self.clock_ticks as f64;
                let cpu = (delta_secs / elapsed_secs * 100.0) as f32;
                let io_r = (raw.io_read_bytes.saturating_sub(prev.io_read_bytes) as f64 / elapsed_secs) as f32;
                let io_w = (raw.io_write_bytes.saturating_sub(prev.io_write_bytes) as f64 / elapsed_secs) as f32;
                (cpu, io_r, io_w)
            } else {
                // New process, no previous sample
                (f32::NAN, f32::NAN, f32::NAN)
            }
        } else {
            // First snapshot for this node, no baseline
            (f32::NAN, f32::NAN, f32::NAN)
        };

        Process {
            pid: raw.pid,
            ppid: raw.ppid,
            starttime: raw.starttime,
            name: raw.name.clone(),
            cmdline: raw.cmdline.clone(),
            user: raw.user.clone(),
            state: raw.state,
            utime: raw.utime,
            stime: raw.stime,
            rss: raw.rss,
            vsize: raw.vsize,
            num_threads: raw.num_threads,
            cpu_percent,
            io_read_bytes: raw.io_read_bytes,
            io_write_bytes: raw.io_write_bytes,
            io_read_rate,
            io_write_rate,
        }
    }

    fn rebuild_items(&mut self) {
        self.items.clear();
        self.connected_count = 0;

        let mut node_trees: Vec<(String, CgroupNode)> = Vec::new();

        for (node_name, received) in &self.node_snapshots {
            if matches!(self.node_status.get(node_name), Some(NodeStatus::Disconnected)) {
                continue;
            }

            if matches!(self.node_status.get(node_name), Some(NodeStatus::Connected)) {
                self.connected_count += 1;
            }

            let prev = self.prev_snapshots.get(node_name);
            let mut tree = if let Some(prev) = prev {
                let elapsed_secs =
                    (received.snapshot.timestamp.saturating_sub(prev.timestamp)) as f64 / 1000.0;
                if elapsed_secs > 0.0 {
                    self.compute_rates(&received.snapshot.cgroup_tree, &prev.cgroup_tree, elapsed_secs)
                } else {
                    self.convert_to_display_tree(&received.snapshot.cgroup_tree, node_name)
                }
            } else {
                self.convert_to_display_tree(&received.snapshot.cgroup_tree, node_name)
            };

            if self.compact_mode {
                let collector_pid = self.collector_pids.get(node_name).copied();
                apply_compact_filter(&mut tree, collector_pid);
            }

            node_trees.push((node_name.clone(), tree));
        }

        node_trees.sort_by(|a, b| a.0.cmp(&b.0));

        let merged = merge_node_trees(&node_trees);

        flatten_merged_tree(
            &merged,
            0,
            true,
            "",
            &mut self.items,
            self.sort_column,
            self.sort_direction,
        );

        // Update prev_snapshots AFTER using them for rate computation
        let updates: Vec<_> = self
            .node_snapshots
            .iter()
            .map(|(k, v)| (k.clone(), v.snapshot.clone()))
            .collect();
        for (node_name, snapshot) in updates {
            self.prev_snapshots.insert(node_name, snapshot);
        }
    }

    pub fn mark_disconnected(&mut self, node: &str, _reason: &str) {
        self.node_status.insert(node.to_string(), NodeStatus::Disconnected);
        // Clear stale data for disconnected node
        self.node_snapshots.remove(node);
        self.prev_snapshots.remove(node);
        self.collector_pids.remove(node);
        // Reset rates to zero (keep counters for stats display)
        if let Some(stats) = self.node_stats.get_mut(node) {
            stats.byte_rate = 0.0;
            stats.msg_rate = 0.0;
        }
        self.needs_rebuild = true;
    }

    pub fn connected_nodes(&self) -> usize {
        self.connected_count
    }

    pub fn total_nodes(&self) -> usize {
        self.node_status.len()
    }

    // Navigation methods
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn select_next(&mut self) {
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
        }
    }

    pub fn page_up(&mut self, page_size: usize) {
        self.selected = self.selected.saturating_sub(page_size);
    }

    pub fn page_down(&mut self, page_size: usize) {
        self.selected = (self.selected + page_size).min(self.items.len().saturating_sub(1));
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        if !self.items.is_empty() {
            self.selected = self.items.len() - 1;
        }
    }

    pub fn sort_next_column(&mut self) {
        self.sort_column = self.sort_column.next();
        self.needs_rebuild = true;
    }

    pub fn sort_prev_column(&mut self) {
        self.sort_column = self.sort_column.prev();
        self.needs_rebuild = true;
    }

    pub fn toggle_sort_direction(&mut self) {
        self.sort_direction = self.sort_direction.toggle();
        self.needs_rebuild = true;
    }
}

// ============================================================================
// Compact mode filtering
// ============================================================================

fn apply_compact_filter(tree: &mut CgroupNode, collector_pid: Option<u32>) {
    for child in &mut tree.children {
        apply_compact_filter(child, collector_pid);
    }

    if let Some(pid) = collector_pid {
        tree.processes.retain(|p| p.pid != pid);
    }

    let mut promoted_children = Vec::new();
    tree.children.retain(|child| {
        if child.name == "user" {
            for mut grandchild in child.children.clone() {
                adjust_depth(&mut grandchild, -1);
                promoted_children.push(grandchild);
            }
            false
        } else {
            true
        }
    });
    tree.children.append(&mut promoted_children);

    tree.children.retain(|child| child.name != "slurm");
    tree.children.retain(|child| !is_effectively_empty(child));
    tree.children.sort_by(|a, b| a.name.cmp(&b.name));
}

fn adjust_depth(node: &mut CgroupNode, delta: i32) {
    node.depth = (node.depth as i32 + delta).max(0) as usize;
    for child in &mut node.children {
        adjust_depth(child, delta);
    }
}

fn is_effectively_empty(node: &CgroupNode) -> bool {
    node.processes.is_empty() && node.children.iter().all(is_effectively_empty)
}

// ============================================================================
// Tree merging for multi-node display
// ============================================================================

#[derive(Debug)]
struct MergedCgroup {
    name: String,
    node_data: Vec<(String, CgroupNode)>,
    children: Vec<MergedCgroup>,
    is_shared: bool,
}

fn merge_node_trees(node_trees: &[(String, CgroupNode)]) -> MergedCgroup {
    if node_trees.is_empty() {
        return MergedCgroup {
            name: String::new(),
            node_data: Vec::new(),
            children: Vec::new(),
            is_shared: true,
        };
    }

    let root_name = &node_trees[0].1.name;
    let node_data: Vec<(String, CgroupNode)> = node_trees
        .iter()
        .map(|(name, tree)| (name.clone(), tree.clone()))
        .collect();

    let children = merge_children(node_trees);
    let is_shared = node_trees.len() > 1;

    MergedCgroup {
        name: root_name.clone(),
        node_data,
        children,
        is_shared,
    }
}

fn merge_children(node_trees: &[(String, CgroupNode)]) -> Vec<MergedCgroup> {
    use std::collections::BTreeMap;

    let mut children_by_name: BTreeMap<String, Vec<(String, CgroupNode)>> = BTreeMap::new();

    for (node_name, tree) in node_trees {
        for child in &tree.children {
            children_by_name
                .entry(child.name.clone())
                .or_default()
                .push((node_name.clone(), child.clone()));
        }
    }

    let total_nodes = node_trees.len();
    children_by_name
        .into_iter()
        .map(|(name, child_node_data)| {
            let is_shared = child_node_data.len() == total_nodes;
            let children = merge_children(&child_node_data);
            MergedCgroup {
                name,
                node_data: child_node_data,
                children,
                is_shared,
            }
        })
        .collect()
}

fn flatten_merged_tree(
    merged: &MergedCgroup,
    depth: usize,
    is_last: bool,
    parent_prefix: &str,
    items: &mut Vec<TreeItem>,
    sort_column: SortColumn,
    sort_direction: SortDirection,
) {
    if merged.node_data.is_empty() {
        return;
    }

    let prefix = if depth == 0 {
        String::new()
    } else {
        parent_prefix.to_string()
    };

    if merged.is_shared || merged.node_data.len() == 1 {
        let node = if merged.node_data.len() == 1 {
            Some(merged.node_data[0].0.clone())
        } else {
            None
        };

        let (cpu_percent, memory_current, pids_current, io_read_rate, io_write_rate) =
            aggregate_metrics(&merged.node_data);

        let data = FlatCgroup {
            name: merged.name.clone(),
            node,
            cpu_percent,
            memory_current,
            pids_current,
            io_read_rate,
            io_write_rate,
        };

        items.push(TreeItem::Cgroup {
            data,
            depth,
            is_last,
            prefix: prefix.clone(),
        });

        let child_prefix: String = if depth == 0 {
            String::new()
        } else if is_last {
            format!("{}    ", parent_prefix)
        } else {
            format!("{}│   ", parent_prefix)
        };

        let mut sorted_children: Vec<_> = merged.children.iter().collect();
        sort_merged_cgroups(&mut sorted_children, sort_column, sort_direction);

        let all_processes: Vec<(String, &Process)> = merged
            .node_data
            .iter()
            .flat_map(|(node, cg)| cg.processes.iter().map(move |p| (node.clone(), p)))
            .collect();

        let child_count = sorted_children.len();
        for (i, child) in sorted_children.iter().enumerate() {
            let child_is_last = i == child_count - 1 && all_processes.is_empty();
            flatten_merged_tree(
                child,
                depth + 1,
                child_is_last,
                &child_prefix,
                items,
                sort_column,
                sort_direction,
            );
        }

        if !all_processes.is_empty() {
            flatten_processes_merged(&all_processes, &child_prefix, items, sort_column, sort_direction);
        }
    } else {
        let num_nodes = merged.node_data.len();
        for (i, (node_name, cgroup)) in merged.node_data.iter().enumerate() {
            let node_is_last = i == num_nodes - 1 && is_last;

            let data = FlatCgroup {
                name: merged.name.clone(),
                node: Some(node_name.clone()),
                cpu_percent: cgroup.metrics.cpu_percent,
                memory_current: cgroup.metrics.memory_current,
                pids_current: cgroup.metrics.pids_current,
                io_read_rate: cgroup.metrics.io_read_rate,
                io_write_rate: cgroup.metrics.io_write_rate,
            };

            items.push(TreeItem::Cgroup {
                data,
                depth,
                is_last: node_is_last,
                prefix: prefix.clone(),
            });

            let child_prefix: String = if depth == 0 {
                String::new()
            } else if node_is_last {
                format!("{}    ", parent_prefix)
            } else {
                format!("{}│   ", parent_prefix)
            };

            let mut sorted_children: Vec<_> = cgroup.children.iter().collect();
            sort_cgroups(&mut sorted_children, sort_column, sort_direction);

            let child_count = sorted_children.len();
            for (j, child) in sorted_children.iter().enumerate() {
                let child_is_last = j == child_count - 1 && cgroup.processes.is_empty();
                flatten_cgroup_with_node(
                    child,
                    node_name,
                    depth + 1,
                    child_is_last,
                    &child_prefix,
                    items,
                    sort_column,
                    sort_direction,
                );
            }

            if !cgroup.processes.is_empty() {
                let node_processes: Vec<(String, &Process)> = cgroup
                    .processes
                    .iter()
                    .map(|p| (node_name.clone(), p))
                    .collect();
                flatten_processes_merged(&node_processes, &child_prefix, items, sort_column, sort_direction);
            }
        }
    }
}

fn aggregate_metrics(node_data: &[(String, CgroupNode)]) -> (f32, Option<u64>, Option<u32>, f32, f32) {
    let mut cpu_percent = 0.0f32;
    let mut memory_current: Option<u64> = None;
    let mut pids_current: Option<u32> = None;
    let mut io_read_rate = 0.0f32;
    let mut io_write_rate = 0.0f32;

    for (_, cg) in node_data {
        cpu_percent += cg.metrics.cpu_percent;
        if let Some(mem) = cg.metrics.memory_current {
            *memory_current.get_or_insert(0) += mem;
        }
        if let Some(pids) = cg.metrics.pids_current {
            *pids_current.get_or_insert(0) += pids;
        }
        io_read_rate += cg.metrics.io_read_rate;
        io_write_rate += cg.metrics.io_write_rate;
    }

    (cpu_percent, memory_current, pids_current, io_read_rate, io_write_rate)
}

fn sort_merged_cgroups(cgroups: &mut [&MergedCgroup], column: SortColumn, direction: SortDirection) {
    cgroups.sort_by(|a, b| {
        let (a_cpu, a_mem, a_pids, a_io_r, a_io_w) = aggregate_metrics(&a.node_data);
        let (b_cpu, b_mem, b_pids, b_io_r, b_io_w) = aggregate_metrics(&b.node_data);

        let cmp = match column {
            SortColumn::Name => a.name.cmp(&b.name),
            SortColumn::Cpu => a_cpu.partial_cmp(&b_cpu).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Memory => a_mem.cmp(&b_mem),
            SortColumn::Pids => a_pids.cmp(&b_pids),
            SortColumn::IoRead => a_io_r.partial_cmp(&b_io_r).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::IoWrite => a_io_w.partial_cmp(&b_io_w).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Pid | SortColumn::User | SortColumn::State => a.name.cmp(&b.name),
        };
        match direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        }
    });
}

fn flatten_processes_merged(
    processes: &[(String, &Process)],
    cgroup_prefix: &str,
    items: &mut Vec<TreeItem>,
    sort_column: SortColumn,
    sort_direction: SortDirection,
) {
    if processes.is_empty() {
        return;
    }

    let mut sorted: Vec<_> = processes.iter().collect();
    sorted.sort_by(|(_, a), (_, b)| {
        let cmp = match sort_column {
            SortColumn::Name => a.name.cmp(&b.name),
            SortColumn::Pid => a.pid.cmp(&b.pid),
            SortColumn::User => a.user.cmp(&b.user),
            SortColumn::State => a.state.as_char().cmp(&b.state.as_char()),
            SortColumn::Cpu => a.cpu_percent.partial_cmp(&b.cpu_percent).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Memory => a.rss.cmp(&b.rss),
            SortColumn::IoRead => a.io_read_rate.partial_cmp(&b.io_read_rate).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::IoWrite => a.io_write_rate.partial_cmp(&b.io_write_rate).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Pids => a.num_threads.cmp(&b.num_threads),
        };
        match sort_direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        }
    });

    let proc_count = sorted.len();
    for (i, (node_name, proc)) in sorted.iter().enumerate() {
        let is_last = i == proc_count - 1;

        let data = FlatProcess {
            pid: proc.pid,
            name: proc.name.clone(),
            cmdline: proc.cmdline.clone(),
            node: Some((*node_name).clone()),
            user: proc.user.clone(),
            state: proc.state,
            cpu_percent: proc.cpu_percent,
            rss: proc.rss,
            num_threads: proc.num_threads,
            io_read_rate: proc.io_read_rate,
            io_write_rate: proc.io_write_rate,
        };

        items.push(TreeItem::Process {
            data,
            is_last,
            cgroup_prefix: cgroup_prefix.to_string(),
            process_prefix: String::new(),
        });
    }
}

fn flatten_cgroup_with_node(
    node: &CgroupNode,
    node_name: &str,
    depth: usize,
    is_last: bool,
    parent_prefix: &str,
    items: &mut Vec<TreeItem>,
    sort_column: SortColumn,
    sort_direction: SortDirection,
) {
    let prefix = if depth == 0 {
        String::new()
    } else {
        parent_prefix.to_string()
    };

    let data = FlatCgroup {
        name: node.name.clone(),
        node: Some(node_name.to_string()),
        cpu_percent: node.metrics.cpu_percent,
        memory_current: node.metrics.memory_current,
        pids_current: node.metrics.pids_current,
        io_read_rate: node.metrics.io_read_rate,
        io_write_rate: node.metrics.io_write_rate,
    };

    items.push(TreeItem::Cgroup {
        data,
        depth,
        is_last,
        prefix,
    });

    let child_prefix: String = if depth == 0 {
        String::new()
    } else if is_last {
        format!("{}    ", parent_prefix)
    } else {
        format!("{}│   ", parent_prefix)
    };

    let mut sorted_children: Vec<_> = node.children.iter().collect();
    sort_cgroups(&mut sorted_children, sort_column, sort_direction);

    let child_count = sorted_children.len();
    for (i, child) in sorted_children.iter().enumerate() {
        let child_is_last = i == child_count - 1 && node.processes.is_empty();
        flatten_cgroup_with_node(
            child,
            node_name,
            depth + 1,
            child_is_last,
            &child_prefix,
            items,
            sort_column,
            sort_direction,
        );
    }

    if !node.processes.is_empty() {
        flatten_processes_with_node(
            &node.processes,
            node_name,
            &child_prefix,
            items,
            sort_column,
            sort_direction,
        );
    }
}

fn flatten_processes_with_node(
    processes: &[Process],
    node_name: &str,
    cgroup_prefix: &str,
    items: &mut Vec<TreeItem>,
    sort_column: SortColumn,
    sort_direction: SortDirection,
) {
    use std::collections::{HashMap, HashSet};

    if processes.is_empty() {
        return;
    }

    let pids: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
    let mut children_map: HashMap<u32, Vec<&Process>> = HashMap::new();
    let mut roots: Vec<&Process> = Vec::new();

    for proc in processes {
        if pids.contains(&proc.ppid) {
            children_map.entry(proc.ppid).or_default().push(proc);
        } else {
            roots.push(proc);
        }
    }

    sort_process_refs(&mut roots, sort_column, sort_direction);

    let proc_count = roots.len();
    for (i, proc) in roots.iter().enumerate() {
        let is_last = i == proc_count - 1;
        flatten_process_with_node(
            proc,
            node_name,
            is_last,
            cgroup_prefix,
            "",
            items,
            &children_map,
            sort_column,
            sort_direction,
        );
    }
}

fn flatten_process_with_node(
    proc: &Process,
    node_name: &str,
    is_last: bool,
    cgroup_prefix: &str,
    process_prefix: &str,
    items: &mut Vec<TreeItem>,
    children_map: &HashMap<u32, Vec<&Process>>,
    sort_column: SortColumn,
    sort_direction: SortDirection,
) {
    let data = FlatProcess {
        pid: proc.pid,
        name: proc.name.clone(),
        cmdline: proc.cmdline.clone(),
        node: Some(node_name.to_string()),
        user: proc.user.clone(),
        state: proc.state,
        cpu_percent: proc.cpu_percent,
        rss: proc.rss,
        num_threads: proc.num_threads,
        io_read_rate: proc.io_read_rate,
        io_write_rate: proc.io_write_rate,
    };

    items.push(TreeItem::Process {
        data,
        is_last,
        cgroup_prefix: cgroup_prefix.to_string(),
        process_prefix: process_prefix.to_string(),
    });

    if let Some(children) = children_map.get(&proc.pid) {
        let mut sorted_children = children.clone();
        sort_process_refs(&mut sorted_children, sort_column, sort_direction);

        let child_process_prefix: String = if is_last {
            format!("{}   ", process_prefix)
        } else {
            format!("{}│  ", process_prefix)
        };

        let child_count = sorted_children.len();
        for (i, child) in sorted_children.iter().enumerate() {
            let child_is_last = i == child_count - 1;
            flatten_process_with_node(
                child,
                node_name,
                child_is_last,
                cgroup_prefix,
                &child_process_prefix,
                items,
                children_map,
                sort_column,
                sort_direction,
            );
        }
    }
}

fn sort_process_refs(processes: &mut [&Process], column: SortColumn, direction: SortDirection) {
    processes.sort_by(|a, b| {
        let cmp = match column {
            SortColumn::Name => a.name.cmp(&b.name),
            SortColumn::Pid => a.pid.cmp(&b.pid),
            SortColumn::User => a.user.cmp(&b.user),
            SortColumn::State => a.state.as_char().cmp(&b.state.as_char()),
            SortColumn::Cpu => a.cpu_percent.partial_cmp(&b.cpu_percent).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Memory => a.rss.cmp(&b.rss),
            SortColumn::IoRead => a.io_read_rate.partial_cmp(&b.io_read_rate).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::IoWrite => a.io_write_rate.partial_cmp(&b.io_write_rate).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Pids => a.num_threads.cmp(&b.num_threads),
        };
        match direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        }
    });
}

fn sort_cgroups(cgroups: &mut [&CgroupNode], column: SortColumn, direction: SortDirection) {
    cgroups.sort_by(|a, b| {
        let cmp = match column {
            SortColumn::Name => a.name.cmp(&b.name),
            SortColumn::Cpu => a.metrics.cpu_percent.partial_cmp(&b.metrics.cpu_percent).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Memory => a.metrics.memory_current.cmp(&b.metrics.memory_current),
            SortColumn::Pids => a.metrics.pids_current.cmp(&b.metrics.pids_current),
            SortColumn::IoRead => a.metrics.io_read_rate.partial_cmp(&b.metrics.io_read_rate).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::IoWrite => a.metrics.io_write_rate.partial_cmp(&b.metrics.io_write_rate).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Pid | SortColumn::User | SortColumn::State => a.name.cmp(&b.name),
        };
        match direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        }
    });
}

// ============================================================================
// Async coordinator runtime
// ============================================================================

/// Message from a collector (used internally in async stream)
enum CollectorEvent {
    Line { node: String, line: String },
    Error { node: String, error: String },
    Eof { node: String },
}

/// Run the coordinator mode - single-threaded async
pub fn run_coordinator_mode(jobid: u32, jam_binary: &str, refresh_ms: u64) -> Result<()> {
    // Create a single-threaded tokio runtime
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(run_coordinator_async(jobid, jam_binary, refresh_ms))
}

async fn run_coordinator_async(jobid: u32, jam_binary: &str, refresh_ms: u64) -> Result<()> {
    // Get job info (blocking, but quick)
    let job_info = get_job_info(jobid).context("Failed to get job information")?;

    if !job_info.state.is_running() {
        if job_info.state.is_finished() {
            bail!(
                "Job {} has already finished (state: {:?}, exit: {:?})",
                jobid,
                job_info.state,
                job_info.exit_code
            );
        }
        bail!(
            "Job {} is not running (state: {:?}). Wait for it to start.",
            jobid,
            job_info.state
        );
    }

    let nodes = expand_nodelist(&job_info.nodelist).context("Failed to expand nodelist")?;

    if nodes.is_empty() {
        bail!("Job {} has no allocated nodes", jobid);
    }

    if nodes.len() > 100 {
        eprintln!(
            "Warning: Monitoring {} nodes. This may consume significant resources.",
            nodes.len()
        );
    }

    eprintln!(
        "Monitoring job {} ({}) on {} nodes: {}",
        jobid,
        job_info.name,
        nodes.len(),
        if nodes.len() <= 5 {
            nodes.join(", ")
        } else {
            format!("{}...", nodes[..5].join(", "))
        }
    );

    // Create state (owned, no locks needed)
    let mut state = CoordinatorState::new(job_info.clone(), refresh_ms);

    for node in &nodes {
        state.node_status.insert(node.clone(), NodeStatus::Connecting);
    }

    // Spawn collectors and create async line streams
    let mut collectors: Vec<Child> = Vec::new();
    let mut collector_streams: SelectAll<std::pin::Pin<Box<dyn Stream<Item = CollectorEvent> + Send>>> =
        SelectAll::new();

    for node in &nodes {
        match spawn_collector_async(jobid, node, jam_binary, refresh_ms).await {
            Ok(mut child) => {
                if let Some(stdout) = child.stdout.take() {
                    let stream = create_collector_stream(node.clone(), stdout);
                    collector_streams.push(Box::pin(stream));
                }
                collectors.push(child);
            }
            Err(e) => {
                state.mark_disconnected(node, &format!("Failed to spawn: {}", e));
            }
        }
    }

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Event stream for terminal input
    let mut event_stream = EventStream::new();

    // Render interval (30 FPS)
    let mut render_interval = interval(Duration::from_millis(33));
    render_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // FPS tracking
    let mut frame_count = 0u64;
    let mut fps_start = Instant::now();

    // Main event loop
    loop {
        tokio::select! {
            // biased ensures render interval is checked first for consistent frame rate
            biased;

            // Render at 30 FPS - checked first to ensure UI stays responsive
            _ = render_interval.tick() => {
                state.update_self_stats();
                state.rebuild_if_needed();

                // Update FPS
                frame_count += 1;
                let fps_elapsed = fps_start.elapsed().as_secs_f32();
                if fps_elapsed >= 1.0 {
                    state.self_stats.fps = frame_count as f32 / fps_elapsed;
                    frame_count = 0;
                    fps_start = Instant::now();
                }

                terminal.draw(|f| render_coordinator_ui(f, &state))?;
            }

            // Handle collector output
            Some(event) = collector_streams.next() => {
                match event {
                    CollectorEvent::Line { node, line } => {
                        let byte_count = line.len();
                        match serde_json::from_str::<NodeSnapshot>(&line) {
                            Ok(snapshot) => {
                                if snapshot.version != PROTOCOL_VERSION {
                                    state.error = Some(format!(
                                        "{}: Protocol version mismatch (expected {}, got {})",
                                        node, PROTOCOL_VERSION, snapshot.version
                                    ));
                                } else {
                                    state.process_snapshot(snapshot, byte_count);
                                }
                            }
                            Err(e) => {
                                state.error = Some(format!("{}: JSON parse error: {}", node, e));
                            }
                        }
                    }
                    CollectorEvent::Error { node, error } => {
                        state.mark_disconnected(&node, &error);
                    }
                    CollectorEvent::Eof { node } => {
                        state.mark_disconnected(&node, "Collector exited");
                    }
                }
            }

            // Handle terminal input
            Some(event_result) = event_stream.next() => {
                match event_result {
                    Ok(Event::Key(key)) => {
                        match (key.code, key.modifiers) {
                            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                state.should_quit = true;
                            }
                            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => state.select_prev(),
                            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => state.select_next(),
                            (KeyCode::PageUp, _) | (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                                let height = terminal.size()?.height as usize;
                                state.page_up(height.saturating_sub(2));
                            }
                            (KeyCode::PageDown, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                let height = terminal.size()?.height as usize;
                                state.page_down(height.saturating_sub(2));
                            }
                            (KeyCode::Home, _) | (KeyCode::Char('g'), _) => state.select_first(),
                            (KeyCode::End, _) | (KeyCode::Char('G'), _) => state.select_last(),
                            (KeyCode::Char('>'), _) | (KeyCode::Char('.'), _) => state.sort_next_column(),
                            (KeyCode::Char('<'), _) | (KeyCode::Char(','), _) => state.sort_prev_column(),
                            (KeyCode::Char('I'), _) => state.toggle_sort_direction(),
                            (KeyCode::Char('c'), _) => state.toggle_compact_mode(),
                            _ => {}
                        }
                    }
                    Ok(Event::Resize(_, _)) => {}
                    _ => {}
                }
            }
        }

        if state.should_quit {
            break;
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Kill collectors
    for mut child in collectors {
        let _ = child.kill().await;
    }

    Ok(())
}

/// Create a stream of events from a collector's stdout
fn create_collector_stream(
    node: String,
    stdout: tokio::process::ChildStdout,
) -> impl Stream<Item = CollectorEvent> + Send {
    let reader = BufReader::new(stdout);
    let lines = reader.lines();

    futures::stream::unfold(Some((node, lines)), |state| async move {
        let (node, mut lines) = state?;

        match lines.next_line().await {
            Ok(Some(line)) => {
                let event = CollectorEvent::Line {
                    node: node.clone(),
                    line,
                };
                Some((event, Some((node, lines))))
            }
            Ok(None) => {
                let event = CollectorEvent::Eof { node };
                Some((event, None)) // Stop after Eof
            }
            Err(e) => {
                let event = CollectorEvent::Error {
                    node,
                    error: e.to_string(),
                };
                Some((event, None)) // Stop after Error
            }
        }
    })
}

/// Spawn a collector on a node via srun (async)
async fn spawn_collector_async(
    jobid: u32,
    node: &str,
    jam_binary: &str,
    refresh_ms: u64,
) -> Result<Child> {
    let child = Command::new("srun")
        .args([
            "--jobid",
            &jobid.to_string(),
            "--overlap",
            "--nodelist",
            node,
            "--ntasks=1",
            "--",
            jam_binary,
            "--collector",
            "--node-name",
            node,
            "--refresh",
            &refresh_ms.to_string(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to spawn collector on {}", node))?;

    Ok(child)
}

// ============================================================================
// UI Rendering
// ============================================================================

fn render_coordinator_ui(frame: &mut ratatui::Frame, state: &CoordinatorState) {
    use ratatui::{
        layout::{Constraint, Direction, Layout},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    // Title
    let title = format!(
        " jam - Job {} ({}) | Nodes: {}/{} ",
        state.job_info.job_id,
        state.job_info.name,
        state.connected_nodes(),
        state.total_nodes(),
    );

    let data_rate = state.total_data_rate();
    let data_rate_str = if data_rate >= 1024.0 * 1024.0 {
        format!("{:.1}Mi/s", data_rate / (1024.0 * 1024.0))
    } else if data_rate >= 1024.0 {
        format!("{:.1}Ki/s", data_rate / 1024.0)
    } else {
        format!("{:.0}B/s", data_rate)
    };

    let mode_indicator = if state.compact_mode { "[C]" } else { "[D]" };

    let memory_mb = state.self_stats.memory_bytes as f64 / (1024.0 * 1024.0);
    let self_stats = format!(
        "{} {:.0}fps {} {:.1}% {:.1}Mi ",
        mode_indicator,
        state.self_stats.fps,
        data_rate_str,
        state.self_stats.cpu_percent,
        memory_mb,
    );

    let padding = (chunks[0].width as usize).saturating_sub(title.len() + self_stats.len());
    let padding_str = " ".repeat(padding);

    let header_line = Line::from(vec![
        Span::styled(title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(padding_str, Style::default()),
        Span::styled(self_stats, Style::default().fg(Color::Gray)),
    ]);

    let header = Paragraph::new(header_line).style(Style::default().bg(Color::DarkGray));
    frame.render_widget(header, chunks[0]);

    render_headers(frame, chunks[1], state);
    render_tree(frame, chunks[2], state);
    render_footer(frame, chunks[3], state);
}

fn render_headers(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &CoordinatorState) {
    use ratatui::{
        layout::Constraint,
        style::{Color, Modifier, Style},
        widgets::{Cell, Row, Table},
    };

    let header_style = Style::default().fg(Color::White).bg(Color::Blue).add_modifier(Modifier::BOLD);
    let sorted_style = Style::default().fg(Color::LightRed).bg(Color::Blue).add_modifier(Modifier::BOLD);

    let indicator = match state.sort_direction {
        SortDirection::Ascending => "▲",
        SortDirection::Descending => "▼",
    };

    let name_width = area.width.saturating_sub(8 + 6 + 3 + 6 + 7 + 9 + 6 + 8 + 8 + 8);

    let fmt_header = |name: &str, col: SortColumn, width: usize, right_align: bool| {
        let is_sorted = state.sort_column == col;
        let text = if is_sorted {
            if right_align {
                format!("{}{:>width$}", indicator, name, width = width - 1)
            } else {
                format!("{}{:<width$}", indicator, name, width = width - 1)
            }
        } else if right_align {
            format!("{:>width$}", name, width = width)
        } else {
            format!("{:<width$}", name, width = width)
        };
        Cell::from(text).style(if is_sorted { sorted_style } else { header_style })
    };

    let headers = Row::new(vec![
        fmt_header("PID", SortColumn::Pid, 8, true),
        fmt_header("USER", SortColumn::User, 6, false),
        fmt_header("S", SortColumn::State, 3, false),
        Cell::from(format!("{:<6}", "NODE")).style(header_style),
        fmt_header("CPU%", SortColumn::Cpu, 7, true),
        fmt_header("MEM", SortColumn::Memory, 9, true),
        fmt_header("TASKS", SortColumn::Pids, 6, true),
        fmt_header("IO-R/s", SortColumn::IoRead, 8, true),
        fmt_header("IO-W/s", SortColumn::IoWrite, 8, true),
        fmt_header("NAME", SortColumn::Name, name_width as usize, false),
    ])
    .style(header_style);

    let table = Table::new(
        vec![headers],
        [
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(3),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Min(name_width),
        ],
    );

    frame.render_widget(table, area);
}

fn render_tree(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &CoordinatorState) {
    use ratatui::{
        layout::Constraint,
        widgets::{Block, Borders, Row, Table},
    };

    let visible_height = area.height as usize;
    let scroll = if state.selected < state.scroll {
        state.selected
    } else if state.selected >= state.scroll + visible_height {
        state.selected - visible_height + 1
    } else {
        state.scroll
    };

    let name_width = area.width.saturating_sub(8 + 6 + 3 + 6 + 7 + 9 + 6 + 8 + 8 + 8) as usize;

    let rows: Vec<Row> = state
        .items
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(idx, item)| render_tree_row(item, idx == state.selected, name_width))
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(3),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Min(name_width as u16),
        ],
    )
    .block(Block::default().borders(Borders::NONE));

    frame.render_widget(table, area);
}

fn render_tree_row(item: &TreeItem, selected: bool, _name_width: usize) -> ratatui::widgets::Row<'static> {
    use ratatui::{
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Cell, Row},
    };

    let row_style = if selected {
        Style::default().bg(Color::DarkGray)
    } else {
        Style::default()
    };

    match item {
        TreeItem::Cgroup { data, depth, is_last, prefix } => {
            let tree_prefix = if *depth > 0 {
                let branch = if *is_last { "└── " } else { "├── " };
                format!("{}{}", prefix, branch)
            } else {
                String::new()
            };

            let name_cell = Line::from(vec![
                Span::styled(tree_prefix, Style::default().fg(Color::Cyan)),
                Span::styled(data.name.clone(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            ]);

            let mem_str = data.memory_current.map(format_bytes).unwrap_or_default();
            let pids_str = data.pids_current.map(|p| p.to_string()).unwrap_or_default();
            let cpu_str = format_cpu(data.cpu_percent);
            let io_r_str = format_rate(data.io_read_rate);
            let io_w_str = format_rate(data.io_write_rate);
            let node_str = data.node.as_deref().unwrap_or("-");

            Row::new(vec![
                Cell::from(format!("{:>8}", "-")).style(Style::default().fg(Color::DarkGray)),
                Cell::from(format!("{:<6}", "-")).style(Style::default().fg(Color::DarkGray)),
                Cell::from(format!("{:^3}", "-")).style(Style::default().fg(Color::DarkGray)),
                Cell::from(format!("{:<6}", node_str)).style(Style::default().fg(Color::Magenta)),
                Cell::from(format!("{:>7}", cpu_str)).style(Style::default().fg(Color::Green)),
                Cell::from(format!("{:>9}", mem_str)).style(Style::default().fg(Color::Cyan)),
                Cell::from(format!("{:>6}", pids_str)).style(Style::default().fg(Color::Blue)),
                Cell::from(format!("{:>8}", io_r_str)).style(Style::default().fg(if io_r_str == "-" { Color::DarkGray } else { Color::LightBlue })),
                Cell::from(format!("{:>8}", io_w_str)).style(Style::default().fg(if io_w_str == "-" { Color::DarkGray } else { Color::LightRed })),
                Cell::from(name_cell),
            ])
            .style(row_style)
        }
        TreeItem::Process { data, is_last, cgroup_prefix, process_prefix } => {
            let branch = if *is_last { "└─ " } else { "├─ " };
            let display_name = data.cmdline.as_ref().unwrap_or(&data.name);
            let name_cell = Line::from(vec![
                Span::styled(cgroup_prefix.clone(), Style::default().fg(Color::Cyan)),
                Span::styled(process_prefix.clone(), Style::default().fg(Color::Yellow)),
                Span::styled(branch, Style::default().fg(Color::Yellow)),
                Span::styled(display_name.clone(), Style::default().fg(Color::Green)),
            ]);

            let user_str = if data.user.len() > 6 {
                format!("{}+", &data.user[..5])
            } else {
                data.user.clone()
            };

            let state_char = data.state.as_char();
            let state_color = match data.state {
                ProcessState::Running => Color::Green,
                ProcessState::Sleeping => Color::Gray,
                ProcessState::DiskSleep => Color::Yellow,
                ProcessState::Stopped => Color::Red,
                ProcessState::Zombie => Color::Magenta,
                _ => Color::Gray,
            };

            let cpu_str = format_cpu(data.cpu_percent);
            let mem_str = format_bytes(data.rss);
            let io_r_str = format_rate(data.io_read_rate);
            let io_w_str = format_rate(data.io_write_rate);
            let node_str = data.node.as_deref().unwrap_or("-");

            Row::new(vec![
                Cell::from(format!("{:>8}", data.pid)).style(Style::default().fg(Color::Yellow)),
                Cell::from(format!("{:<6}", user_str)).style(Style::default().fg(Color::Magenta)),
                Cell::from(format!("{:^3}", state_char)).style(Style::default().fg(state_color)),
                Cell::from(format!("{:<6}", node_str)).style(Style::default().fg(Color::Magenta)),
                Cell::from(format!("{:>7}", cpu_str)).style(Style::default().fg(Color::Green)),
                Cell::from(format!("{:>9}", mem_str)).style(Style::default().fg(Color::Cyan)),
                Cell::from(format!("{:>6}", data.num_threads)).style(Style::default().fg(Color::Blue)),
                Cell::from(format!("{:>8}", io_r_str)).style(Style::default().fg(if io_r_str == "-" { Color::DarkGray } else { Color::LightBlue })),
                Cell::from(format!("{:>8}", io_w_str)).style(Style::default().fg(if io_w_str == "-" { Color::DarkGray } else { Color::LightRed })),
                Cell::from(name_cell),
            ])
            .style(row_style)
        }
    }
}

fn render_footer(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &CoordinatorState) {
    use ratatui::{
        style::{Color, Style},
        text::{Line, Span},
        widgets::Paragraph,
    };

    let text = if let Some(ref err) = state.error {
        Span::styled(format!(" Error: {} ", err), Style::default().fg(Color::Red))
    } else {
        let count = state.items.len();
        let pos = if count > 0 { state.selected + 1 } else { 0 };
        let mode_str = if state.compact_mode { "compact" } else { "detail" };
        Span::styled(
            format!(" {}/{} | q:quit  j/k:nav  </> :sort  I:reverse  c:{} ", pos, count, mode_str),
            Style::default().fg(Color::Gray),
        )
    };

    frame.render_widget(Paragraph::new(Line::from(text)), area);
}

fn parse_self_stat(content: &str) -> Option<(u64, u64, u64)> {
    let end = content.rfind(')')?;
    let rest = &content[end + 2..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    if fields.len() < 22 {
        return None;
    }
    let utime: u64 = fields[11].parse().ok()?;
    let stime: u64 = fields[12].parse().ok()?;
    let rss: u64 = fields[21].parse().ok()?;
    Some((utime, stime, rss))
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    if bytes >= GIB {
        format!("{:.1}Gi", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1}Mi", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.0}Ki", bytes as f64 / KIB as f64)
    } else {
        format!("{}B", bytes)
    }
}

/// Format CPU percentage, showing "-" for pending (NAN) values
fn format_cpu(cpu_percent: f32) -> String {
    if cpu_percent.is_nan() {
        "-".to_string() // No baseline yet (first sample)
    } else {
        format!("{:.1}", cpu_percent)
    }
}

fn format_rate(rate: f32) -> String {
    const KIB: f32 = 1024.0;
    const MIB: f32 = KIB * 1024.0;
    const GIB: f32 = MIB * 1024.0;

    if rate.is_nan() || rate < 0.1 {
        "-".to_string() // Pending or near-zero
    } else if rate >= GIB {
        format!("{:.1}Gi", rate / GIB)
    } else if rate >= MIB {
        format!("{:.1}Mi", rate / MIB)
    } else if rate >= KIB {
        format!("{:.0}Ki", rate / KIB)
    } else {
        format!("{:.0}B", rate)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slurm::types::{CgroupNodeRaw, NodeSnapshot, ProcessRaw};
    use std::path::PathBuf;

    fn create_test_job_info() -> JobInfo {
        JobInfo {
            job_id: 12345,
            name: "test_job".to_string(),
            state: super::super::slurm::JobState::Running,
            user: "testuser".to_string(),
            nodelist: "node[001-002]".to_string(),
            num_nodes: 2,
            partition: "compute".to_string(),
            exit_code: None,
        }
    }

    fn create_test_snapshot(node: &str, timestamp: u64) -> NodeSnapshot {
        NodeSnapshot {
            msg_type: "snapshot".to_string(),
            node: node.to_string(),
            timestamp,
            cgroup_tree: CgroupNodeRaw {
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
                    rss: 100 * 1024 * 1024,
                    vsize: 500 * 1024 * 1024,
                    utime: 1000,
                    stime: 500,
                    io_read_bytes: 1024 * 1024,
                    io_write_bytes: 512 * 1024,
                }],
                depth: 0,
                memory_current: Some(100 * 1024 * 1024),
                memory_max: Some(1024 * 1024 * 1024),
                cpu_usage_usec: Some(1500000),
                pids_current: Some(1),
                io_read_bytes: Some(1024 * 1024),
                io_write_bytes: Some(512 * 1024),
            },
            version: 1,
            collector_pid: 9999,
            final_reason: None,
        }
    }

    #[test]
    fn test_coordinator_state_new() {
        let job_info = create_test_job_info();
        let state = CoordinatorState::new(job_info.clone(), 1000);

        assert_eq!(state.job_info.job_id, 12345);
        assert_eq!(state.refresh_interval, Duration::from_millis(1000));
        assert!(state.items.is_empty());
        assert!(!state.should_quit);
    }

    #[test]
    fn test_process_snapshot() {
        let job_info = create_test_job_info();
        let mut state = CoordinatorState::new(job_info, 1000);

        let snapshot = create_test_snapshot("node001", 1000);
        state.process_snapshot(snapshot, 1000);

        assert_eq!(state.node_snapshots.len(), 1);
        assert!(state.node_snapshots.contains_key("node001"));
        assert!(matches!(
            state.node_status.get("node001"),
            Some(NodeStatus::Connected)
        ));
    }

    #[test]
    fn test_rate_calculation() {
        let job_info = create_test_job_info();
        let mut state = CoordinatorState::new(job_info, 1000);

        let mut snapshot1 = create_test_snapshot("node001", 0);
        snapshot1.cgroup_tree.cpu_usage_usec = Some(0);
        snapshot1.cgroup_tree.io_read_bytes = Some(0);
        state.process_snapshot(snapshot1, 1000);
        state.rebuild_if_needed();

        let mut snapshot2 = create_test_snapshot("node001", 1000);
        snapshot2.cgroup_tree.cpu_usage_usec = Some(500_000);
        snapshot2.cgroup_tree.io_read_bytes = Some(1024 * 1024);
        state.process_snapshot(snapshot2, 1000);
        state.rebuild_if_needed();

        assert_eq!(state.node_snapshots.len(), 1);
        assert_eq!(state.prev_snapshots.len(), 1);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(512), "512B");
        assert_eq!(format_bytes(1024), "1Ki");
        assert_eq!(format_bytes(1024 * 1024), "1.0Mi");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0Gi");
    }

    #[test]
    fn test_format_rate() {
        assert_eq!(format_rate(0.0), "-");
        assert_eq!(format_rate(0.05), "-");
        assert_eq!(format_rate(100.0), "100B");
        assert_eq!(format_rate(1024.0), "1Ki");
    }

    #[test]
    fn test_compact_filter() {
        use crate::cgroup::CgroupMetrics;

        let mut tree = CgroupNode {
            path: PathBuf::from("/sys/fs/cgroup/job_1"),
            name: "job_1".to_string(),
            children: vec![
                CgroupNode {
                    path: PathBuf::from("/sys/fs/cgroup/job_1/step_0"),
                    name: "step_0".to_string(),
                    children: vec![
                        CgroupNode {
                            path: PathBuf::from("/sys/fs/cgroup/job_1/step_0/slurm"),
                            name: "slurm".to_string(),
                            children: vec![],
                            processes: vec![Process {
                                pid: 1000, ppid: 1, starttime: 100,
                                name: "slurmstepd".to_string(), cmdline: None, user: "root".to_string(),
                                state: ProcessState::Sleeping, utime: 0, stime: 0,
                                rss: 0, vsize: 0, num_threads: 1, cpu_percent: 0.0,
                                io_read_bytes: 0, io_write_bytes: 0,
                                io_read_rate: 0.0, io_write_rate: 0.0,
                            }],
                            metrics: CgroupMetrics::default(),
                            depth: 2,
                        },
                        CgroupNode {
                            path: PathBuf::from("/sys/fs/cgroup/job_1/step_0/user"),
                            name: "user".to_string(),
                            children: vec![
                                CgroupNode {
                                    path: PathBuf::from("/sys/fs/cgroup/job_1/step_0/user/task_0"),
                                    name: "task_0".to_string(),
                                    children: vec![],
                                    processes: vec![Process {
                                        pid: 2000, ppid: 1, starttime: 200,
                                        name: "python".to_string(), cmdline: None, user: "testuser".to_string(),
                                        state: ProcessState::Running, utime: 100, stime: 50,
                                        rss: 1024, vsize: 4096, num_threads: 4, cpu_percent: 50.0,
                                        io_read_bytes: 0, io_write_bytes: 0,
                                        io_read_rate: 0.0, io_write_rate: 0.0,
                                    }],
                                    metrics: CgroupMetrics::default(),
                                    depth: 3,
                                },
                            ],
                            processes: vec![],
                            metrics: CgroupMetrics::default(),
                            depth: 2,
                        },
                    ],
                    processes: vec![],
                    metrics: CgroupMetrics::default(),
                    depth: 1,
                },
            ],
            processes: vec![],
            metrics: CgroupMetrics::default(),
            depth: 0,
        };

        apply_compact_filter(&mut tree, Some(9999));

        assert_eq!(tree.name, "job_1");
        assert_eq!(tree.children.len(), 1);
        let step_0 = &tree.children[0];
        assert_eq!(step_0.name, "step_0");
        assert_eq!(step_0.children.len(), 1);
        let task_0 = &step_0.children[0];
        assert_eq!(task_0.name, "task_0");
        assert_eq!(task_0.processes.len(), 1);
        assert_eq!(task_0.processes[0].name, "python");
    }
}
