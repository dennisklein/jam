// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::cgroup::{parse_cgroup_tree, CgroupNode};
use crate::process::types::Process;

/// Column that can be sorted
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    Pid,
    User,
    State,
    Cpu,
    Memory,
    Pids,
    IoRead,
    IoWrite,
}

impl SortColumn {
    /// Get next column (for cycling with > key)
    pub fn next(self) -> Self {
        match self {
            SortColumn::Name => SortColumn::Pid,
            SortColumn::Pid => SortColumn::User,
            SortColumn::User => SortColumn::State,
            SortColumn::State => SortColumn::Cpu,
            SortColumn::Cpu => SortColumn::Memory,
            SortColumn::Memory => SortColumn::Pids,
            SortColumn::Pids => SortColumn::IoRead,
            SortColumn::IoRead => SortColumn::IoWrite,
            SortColumn::IoWrite => SortColumn::Name,
        }
    }

    /// Get previous column (for cycling with < key)
    pub fn prev(self) -> Self {
        match self {
            SortColumn::Name => SortColumn::IoWrite,
            SortColumn::Pid => SortColumn::Name,
            SortColumn::User => SortColumn::Pid,
            SortColumn::State => SortColumn::User,
            SortColumn::Cpu => SortColumn::State,
            SortColumn::Memory => SortColumn::Cpu,
            SortColumn::Pids => SortColumn::Memory,
            SortColumn::IoRead => SortColumn::Pids,
            SortColumn::IoWrite => SortColumn::IoRead,
        }
    }
}

/// Sort direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    pub fn toggle(self) -> Self {
        match self {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        }
    }
}

/// Flattened cgroup data for display (avoids cloning entire CgroupNode)
#[derive(Debug, Clone)]
pub struct FlatCgroup {
    pub name: String,
    pub cpu_percent: f32,
    pub memory_current: Option<u64>,
    pub pids_current: Option<u32>,
    pub io_read_rate: f32,
    pub io_write_rate: f32,
}

/// Flattened process data for display (avoids cloning entire ProcessNode)
#[derive(Debug, Clone)]
pub struct FlatProcess {
    pub pid: u32,
    pub name: String,
    pub user: String,
    pub state: crate::process::ProcessState,
    pub cpu_percent: f32,
    pub rss: u64,
    pub num_threads: u32,
    pub io_read_rate: f32,
    pub io_write_rate: f32,
}

/// Display item in the flattened tree view
#[derive(Debug, Clone)]
pub enum TreeItem {
    Cgroup {
        data: FlatCgroup,
        depth: usize,
        is_last: bool,
        prefix: String,
    },
    Process {
        data: FlatProcess,
        is_last: bool,
        /// Prefix from the cgroup tree (rendered in cgroup color)
        cgroup_prefix: String,
        /// Prefix from the process tree (rendered in process color)
        process_prefix: String,
    },
}

/// Previous CPU time for a process (for calculating CPU%)
#[derive(Clone)]
struct PrevCpuInfo {
    total_time: u64,  // utime + stime in clock ticks
    timestamp: Instant,
}

/// Previous CPU time for a cgroup (for calculating CPU%)
#[derive(Clone)]
struct PrevCgroupCpuInfo {
    usage_usec: u64,
    timestamp: Instant,
}

/// Previous I/O stats for a process (for calculating I/O rates)
#[derive(Clone)]
struct PrevIoInfo {
    read_bytes: u64,
    write_bytes: u64,
    timestamp: Instant,
}

/// Previous I/O stats for a cgroup (for calculating I/O rates)
#[derive(Clone)]
struct PrevCgroupIoInfo {
    read_bytes: u64,
    write_bytes: u64,
    timestamp: Instant,
}

/// Self-monitoring stats for the jam process
#[derive(Clone, Default)]
pub struct SelfStats {
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub fps: f32,
    /// Time to collect all metrics in milliseconds
    pub refresh_time_ms: f32,
}

/// Application state
pub struct App {
    /// The root cgroup node
    pub root: Option<CgroupNode>,
    /// Flattened tree items for display
    pub items: Vec<TreeItem>,
    /// Currently selected item index
    pub selected: usize,
    /// Scroll offset
    pub scroll: usize,
    /// Last refresh time
    pub last_refresh: Instant,
    /// Refresh interval
    pub refresh_interval: Duration,
    /// Should quit
    pub should_quit: bool,
    /// Error message to display
    pub error: Option<String>,
    /// Previous CPU times by PID (for CPU% calculation)
    prev_cpu: HashMap<u32, PrevCpuInfo>,
    /// Previous CPU times by cgroup path (for CPU% calculation)
    prev_cgroup_cpu: HashMap<PathBuf, PrevCgroupCpuInfo>,
    /// Previous I/O stats by PID (for I/O rate calculation)
    prev_io: HashMap<u32, PrevIoInfo>,
    /// Previous I/O stats by cgroup path (for I/O rate calculation)
    prev_cgroup_io: HashMap<PathBuf, PrevCgroupIoInfo>,
    /// Number of CPUs (for CPU% calculation)
    #[allow(dead_code)]
    num_cpus: usize,
    /// Clock ticks per second
    clock_ticks: u64,
    /// Current sort column
    pub sort_column: SortColumn,
    /// Current sort direction
    pub sort_direction: SortDirection,
    /// Self-monitoring stats for the jam process
    pub self_stats: SelfStats,
    /// Previous CPU time for self (for CPU% calculation)
    prev_self_cpu: Option<PrevCpuInfo>,
}

impl App {
    pub fn new(refresh_ms: u64) -> Self {
        // Get number of CPUs
        let num_cpus = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(1);

        Self {
            root: None,
            items: Vec::new(),
            selected: 0,
            scroll: 0,
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_millis(refresh_ms),
            should_quit: false,
            error: None,
            prev_cpu: HashMap::new(),
            prev_cgroup_cpu: HashMap::new(),
            prev_io: HashMap::new(),
            prev_cgroup_io: HashMap::new(),
            num_cpus,
            clock_ticks: 100, // Standard on Linux (sysconf(_SC_CLK_TCK))
            sort_column: SortColumn::Cpu,
            sort_direction: SortDirection::Descending,
            self_stats: SelfStats::default(),
            prev_self_cpu: None,
        }
    }

    /// Cycle to next sort column
    pub fn sort_next_column(&mut self) {
        self.sort_column = self.sort_column.next();
        self.rebuild_items();
    }

    /// Cycle to previous sort column
    pub fn sort_prev_column(&mut self) {
        self.sort_column = self.sort_column.prev();
        self.rebuild_items();
    }

    /// Toggle sort direction
    pub fn toggle_sort_direction(&mut self) {
        self.sort_direction = self.sort_direction.toggle();
        self.rebuild_items();
    }

    /// Rebuild items from current root with current sort settings
    fn rebuild_items(&mut self) {
        if let Some(ref root) = self.root {
            self.items = flatten_tree(root, self.sort_column, self.sort_direction);
        }
    }

    /// Refresh the cgroup tree data
    pub fn refresh(&mut self) -> Result<()> {
        self.error = None;
        let now = Instant::now();

        // Update self-monitoring stats
        self.update_self_stats(now);

        match parse_cgroup_tree() {
            Ok(mut root) => {
                // Calculate CPU% for all processes
                self.calculate_cpu_percent(&mut root, now);

                self.items = flatten_tree(&root, self.sort_column, self.sort_direction);
                self.root = Some(root);
                self.last_refresh = now;
            }
            Err(e) => {
                self.error = Some(format!("Failed to parse cgroup tree: {}", e));
            }
        }

        Ok(())
    }

    /// Update self-monitoring stats for the jam process
    fn update_self_stats(&mut self, now: Instant) {
        // Read /proc/self/stat directly
        if let Ok(content) = std::fs::read_to_string("/proc/self/stat") {
            if let Some((utime, stime, rss)) = parse_self_stat(&content) {
                let page_size = 4096u64;
                self.self_stats.memory_bytes = rss * page_size;

                // Calculate CPU%
                let total_time = utime + stime;
                if let Some(ref prev) = self.prev_self_cpu {
                    let elapsed = now.duration_since(prev.timestamp).as_secs_f64();
                    if elapsed > 0.0 {
                        let delta_ticks = total_time.saturating_sub(prev.total_time);
                        let delta_secs = delta_ticks as f64 / self.clock_ticks as f64;
                        self.self_stats.cpu_percent = (delta_secs / elapsed * 100.0) as f32;
                    }
                }

                self.prev_self_cpu = Some(PrevCpuInfo {
                    total_time,
                    timestamp: now,
                });
            }
        }
    }

    /// Calculate CPU% and I/O rates for all cgroups and processes in the tree
    fn calculate_cpu_percent(&mut self, node: &mut CgroupNode, now: Instant) {
        // Calculate CPU% for this cgroup
        if let Some(usage_usec) = node.metrics.cpu_usage_usec {
            if let Some(prev) = self.prev_cgroup_cpu.get(&node.path) {
                let elapsed = now.duration_since(prev.timestamp).as_secs_f64();
                if elapsed > 0.0 {
                    let delta_usec = usage_usec.saturating_sub(prev.usage_usec);
                    let delta_secs = delta_usec as f64 / 1_000_000.0;
                    // CPU% = (cpu_time_used / wall_time) * 100
                    node.metrics.cpu_percent = (delta_secs / elapsed * 100.0) as f32;
                }
            }

            // Store current CPU time for next calculation
            self.prev_cgroup_cpu.insert(node.path.clone(), PrevCgroupCpuInfo {
                usage_usec,
                timestamp: now,
            });
        }

        // Calculate I/O rates for this cgroup
        let io_read = node.metrics.io_read_bytes.unwrap_or(0);
        let io_write = node.metrics.io_write_bytes.unwrap_or(0);
        if io_read > 0 || io_write > 0 {
            if let Some(prev) = self.prev_cgroup_io.get(&node.path) {
                let elapsed = now.duration_since(prev.timestamp).as_secs_f64();
                if elapsed > 0.0 {
                    let delta_read = io_read.saturating_sub(prev.read_bytes);
                    let delta_write = io_write.saturating_sub(prev.write_bytes);
                    node.metrics.io_read_rate = (delta_read as f64 / elapsed) as f32;
                    node.metrics.io_write_rate = (delta_write as f64 / elapsed) as f32;
                }
            }

            // Store current I/O stats for next calculation
            self.prev_cgroup_io.insert(node.path.clone(), PrevCgroupIoInfo {
                read_bytes: io_read,
                write_bytes: io_write,
                timestamp: now,
            });
        }

        // Calculate for processes in this cgroup
        for proc in &mut node.processes {
            let total_time = proc.utime + proc.stime;

            if let Some(prev) = self.prev_cpu.get(&proc.pid) {
                let elapsed = now.duration_since(prev.timestamp).as_secs_f64();
                if elapsed > 0.0 {
                    let delta_ticks = total_time.saturating_sub(prev.total_time);
                    let delta_secs = delta_ticks as f64 / self.clock_ticks as f64;
                    // CPU% = (cpu_time_used / wall_time) * 100
                    // This can exceed 100% on multi-core if process uses multiple cores
                    proc.cpu_percent = (delta_secs / elapsed * 100.0) as f32;
                }
            }

            // Store current CPU time for next calculation
            self.prev_cpu.insert(proc.pid, PrevCpuInfo {
                total_time,
                timestamp: now,
            });

            // Calculate I/O rates for this process
            if proc.io_read_bytes > 0 || proc.io_write_bytes > 0 {
                if let Some(prev) = self.prev_io.get(&proc.pid) {
                    let elapsed = now.duration_since(prev.timestamp).as_secs_f64();
                    if elapsed > 0.0 {
                        let delta_read = proc.io_read_bytes.saturating_sub(prev.read_bytes);
                        let delta_write = proc.io_write_bytes.saturating_sub(prev.write_bytes);
                        proc.io_read_rate = (delta_read as f64 / elapsed) as f32;
                        proc.io_write_rate = (delta_write as f64 / elapsed) as f32;
                    }
                }

                // Store current I/O stats for next calculation
                self.prev_io.insert(proc.pid, PrevIoInfo {
                    read_bytes: proc.io_read_bytes,
                    write_bytes: proc.io_write_bytes,
                    timestamp: now,
                });
            }
        }

        // Recurse into child cgroups
        for child in &mut node.children {
            self.calculate_cpu_percent(child, now);
        }
    }

    /// Check if refresh is needed
    pub fn needs_refresh(&self) -> bool {
        self.last_refresh.elapsed() >= self.refresh_interval
    }

    /// Move selection up
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
        }
    }

    /// Page up
    pub fn page_up(&mut self, page_size: usize) {
        self.selected = self.selected.saturating_sub(page_size);
    }

    /// Page down
    pub fn page_down(&mut self, page_size: usize) {
        self.selected = (self.selected + page_size).min(self.items.len().saturating_sub(1));
    }

    /// Jump to top
    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    /// Jump to bottom
    pub fn select_last(&mut self) {
        if !self.items.is_empty() {
            self.selected = self.items.len() - 1;
        }
    }
}

/// Flatten the cgroup tree with embedded process trees into a list for display
fn flatten_tree(root: &CgroupNode, sort_column: SortColumn, sort_direction: SortDirection) -> Vec<TreeItem> {
    let mut items = Vec::new();
    flatten_cgroup_node(root, 0, true, "", &mut items, sort_column, sort_direction);
    items
}

fn flatten_cgroup_node(
    node: &CgroupNode,
    depth: usize,
    is_last: bool,
    parent_prefix: &str,
    items: &mut Vec<TreeItem>,
    sort_column: SortColumn,
    sort_direction: SortDirection,
) {
    // Build prefix for this node
    let prefix = if depth == 0 {
        String::new()
    } else {
        parent_prefix.to_string()
    };

    // Extract only the data needed for display
    // Use "localhost" for root cgroup instead of "cgroup"
    let name = if depth == 0 {
        "localhost".to_string()
    } else {
        node.name.clone()
    };
    let data = FlatCgroup {
        name,
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

    // Build prefix for children
    let child_prefix: String = if depth == 0 {
        String::new()
    } else if is_last {
        format!("{}    ", parent_prefix)
    } else {
        format!("{}│   ", parent_prefix)
    };

    // Sort child cgroups
    let mut sorted_children: Vec<_> = node.children.iter().collect();
    sort_cgroups(&mut sorted_children, sort_column, sort_direction);

    // Add child cgroups
    let child_count = sorted_children.len();
    for (i, child) in sorted_children.iter().enumerate() {
        let child_is_last = i == child_count - 1 && node.processes.is_empty();
        flatten_cgroup_node(child, depth + 1, child_is_last, &child_prefix, items, sort_column, sort_direction);
    }

    // Add processes (build process tree from references)
    if !node.processes.is_empty() {
        flatten_processes_as_tree(
            &node.processes,
            &child_prefix,
            items,
            sort_column,
            sort_direction,
        );
    }
}

/// Build process tree from references and flatten in one pass (avoids cloning)
fn flatten_processes_as_tree(
    processes: &[Process],
    cgroup_prefix: &str,
    items: &mut Vec<TreeItem>,
    sort_column: SortColumn,
    sort_direction: SortDirection,
) {
    use std::collections::{HashMap, HashSet};

    if processes.is_empty() {
        return;
    }

    // Collect all PIDs
    let pids: HashSet<u32> = processes.iter().map(|p| p.pid).collect();

    // Build parent -> children mapping
    let mut children_map: HashMap<u32, Vec<&Process>> = HashMap::new();
    let mut roots: Vec<&Process> = Vec::new();

    for proc in processes {
        if pids.contains(&proc.ppid) {
            children_map.entry(proc.ppid).or_default().push(proc);
        } else {
            roots.push(proc);
        }
    }

    // Sort roots
    sort_process_refs(&mut roots, sort_column, sort_direction);

    // Flatten recursively
    let proc_count = roots.len();
    for (i, proc) in roots.iter().enumerate() {
        let is_last = i == proc_count - 1;
        flatten_process_ref(
            proc,
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

fn flatten_process_ref(
    proc: &Process,
    is_last: bool,
    cgroup_prefix: &str,
    process_prefix: &str,
    items: &mut Vec<TreeItem>,
    children_map: &HashMap<u32, Vec<&Process>>,
    sort_column: SortColumn,
    sort_direction: SortDirection,
) {
    // Extract only the data needed for display
    let data = FlatProcess {
        pid: proc.pid,
        name: proc.name.clone(),
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

    // Get and sort children
    if let Some(children) = children_map.get(&proc.pid) {
        let mut sorted_children = children.clone();
        sort_process_refs(&mut sorted_children, sort_column, sort_direction);

        // Build process prefix for children
        let child_process_prefix: String = if is_last {
            format!("{}   ", process_prefix)
        } else {
            format!("{}│  ", process_prefix)
        };

        let child_count = sorted_children.len();
        for (i, child) in sorted_children.iter().enumerate() {
            let child_is_last = i == child_count - 1;
            flatten_process_ref(
                child,
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

/// Sort process references by the specified column
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

/// Sort cgroups by the specified column
fn sort_cgroups(cgroups: &mut [&CgroupNode], column: SortColumn, direction: SortDirection) {
    cgroups.sort_by(|a, b| {
        let cmp = match column {
            SortColumn::Name => a.name.cmp(&b.name),
            SortColumn::Cpu => a.metrics.cpu_percent.partial_cmp(&b.metrics.cpu_percent).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Memory => a.metrics.memory_current.cmp(&b.metrics.memory_current),
            SortColumn::Pids => a.metrics.pids_current.cmp(&b.metrics.pids_current),
            SortColumn::IoRead => a.metrics.io_read_rate.partial_cmp(&b.metrics.io_read_rate).unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::IoWrite => a.metrics.io_write_rate.partial_cmp(&b.metrics.io_write_rate).unwrap_or(std::cmp::Ordering::Equal),
            // For columns that don't apply to cgroups, sort by name
            SortColumn::Pid | SortColumn::User | SortColumn::State => a.name.cmp(&b.name),
        };
        match direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        }
    });
}

/// Parse /proc/self/stat to extract utime, stime, and rss
fn parse_self_stat(content: &str) -> Option<(u64, u64, u64)> {
    // Find comm between parentheses (handles names with spaces/parens)
    let end = content.rfind(')')?;
    let rest = &content[end + 2..]; // Skip ") "
    let fields: Vec<&str> = rest.split_whitespace().collect();

    if fields.len() < 22 {
        return None;
    }

    // Fields after comm: state(0), ppid(1), ..., utime(11), stime(12), ..., rss(21)
    let utime: u64 = fields[11].parse().ok()?;
    let stime: u64 = fields[12].parse().ok()?;
    let rss: u64 = fields[21].parse().ok()?;

    Some((utime, stime, rss))
}
