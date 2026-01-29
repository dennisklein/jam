# Slurm Test Cluster

A Docker Compose-based Slurm cluster for testing jam's multi-node monitoring capabilities with cgroup v2 support.

## Overview

This test environment emulates a minimal Slurm HPC cluster:

```
┌─────────────────────────────────────────────────────────────┐
│                Docker Network (slurm-net)                   │
│              (subnet assigned by Docker)                    │
│                                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐         │
│  │  slurmctld  │  │     c1      │  │     c2      │         │
│  │             │  │             │  │             │         │
│  │ Controller  │  │   slurmd    │  │   slurmd    │         │
│  │             │  │  (worker)   │  │  (worker)   │         │
│  └─────────────┘  └─────────────┘  └─────────────┘         │
│         ▲               ▲               ▲                  │
│         └───────────────┴───────────────┘                  │
│              Docker DNS resolves hostnames                 │
└─────────────────────────────────────────────────────────────┘
```

## Prerequisites

- Docker Engine 28.0+ (for `--security-opt writable-cgroups`)
- Docker Compose v2
- Host running cgroup v2 (unified hierarchy)
- Network access to GSI package mirror (for Slurm RPMs)

### Verify prerequisites

```bash
# Check Docker version (need 28.0+)
docker --version

# Check cgroup version (should show "Cgroup Version: 2")
docker info | grep "Cgroup Version"

# Test writable-cgroups support
docker run --rm --security-opt writable-cgroups=true alpine cat /proc/mounts | grep cgroup
```

## Quick Start

```bash
cd tests/slurm-cluster

# Build and start the cluster
make up

# Check cluster status
make status

# Run a test job
make test

# Get a shell on the controller
make shell

# View logs
make logs

# Stop the cluster
make down

# Clean up everything (volumes, images)
make clean
```

## Architecture

### Components

| Container | Hostname | Role |
|-----------|----------|------|
| `slurmctld` | slurmctld | Controller daemon, job scheduler |
| `c1` | c1 | Compute node (slurmd) |
| `c2` | c2 | Compute node (slurmd) |

IP addresses are assigned dynamically by Docker. Containers communicate via hostnames resolved by Docker's embedded DNS.

### Slurm Version

Uses Slurm 25.11.2 from GSI's package repository:
- `http://cluster-mirror.hpc.gsi.de/packages/slurm-25.11/el9/`

### Cgroup v2 Support

Each container has:
- **Isolated cgroup namespace** (`cgroupns: private` is default on cgroup v2)
- **Writable cgroup filesystem** (`--security-opt writable-cgroups=true`)
- **SELinux disabled for container** (`--security-opt label=disable`)

Job cgroups are created with this hierarchy:
```
/sys/fs/cgroup/system.slice/slurmstepd.scope/
├── slurmd/                      # slurmd daemon
├── system/                      # system processes
└── job_<ID>/                    # per-job cgroup
    └── step_<N>/                # per-step cgroup (e.g., step_0)
        ├── slurm/               # slurmstepd helper processes
        └── user/                # user processes
            ├── task_0/          # task 0 processes
            └── task_special/    # special task processes
```

## How This Works Without Privileged Containers

Most Slurm Docker setups use `privileged: true` because slurmd needs to create and manage cgroups for job resource accounting. Privileged mode grants all capabilities and disables security features, which is undesirable for testing.

This setup achieves writable cgroups **without privileged mode** using Docker 28.0+'s `writable-cgroups` security option.

### The Problem

By default, Docker mounts `/sys/fs/cgroup` as **read-only** inside containers, even with a private cgroup namespace:

```bash
# Default behavior - cgroup is read-only
$ docker run --rm alpine mount | grep cgroup
cgroup2 on /sys/fs/cgroup type cgroup2 (ro,nosuid,nodev,noexec,relatime)
```

Slurm's slurmd daemon needs to **write** to cgroups to:
- Create per-job cgroup hierarchies
- Set resource limits (CPU, memory)
- Track processes for job accounting

### The Solution

Docker 28.0 introduced `--security-opt writable-cgroups=true` ([moby/moby#48828](https://github.com/moby/moby/pull/48828)), which mounts the container's cgroup namespace as read-write:

```bash
# With writable-cgroups - cgroup is read-write
$ docker run --rm --security-opt writable-cgroups=true alpine mount | grep cgroup
cgroup2 on /sys/fs/cgroup type cgroup2 (rw,nosuid,nodev,noexec,relatime)
```

### Security Comparison

| Aspect | `privileged: true` | `writable-cgroups=true` |
|--------|-------------------|------------------------|
| All capabilities | Yes | No (default set only) |
| Seccomp filtering | Disabled | Active |
| AppArmor/SELinux | Disabled | Active (we disable SELinux separately) |
| Device access | Full `/dev` | Restricted |
| Cgroup writes | Yes | Yes |
| Host namespace | Shared | Isolated |

### Why We Also Need `label=disable`

On SELinux-enabled hosts (RHEL, Fedora, Rocky), even with `writable-cgroups=true`, SELinux policies block cgroup writes:

```bash
# Fails on SELinux systems
$ docker run --rm --security-opt writable-cgroups=true alpine \
    mkdir /sys/fs/cgroup/test
mkdir: can't create directory '/sys/fs/cgroup/test': Permission denied

# Works with SELinux disabled for container
$ docker run --rm --security-opt writable-cgroups=true \
    --security-opt label=disable alpine \
    mkdir /sys/fs/cgroup/test && echo "success"
success
```

For a local test cluster, disabling SELinux confinement is acceptable. In production, you would create a custom SELinux policy module.

### Cgroup Namespace Isolation

With `cgroupns: private` (the default on cgroup v2), each container sees only its own cgroup subtree rooted at `/sys/fs/cgroup`:

```
Host view:                              Container view (c1):
/sys/fs/cgroup/                         /sys/fs/cgroup/  (container's root)
├── system.slice/                       ├── cgroup.controllers
│   ├── docker-<c1>.scope/  ──────────► ├── system.slice/
│   │   └── system.slice/               │   └── slurmstepd.scope/
│   │       └── slurmstepd.scope/       │       ├── slurmd/
│   │           ├── slurmd/             │       ├── system/
│   │           └── job_1/              │       └── job_1/
│   │               └── step_0/         │           └── step_0/
│   │                   ├── slurm/      │               ├── slurm/
│   │                   └── user/       │               └── user/
│   │                       └── task_0/ │                   └── task_0/
│   └── docker-<c2>.scope/              └── ...
└── ...
```

This means:
- Containers cannot see or affect each other's cgroups
- Each slurmd manages its own isolated cgroup tree
- Job cgroups are created within the container's namespace

### Summary

The combination of:
1. **`writable-cgroups=true`** - Makes `/sys/fs/cgroup` writable
2. **`label=disable`** - Bypasses SELinux restrictions
3. **`cgroupns: private`** (default) - Isolates cgroup namespace
4. **`IgnoreSystemd=yes`** in cgroup.conf - Slurm manages cgroups directly

...allows Slurm to function correctly without the security risks of privileged containers.

## Configuration

### slurm.conf

Key settings:
```
ClusterName=jam-test
ProctrackType=proctrack/cgroup      # Track processes via cgroups
TaskPlugin=task/cgroup,task/affinity
SelectType=select/cons_tres
```

### cgroup.conf

```
CgroupPlugin=cgroup/v2
IgnoreSystemd=yes        # Required: no systemd in containers
EnableControllers=yes    # Enable all available controllers
ConstrainCores=yes
ConstrainRAMSpace=yes
ConstrainSwapSpace=yes
```

The `IgnoreSystemd=yes` setting is critical - it tells Slurm to manage the cgroup hierarchy directly instead of via dbus/systemd, which isn't running in the containers.

## Docker Configuration Details

### Why these security options?

| Option | Purpose |
|--------|---------|
| `writable-cgroups=true` | Allows slurmd to create job cgroups under `/sys/fs/cgroup` |
| `label=disable` | Disables SELinux confinement (SELinux blocks cgroup writes) |

### Why `--network=host` for build?

The Dockerfile fetches packages from external repositories. Docker's default bridge network may have DNS resolution issues depending on your environment. Using `--network=host` during build inherits the host's DNS configuration.

### Volume mounts

| Volume | Purpose | Mount flags |
|--------|---------|-------------|
| `shared-data` | Shared `/data` directory for jobs | `:z` (SELinux shared label) |
| `./etc/slurm` | Slurm configuration files | `:ro,z` (read-only, SELinux) |

## Usage Examples

### Run interactive job

```bash
docker compose exec slurmctld srun -N2 --pty bash
```

### Submit batch job

```bash
docker compose exec slurmctld sbatch --wrap="hostname && sleep 10" -N1
```

### Check job queue

```bash
docker compose exec slurmctld squeue
```

### View job cgroups

```bash
# While a job is running:
docker compose exec c1 find /sys/fs/cgroup -name "job_*" -type d
```

### Read cgroup metrics

```bash
# Memory usage of a running job (at job level):
docker compose exec c1 cat /sys/fs/cgroup/system.slice/slurmstepd.scope/job_1/memory.current

# Memory usage of user task 0:
docker compose exec c1 cat /sys/fs/cgroup/system.slice/slurmstepd.scope/job_1/step_0/user/task_0/memory.current

# List all processes in a job step:
docker compose exec c1 cat /sys/fs/cgroup/system.slice/slurmstepd.scope/job_1/step_0/user/cgroup.procs
```

## Testing jam

Once the cluster is running, you can test jam's Slurm integration:

```bash
# From the slurmctld container, run jam with a job ID
docker compose exec slurmctld jam --jobid <JOB_ID>

# Or mount the jam binary into the container (add to docker-compose.yml):
# volumes:
#   - ../../target/release/jam:/usr/local/bin/jam:ro
```

## Troubleshooting

### "Keyfile must be at least 32 bytes"

The munge key is too short. Rebuild the image:
```bash
make clean && make build
```

### "cannot connect to dbus system daemon"

Missing `IgnoreSystemd=yes` in cgroup.conf. The containers don't run systemd.

### "Permission denied" creating cgroups

1. Check Docker version is 28.0+
2. Verify `writable-cgroups=true` is set
3. On SELinux systems, ensure `label=disable` is set

### Nodes stuck in "DOWN" state

```bash
# Check slurmd logs
docker compose logs c1

# Manually set nodes to idle
docker compose exec slurmctld scontrol update nodename=c[1-2] state=idle
```

### DNS resolution failures during build

Use host networking for build:
```bash
docker build --network=host -t jam-slurm-node .
```

### Health check failures

The slurmctld health check uses `scontrol ping`. If it fails:
```bash
# Check if slurmctld is actually running
docker compose exec slurmctld pidof slurmctld

# Check logs
docker compose logs slurmctld
```

## Customization

### Adding more compute nodes

Add to `docker-compose.yml`:
```yaml
  c3:
    image: jam-slurm-node
    hostname: c3
    container_name: c3
    security_opt:
      - writable-cgroups=true
      - label=disable
    volumes:
      - shared-data:/data:z
      - ./etc/slurm:/etc/slurm:ro,z
    networks:
      - slurm-net
    depends_on:
      slurmctld:
        condition: service_healthy
    command: ["/usr/sbin/slurmd", "-D", "-vv"]
```

And update `slurm.conf`:
```
NodeName=c3 CPUs=2 State=UNKNOWN
PartitionName=debug Nodes=c1,c2,c3 Default=YES MaxTime=INFINITE State=UP
```

### Changing Slurm version

The GSI repository provides multiple Slurm versions. To use a different version, update the Dockerfile to point to a different repository URL.

## Files

```
tests/slurm-cluster/
├── docker-compose.yml    # Container orchestration
├── Dockerfile            # Image build (Rocky 9 + Slurm)
├── Makefile              # Convenience commands
├── README.md             # This file
├── etc/
│   └── slurm/
│       ├── slurm.conf    # Main Slurm configuration
│       └── cgroup.conf   # Cgroup plugin configuration
└── scripts/
    └── entrypoint.sh     # Container entrypoint (starts munged)
```

## References

- [Slurm cgroup.conf documentation](https://slurm.schedmd.com/cgroup.conf.html)
- [Slurm cgroup v2 plugin](https://slurm.schedmd.com/cgroup_v2.html)
- [Docker writable-cgroups PR](https://github.com/moby/moby/pull/48828)
- [systemd cgroup delegation](https://systemd.io/CGROUP_DELEGATION/)
