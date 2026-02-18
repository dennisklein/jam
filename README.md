> ⚠️ **CAUTION: Vibe-coded proof-of-concept! Use at your own risk and for inspiration only!**

# JAM - Job Analysis and Monitoring

A terminal-based tool for visualizing and monitoring cgroup v2 resource hierarchies and process trees on Linux systems, with native Slurm multi-node support.

## Features

- **Real-time monitoring** of CPU usage, memory consumption, and I/O operations
- **Interactive tree view** displaying cgroup and process hierarchies
- **Slurm integration** monitors all nodes of a Slurm job from a single TUI
- **Multiple sort options** (CPU, memory, I/O, name)
- **Self-monitoring** displays JAM's own resource usage
- **Vim-style navigation** with familiar keybindings

## Requirements

- Linux with cgroup v2 enabled
- Rust 1.70+ (for building)
- Read access to `/sys/fs/cgroup` and `/proc`
- For Slurm mode: SSH access to compute nodes, `scontrol` on the coordinator

## Installation

```bash
# Clone the repository
git clone <repository-url>
cd jam

# Build release binary
cargo build --release

# Binary will be at target/release/jam
```

## Usage

### Local Mode

Monitor the local cgroup tree:

```bash
# Interactive TUI mode
jam

# Custom refresh interval (in milliseconds)
jam --refresh 500
```

### Slurm Mode

Monitor all nodes of a running Slurm job:

```bash
# Monitor job by ID (queries scontrol for allocated nodes)
jam --jobid 12345

# Use a custom path to the jam binary on worker nodes
jam --jobid 12345 --jam-binary /opt/jam/bin/jam
```

The coordinator spawns collector processes on each allocated node via SSH. Collectors discover the job's cgroup hierarchy and stream metric snapshots back to the coordinator, which aggregates and renders them in a unified TUI.

### Command-Line Options

| Option | Short | Default | Description |
|--------|-------|---------|-------------|
| `--refresh` | `-r` | 1000 | Refresh interval in milliseconds |
| `--jobid` | | | Slurm job ID to monitor across all allocated nodes |
| `--jam-binary` | | `jam` | Path to jam binary on worker nodes (for `--jobid` mode) |

### Keyboard Controls

| Key | Action |
|-----|--------|
| `q` / `Ctrl+C` | Quit |
| `j` / `↓` | Navigate down |
| `k` / `↑` | Navigate up |
| `d` / `Ctrl+D` | Page down |
| `u` / `Ctrl+U` | Page up |
| `g` / `Home` | Jump to top |
| `G` / `End` | Jump to bottom |
| `r` | Manual refresh |
| `>` / `.` | Sort by next column |
| `<` / `,` | Sort by previous column |
| `I` | Reverse sort direction |

## Display Columns

| Column | Description |
|--------|-------------|
| PID | Process ID (empty for cgroups) |
| User | Process owner username |
| State | Process state (R=running, S=sleeping, D=disk sleep, etc.) |
| CPU% | CPU utilization percentage (`-` when no baseline yet) |
| Memory | Memory usage (RSS for processes, cgroup memory for cgroups) |
| Tasks | Number of processes in cgroup |
| IO-Read/s | I/O read rate (`-` when no baseline or near-zero) |
| IO-Write/s | I/O write rate (`-` when no baseline or near-zero) |
| Name | Process command or cgroup name |

## Architecture

```
src/
├── main.rs          # Entry point, CLI parsing, event loop
├── app.rs           # Core application state and logic
├── cgroup/          # cgroup v2 data collection
│   ├── types.rs     # CgroupNode, CgroupMetrics
│   └── parser.rs    # /sys/fs/cgroup parsing
├── process/         # Process information collection
│   ├── types.rs     # Process, ProcessState
│   └── parser.rs    # /proc filesystem parsing
├── slurm/           # Slurm multi-node monitoring
│   ├── collector.rs # Node-local metric collector (NDJSON output)
│   ├── coordinator.rs # Aggregates collectors into unified TUI
│   ├── slurm.rs     # scontrol parsing and job info
│   └── types.rs     # Snapshot protocol types
└── ui/              # Terminal UI rendering
    └── tree.rs      # Tree view with ratatui
```

### Slurm Coordinator/Collector Pattern

```
┌────────────────┐         SSH + jam --collector
│  Coordinator   │──────────────────────────────►┌──────────┐
│  (jam --jobid) │          NDJSON snapshots      │ node c1  │
│                │◄──────────────────────────────┤ collector │
│  Aggregates    │                                └──────────┘
│  metrics from  │         SSH + jam --collector
│  all nodes     │──────────────────────────────►┌──────────┐
│                │          NDJSON snapshots      │ node c2  │
│  Renders       │◄──────────────────────────────┤ collector │
│  unified TUI   │                                └──────────┘
└────────────────┘
```

Each collector:
1. Discovers the job's cgroup via `/proc/self/cgroup`
2. Reads cumulative counters from cgroup controllers and `/proc`
3. Streams timestamped NDJSON snapshots to stdout

The coordinator:
1. Queries `scontrol` for allocated nodes
2. Spawns collectors via `ssh <node> jam --collector`
3. Computes CPU% and I/O rates from consecutive snapshots
4. Merges per-node trees into a single display

## How It Works

1. **Data Collection**: Recursively reads `/sys/fs/cgroup` to build the cgroup hierarchy, parsing controller files (`cpu.stat`, `memory.current`, `io.stat`, `pids.current`)

2. **Process Mapping**: For each cgroup, reads process information from `/proc/[pid]/` including CPU times, memory, and I/O statistics

3. **Rate Calculation**: Computes CPU% and I/O rates by comparing current values with previous samples. Timestamps are captured before metric parsing to ensure accurate elapsed time computation. First-sample rates show `-` (pending) until a baseline is established.

4. **Tree Rendering**: Flattens the hierarchical structure into a scrollable list with tree-drawing characters for visualization

## Dependencies

- [ratatui](https://crates.io/crates/ratatui) - Terminal UI framework
- [crossterm](https://crates.io/crates/crossterm) - Terminal manipulation
- [clap](https://crates.io/crates/clap) - Command-line argument parsing
- [anyhow](https://crates.io/crates/anyhow) - Error handling
- [serde](https://crates.io/crates/serde) / [serde_json](https://crates.io/crates/serde_json) - Serialization
- [tokio](https://crates.io/crates/tokio) - Async runtime (Slurm coordinator)
- [hostname](https://crates.io/crates/hostname) - Node name detection (collector)

## Development

### Building

```bash
cargo build          # Debug build
cargo build --release # Optimized release build
cargo test           # Run unit tests
```

### Integration Tests

A Docker-based Slurm cluster is provided for testing multi-node monitoring. See [`tests/slurm-cluster/README.md`](tests/slurm-cluster/README.md) for setup instructions.

Quick start:

```bash
cd tests/slurm-cluster
make up        # Start the 3-container cluster (slurmctld + 2 workers)
make deploy    # Build jam and install to all containers
make test-jam  # Run the integration test suite
make down      # Stop the cluster
```

## License

This project is licensed under the GNU Lesser General Public License v3.0 or later - see the [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.
