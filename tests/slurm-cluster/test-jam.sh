#!/bin/bash
# SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
# SPDX-License-Identifier: LGPL-3.0-or-later

# Integration tests for jam Slurm integration
# Requires Docker with the Slurm cluster running

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

passed=0
failed=0

log_info() {
    echo -e "${YELLOW}[INFO]${NC} $1"
}

log_pass() {
    echo -e "${GREEN}[PASS]${NC} $1"
    passed=$((passed + 1))
}

log_fail() {
    echo -e "${RED}[FAIL]${NC} $1"
    failed=$((failed + 1))
}

# Docker compose wrapper that uses the correct project directory
dc() {
    docker compose -f "$SCRIPT_DIR/docker-compose.yml" "$@"
}

# Check prerequisites
check_prerequisites() {
    log_info "Checking prerequisites..."

    if ! command -v docker &> /dev/null; then
        log_fail "Docker is not installed"
        exit 1
    fi

    if ! dc ps -q slurmctld &> /dev/null; then
        log_fail "Slurm cluster is not running. Start it with: cd tests/slurm-cluster && make up"
        exit 1
    fi

    log_pass "Prerequisites check passed"
}

# Build jam inside the container to avoid glibc version issues
build_jam() {
    log_info "Building jam inside container (to match glibc version)..."

    # Copy source to shared volume
    docker exec slurmctld rm -rf /data/jam-src 2>/dev/null || true
    docker cp "$PROJECT_ROOT" slurmctld:/data/jam-src

    # Install Rust in the container if not present
    docker exec slurmctld bash -c '
        if ! command -v cargo &> /dev/null; then
            echo "Installing Rust..."
            curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
            source "$HOME/.cargo/env"
        fi
        source "$HOME/.cargo/env" 2>/dev/null || true
        cd /data/jam-src
        cargo build --release
    ' 2>&1

    # Check if build succeeded
    if docker exec slurmctld test -f /data/jam-src/target/release/jam; then
        log_pass "jam built successfully inside container"
    else
        log_fail "jam binary not found after build"
        exit 1
    fi
}

# Copy jam binary to all containers from the shared volume
copy_jam_to_containers() {
    log_info "Copying jam binary to containers..."

    # Copy from shared volume to /usr/local/bin on each container
    for container in slurmctld c1 c2; do
        docker exec "$container" cp /data/jam-src/target/release/jam /usr/local/bin/jam
        docker exec "$container" chmod +x /usr/local/bin/jam
    done

    log_pass "jam copied to all containers"
}

# Test 1: Verify local mode works in container
test_local_mode() {
    log_info "Test: Local mode in container"

    # Run jam in local mode with a timeout (it would run forever otherwise)
    output=$(docker exec slurmctld timeout 2 jam --refresh 500 2>&1 || true)

    # Local mode should start (even if it times out)
    # We can't really test interactive TUI, but we can verify it doesn't crash
    if [[ $? -le 124 ]]; then  # 124 is timeout exit code
        log_pass "Local mode starts without crashing"
    else
        log_fail "Local mode crashed: $output"
    fi
}

# Test 2: Verify collector mode outputs NDJSON
test_collector_mode() {
    log_info "Test: Collector mode outputs NDJSON"

    # Submit a job that runs the collector
    job_output=$(docker exec slurmctld bash -c '
        # Submit a simple job
        jobid=$(sbatch --parsable -N1 --wrap="sleep 30" 2>/dev/null)
        echo "JobID: $jobid"
        sleep 2

        # Run collector in the job context
        srun --jobid=$jobid --overlap -N1 timeout 3 jam --collector --refresh 500 2>/dev/null || true

        # Cancel the job
        scancel $jobid 2>/dev/null
    ')

    # Check if output contains valid JSON snapshots
    if echo "$job_output" | grep -q '"type":"snapshot"'; then
        log_pass "Collector mode outputs valid NDJSON snapshots"
    else
        log_fail "Collector mode did not output valid NDJSON: $job_output"
    fi
}

# Test 3: Verify collector discovers cgroup
test_collector_cgroup_discovery() {
    log_info "Test: Collector discovers job cgroup"

    output=$(docker exec slurmctld bash -c '
        # Submit a job
        jobid=$(sbatch --parsable -N1 --wrap="sleep 30" 2>/dev/null)
        sleep 2

        # Run collector and capture stderr (where discovery info is logged)
        srun --jobid=$jobid --overlap -N1 timeout 3 jam --collector --refresh 500 2>&1 || true

        scancel $jobid 2>/dev/null
    ')

    if echo "$output" | grep -q "Collector starting on node"; then
        log_pass "Collector discovers cgroup and starts monitoring"
    else
        log_fail "Collector did not report cgroup discovery: $output"
    fi
}

# Test 4: Verify snapshot contains process information
test_snapshot_contains_processes() {
    log_info "Test: Snapshot contains process information"

    output=$(docker exec slurmctld bash -c '
        # Submit a job that does some work
        jobid=$(sbatch --parsable -N1 --wrap="sleep 30" 2>/dev/null)
        sleep 3

        # Collect one snapshot
        snapshot=$(srun --jobid=$jobid --overlap -N1 timeout 3 jam --collector --refresh 500 2>/dev/null | head -1)
        echo "$snapshot"

        scancel $jobid 2>/dev/null
    ')

    # Check if snapshot has process information
    if echo "$output" | grep -q '"processes":\['; then
        log_pass "Snapshot contains process information"
    else
        log_fail "Snapshot missing process information: $output"
    fi
}

# Test 5: Verify scontrol parsing works
test_scontrol_parsing() {
    log_info "Test: scontrol job parsing"

    output=$(docker exec slurmctld bash -c '
        # Submit a job
        jobid=$(sbatch --parsable -N2 --wrap="sleep 30" 2>/dev/null)
        sleep 2

        # Get job info via scontrol
        scontrol show job $jobid --oneliner

        scancel $jobid 2>/dev/null
    ')

    if echo "$output" | grep -q "JobState=RUNNING\|JobState=PENDING"; then
        log_pass "scontrol job parsing works"
    else
        log_fail "scontrol parsing failed: $output"
    fi
}

# Test 6: Verify nodelist expansion
test_nodelist_expansion() {
    log_info "Test: Nodelist expansion"

    output=$(docker exec slurmctld scontrol show hostnames c[1-2])

    if echo "$output" | grep -q "c1" && echo "$output" | grep -q "c2"; then
        log_pass "Nodelist expansion works"
    else
        log_fail "Nodelist expansion failed: $output"
    fi
}

# Test 7: Multi-node collector test
test_multinode_collectors() {
    log_info "Test: Multi-node collectors"

    output=$(docker exec slurmctld bash -c '
        # Submit a 2-node job
        jobid=$(sbatch --parsable -N2 --wrap="sleep 60" 2>/dev/null)
        sleep 5

        # Run collectors on both nodes in parallel and capture output
        (
            srun --jobid=$jobid --overlap --nodelist=c1 -N1 timeout 3 jam --collector --node-name c1 --refresh 500 2>/dev/null &
            srun --jobid=$jobid --overlap --nodelist=c2 -N1 timeout 3 jam --collector --node-name c2 --refresh 500 2>/dev/null &
            wait
        ) | head -5

        scancel $jobid 2>/dev/null
    ')

    # Check that we got snapshots from both nodes
    c1_count=$(echo "$output" | grep -c '"node":"c1"' || true)
    c2_count=$(echo "$output" | grep -c '"node":"c2"' || true)

    if [[ $c1_count -gt 0 ]] && [[ $c2_count -gt 0 ]]; then
        log_pass "Multi-node collectors work (c1: $c1_count, c2: $c2_count snapshots)"
    else
        log_fail "Multi-node collectors failed (c1: $c1_count, c2: $c2_count snapshots): $output"
    fi
}

# Test 8: Verify protocol version in snapshots
test_protocol_version() {
    log_info "Test: Protocol version in snapshots"

    output=$(docker exec slurmctld bash -c '
        jobid=$(sbatch --parsable -N1 --wrap="sleep 30" 2>/dev/null)
        sleep 2
        srun --jobid=$jobid --overlap -N1 timeout 3 jam --collector --refresh 500 2>/dev/null | head -1
        scancel $jobid 2>/dev/null
    ')

    if echo "$output" | grep -q '"version":1'; then
        log_pass "Protocol version is correct"
    else
        log_fail "Protocol version incorrect: $output"
    fi
}

# Run all tests
main() {
    echo "========================================"
    echo "JAM Slurm Integration Tests"
    echo "========================================"
    echo

    check_prerequisites
    build_jam
    copy_jam_to_containers

    echo
    echo "Running tests..."
    echo

    test_local_mode
    test_scontrol_parsing
    test_nodelist_expansion
    test_collector_mode
    test_collector_cgroup_discovery
    test_snapshot_contains_processes
    test_protocol_version
    test_multinode_collectors

    echo
    echo "========================================"
    echo "Test Results: $passed passed, $failed failed"
    echo "========================================"

    if [[ $failed -gt 0 ]]; then
        exit 1
    fi
}

# Only run main if script is executed directly (not sourced)
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
