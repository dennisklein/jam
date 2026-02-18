// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Slurm integration module for monitoring jobs across multiple nodes.
//!
//! Architecture:
//! - Coordinator mode (--jobid): Queries Slurm, spawns collectors via srun, aggregates metrics
//! - Collector mode (--collector): Outputs NDJSON metrics from local cgroup

pub mod collector;
pub mod coordinator;
pub mod slurm;
pub mod types;

pub use collector::run_collector_mode;
pub use coordinator::run_coordinator_mode;
