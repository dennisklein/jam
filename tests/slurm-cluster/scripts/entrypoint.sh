#!/bin/bash
set -e

# Start munge for authentication
munged -f

# Wait for munge to be ready
sleep 1

exec "$@"
