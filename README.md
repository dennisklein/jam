# JAM - Job Analysis and Monitoring

A terminal-based tool for visualizing and monitoring cgroup v2 resource hierarchies and process trees on Linux systems.

## Features

- **Real-time monitoring** of CPU usage, memory consumption, and I/O operations
- **Interactive tree view** displaying cgroup and process hierarchies
- **Multiple sort options** (CPU, memory, I/O, name)
- **Self-monitoring** displays JAM's own resource usage
- **Vim-style navigation** with familiar keybindings

## Requirements

- Linux with cgroup v2 enabled
- Rust 1.70+ (for building)
- Read access to `/sys/fs/cgroup` and `/proc`

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

```bash
# Interactive TUI mode
jam

# Custom refresh interval (in milliseconds)
jam --refresh 500
```

### Command-Line Options

| Option | Short | Default | Description |
|--------|-------|---------|-------------|
| `--refresh` | `-r` | 1000 | Refresh interval in milliseconds |

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
| CPU% | CPU utilization percentage |
| Memory | Memory usage (RSS for processes, cgroup memory for cgroups) |
| Tasks | Number of processes in cgroup |
| IO-Read/s | I/O read rate |
| IO-Write/s | I/O write rate |
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
└── ui/              # Terminal UI rendering
    └── tree.rs      # Tree view with ratatui
```

## How It Works

1. **Data Collection**: Recursively reads `/sys/fs/cgroup` to build the cgroup hierarchy, parsing controller files (`cpu.stat`, `memory.current`, `io.stat`, `pids.current`)

2. **Process Mapping**: For each cgroup, reads process information from `/proc/[pid]/` including CPU times, memory, and I/O statistics

3. **Rate Calculation**: Computes CPU% and I/O rates by comparing current values with previous samples

4. **Tree Rendering**: Flattens the hierarchical structure into a scrollable list with tree-drawing characters for visualization

## Dependencies

- [ratatui](https://crates.io/crates/ratatui) - Terminal UI framework
- [crossterm](https://crates.io/crates/crossterm) - Terminal manipulation
- [clap](https://crates.io/crates/clap) - Command-line argument parsing
- [anyhow](https://crates.io/crates/anyhow) - Error handling
- [serde](https://crates.io/crates/serde) / [serde_json](https://crates.io/crates/serde_json) - Serialization

## License

This project is licensed under the GNU Lesser General Public License v3.0 or later - see the [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.
