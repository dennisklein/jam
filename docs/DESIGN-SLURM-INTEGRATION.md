# Slurm Integration Design for JAM

## Overview

Monitor processes and cgroups across all nodes of a Slurm job with a unified TUI view.

```bash
jam --jobid 12345              # Monitor job across all allocated nodes
jam --jobid 12345 --refresh 500   # Custom refresh rate
```

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          Login/Submit Node                              │
│  jam --jobid 12345                                                      │
│  1. Query Slurm for job info (nodes, cgroup path)                       │
│  2. Launch `srun --jobid 12345 --overlap jam --collector` per node      │
│  3. Receive JSON metrics via stdout                                     │
│  4. Aggregate and display in TUI                                        │
└─────────────────────────────────────────────────────────────────────────┘
         │ srun --overlap        │ srun --overlap        │ srun --overlap
         ▼                       ▼                       ▼
┌─────────────┐          ┌─────────────┐          ┌─────────────┐
│   Node 1    │          │   Node 2    │          │   Node 3    │
│ jam         │          │ jam         │          │ jam         │
│ --collector │          │ --collector │          │ --collector │
│ Reads:      │          │ Reads:      │          │ Reads:      │
│ /sys/fs/    │          │ /sys/fs/    │          │ /sys/fs/    │
│ cgroup/...  │          │ cgroup/...  │          │ cgroup/...  │
│ stdout:JSON │          │ stdout:JSON │          │ stdout:JSON │
└─────────────┘          └─────────────┘          └─────────────┘
```

### Components

**Coordinator Mode** (default with `--jobid`): Queries Slurm, spawns collectors via `srun --overlap`, aggregates metrics, renders TUI.

**Collector Mode** (`--collector`): Headless, reads local cgroup/process metrics, outputs NDJSON snapshots with raw cumulative counters. Discovers cgroup root via `/proc/self/cgroup`.

## CLI Changes

```rust
#[derive(Parser)]
struct Args {
    #[arg(long)]
    jobid: Option<u32>,
    #[arg(long, default_value = "jam")]
    jam_binary: String,            // Path to jam on worker nodes
    #[arg(long, hide = true)]
    collector: bool,               // Internal: collector mode
    #[arg(long, hide = true)]
    node_name: Option<String>,     // Internal: passed by coordinator
    #[arg(short, long, default_value = "1000")]
    refresh: u64,
}

fn main() {
    let args = Args::parse();
    if args.collector { run_collector_mode(args); }
    else if let Some(jobid) = args.jobid { run_coordinator_mode(jobid, args); }
    else { run_local_mode(args); }
}
```

## Slurm Integration

**Decision: CLI-based** (scontrol, srun) - simpler, portable, no libslurm dependency.

```rust
fn get_job_info(jobid: u32) -> Result<JobInfo> {
    let output = Command::new("scontrol")
        .args(["show", "job", &jobid.to_string(), "--oneliner"])
        .output()?;
    parse_scontrol_output(&output.stdout)
}

fn expand_nodelist(nodelist: &str) -> Result<Vec<String>> {
    let output = Command::new("scontrol")
        .args(["show", "hostnames", nodelist])
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).lines().map(String::from).collect())
}
```

### Launching Collectors

**Decision: One srun per node** for clean stdout separation and independent failure handling.

```rust
async fn spawn_collectors(jobid: u32, nodes: &[String], jam_binary: &str, refresh_ms: u64) -> Result<Vec<CollectorHandle>> {
    let mut handles = Vec::new();
    for node in nodes {
        let child = Command::new("srun")
            .args(["--jobid", &jobid.to_string(), "--overlap", "--nodelist", node,
                   "--ntasks=1", "--", jam_binary, "--collector", "--node-name", node,
                   "--refresh", &refresh_ms.to_string()])
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        handles.push(CollectorHandle { node: node.clone(), process: child });
    }
    Ok(handles)
}
```

Key srun flags: `--overlap` (attach without consuming resources), `--jobid`, `--nodelist`, `--ntasks=1`.

## Serialization Protocol

**Format:** Newline-delimited JSON (NDJSON)

```rust
#[derive(Serialize, Deserialize)]
pub struct NodeSnapshot {
    #[serde(rename = "type")]
    pub msg_type: String,           // "snapshot"
    pub node: String,
    pub timestamp: u64,             // Unix ms (collector's clock)
    pub cgroup_tree: CgroupNodeRaw,
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_reason: Option<String>, // "completed" | "sigterm" | "error"
}

#[derive(Serialize, Deserialize)]
pub struct CgroupNodeRaw {
    pub path: PathBuf,
    pub name: String,
    pub children: Vec<CgroupNodeRaw>,
    pub processes: Vec<ProcessRaw>,
    pub depth: usize,
    // Raw cumulative counters (Option<> - controllers may be disabled)
    pub memory_current: Option<u64>,
    pub memory_max: Option<u64>,
    pub cpu_usage_usec: Option<u64>,
    pub pids_current: Option<u32>,
    pub io_read_bytes: Option<u64>,
    pub io_write_bytes: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub struct ProcessRaw {
    pub pid: u32,
    pub ppid: u32,
    pub starttime: u64,  // /proc/[pid]/stat field 22, for PID reuse detection
    pub name: String,
    pub user: String,
    pub state: ProcessState,
    pub num_threads: u32,
    pub rss: u64,
    pub vsize: u64,
    pub utime: u64,
    pub stime: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
}
```

**Collector sends raw counters with timestamps; coordinator computes rates.**

## Aggregation Strategy

```rust
struct CoordinatorState {
    node_snapshots: HashMap<String, ReceivedSnapshot>,
    prev_snapshots: HashMap<String, NodeSnapshot>,  // For rate calculation
    merged_items: Vec<TreeItem>,
    node_status: HashMap<String, NodeStatus>,
    collectors: HashMap<String, CollectorHandle>,
}

struct ReceivedSnapshot {
    snapshot: NodeSnapshot,
    received_at: Instant,  // Local time for diagnostics
}

enum NodeStatus { Connected, Disconnected { reason: String }, Disabled }
```

### Rate Calculation (Coordinator-Side)

```rust
fn process_snapshot(&mut self, snapshot: NodeSnapshot) -> CgroupNode {
    let prev = self.prev_snapshots.get(&snapshot.node);
    let tree_with_rates = if let Some(prev) = prev {
        let elapsed_secs = (snapshot.timestamp - prev.timestamp) as f64 / 1000.0;
        compute_rates(&snapshot.cgroup_tree, &prev.cgroup_tree, elapsed_secs)
    } else {
        convert_to_display_tree(&snapshot.cgroup_tree)  // First snapshot, no rates
    };
    self.prev_snapshots.insert(snapshot.node.clone(), snapshot);
    tree_with_rates
}
```

- Matches cgroups by path, processes by (pid, starttime) tuple to handle PID reuse
- Transport latency doesn't affect rate accuracy (uses collector timestamps)

### Tree Merging

Shared hierarchy (uid/job/step) shown once; node-specific subtrees merged with NODE attribution:

```
NODE     NAME                       CPU%   MEM      PIDS
─────────────────────────────────────────────────────────
         uid_1000/                    --     --       --
         └── job_12345/               --     --       --
             └── step_0/              --     --       --
node001          ├── task_0/        25.2   4.1G       12
node001          │   └── python     20.1   3.8G        8
node002          └── task_0/        28.7   4.3G       12
node002              └── python     22.3   4.0G        8
```

## Cgroup Discovery

Collectors discover cgroup root via `/proc/self/cgroup`:

```rust
fn discover_job_cgroup_root() -> Result<PathBuf> {
    let cgroup_info = std::fs::read_to_string("/proc/self/cgroup")?;
    let cgroup_path = if let Some(line) = cgroup_info.lines().find(|l| l.starts_with("0::")) {
        line.strip_prefix("0::").unwrap()  // v2: "0::/system.slice/.../job_123/..."
    } else {
        cgroup_info.lines()
            .find(|l| l.contains(":memory:") || l.contains(":cpu"))
            .and_then(|l| l.split(':').nth(2))
            .ok_or_else(|| anyhow!("No suitable cgroup controller"))?  // v1
    };
    let job_root = extract_job_level_path(cgroup_path)?;  // Strip step_*/task_*
    let mount_point = find_cgroup_mount_point()?;
    Ok(mount_point.join(job_root.trim_start_matches('/')))
}
```

Typical Slurm cgroup layouts:
- `/sys/fs/cgroup/system.slice/slurmstepd.scope/job_<id>/step_<id>/task_<id>/`
- `/sys/fs/cgroup/slurm/uid_<uid>/job_<id>/step_<id>/task_<id>/`

### Scan Consistency

Cgroup scanning is not atomic. Handle race conditions gracefully:
- Process exits during scan: skip (ENOENT/ESRCH)
- Process moves cgroups: accept as point-in-time approximation
- PID reuse: detect via (pid, starttime) matching

## TUI Changes

### NODE Column

| Column | Width | Content |
|--------|-------|---------|
| NODE | 12 | Hostname (truncated) |
| NAME | dynamic | Cgroup/process name with tree |
| ... | ... | (existing columns) |

```rust
pub enum TreeItem {
    Cgroup { data: FlatCgroup, depth: usize, node: Option<String>, is_last: bool, prefix: String },
    Process { data: FlatProcess, node: String, is_last: bool, cgroup_prefix: String, process_prefix: String },
}
```

### Status Bar

```
JAM v0.1.0 │ Job 12345 │ Nodes: 3/3 │ Refresh: 1.0Hz │ ...
```

### Interactive Node Control

| Key | Action |
|-----|--------|
| `n` | Open node list popup |
| `d` | Toggle disable/enable for selected node |

```
┌─ Nodes ─────────────────────┐
│ [x] node001  ● connected    │
│ [x] node002  ● connected    │
│ [ ] node003  ○ disabled     │
└─────────────────────────────┘
```

## Error Handling

- **Collector exit**: Coordinator detects via EOF, marks node disconnected
- **Permission errors**: Return partial metrics with None for inaccessible values
- **Restart**: Attempt reconnect with backoff; after N failures, show permanent error

## Signal Handling

**Collector**: SIGTERM → set shutdown flag, send final snapshot (with `final_reason`), exit 0

**Coordinator**: SIGINT/SIGTERM → kill all srun children, wait up to 5s, exit

### Job Completion Detection

Collector exits gracefully when only monitoring infrastructure remains:

```rust
fn is_job_workload_complete(tree: &CgroupNodeRaw) -> bool {
    collect_all_pids(tree).iter().all(|&pid| pid == std::process::id() || is_slurm_infrastructure(pid))
}
```

## Job State Handling

```rust
fn run_coordinator_mode(jobid: u32, args: Args) -> Result<()> {
    match check_job_state(jobid)? {
        JobState::Running { nodes } => start_monitoring(jobid, &nodes, &args),
        JobState::Pending => bail!("Job {} is pending, not yet running.", jobid),
        JobState::Completed { exit_code } => bail!("Job {} already completed (exit {}).", jobid, exit_code),
        _ => bail!("Job {} not in runnable state.", jobid),
    }
}
```

## Scalability

**Target: 1-100 nodes initially.**

Limits:
- MaxStepCount: ~40k steps/job (allows ~400 restarts for 100-node job)
- FDs: 3 per collector (300 for 100 nodes, well within limits)
- Memory: ~20MB for 100 nodes (current + previous snapshots)

Warning for >100 nodes; future optimizations: `--nodes` subset selection, hierarchical aggregation, node sampling.

## Dependencies

```toml
tokio = { version = "1", features = ["full"] }
signal-hook = "0.3"
# serde, serde_json already present
```

System: Slurm CLI tools (scontrol, srun) in PATH.

## Security

- Slurm enforces job ownership (only owner/admin can use `--overlap`)
- Collectors read only their job's cgroup subtree
- No elevated privileges required

## Implementation Phases

1. **Collector Mode**: `--collector` flag, cgroup discovery, raw JSON output
2. **Slurm Query**: scontrol parsing, nodelist expansion
3. **Coordinator Mode**: srun spawning, async stdout aggregation
4. **TUI Integration**: NODE column, tree merging, status bar
5. **Robustness**: Reconnection, graceful degradation, health monitoring

## Future Enhancements

- libslurm native API
- GPU metrics (nvidia-smi/ROCm)
- Historical data recording
- Job comparison
- Alerts
- Web UI
