// SPDX-FileCopyrightText: 2026 GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Slurm CLI integration: query job information via scontrol.

use std::process::Command;

use anyhow::{anyhow, Context, Result};

/// Job state as reported by Slurm
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Suspended,
    Completing,
    Completed,
    Cancelled,
    Failed,
    Timeout,
    NodeFail,
    Unknown(String),
}

impl From<&str> for JobState {
    fn from(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "PENDING" | "PD" => JobState::Pending,
            "RUNNING" | "R" => JobState::Running,
            "SUSPENDED" | "S" => JobState::Suspended,
            "COMPLETING" | "CG" => JobState::Completing,
            "COMPLETED" | "CD" => JobState::Completed,
            "CANCELLED" | "CA" => JobState::Cancelled,
            "FAILED" | "F" => JobState::Failed,
            "TIMEOUT" | "TO" => JobState::Timeout,
            "NODE_FAIL" | "NF" => JobState::NodeFail,
            other => JobState::Unknown(other.to_string()),
        }
    }
}

impl JobState {
    /// Check if the job is in a running state (can be monitored)
    pub fn is_running(&self) -> bool {
        matches!(self, JobState::Running | JobState::Completing)
    }

    /// Check if the job has finished
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            JobState::Completed | JobState::Cancelled | JobState::Failed | JobState::Timeout | JobState::NodeFail
        )
    }
}

/// Information about a Slurm job
#[derive(Debug, Clone)]
pub struct JobInfo {
    /// Job ID
    pub job_id: u32,
    /// Job name
    pub name: String,
    /// Job state
    pub state: JobState,
    /// User who submitted the job
    pub user: String,
    /// Compact nodelist (e.g., "node[001-003]")
    pub nodelist: String,
    /// Number of nodes allocated
    pub num_nodes: u32,
    /// Partition
    pub partition: String,
    /// Exit code (if completed)
    pub exit_code: Option<i32>,
}

/// Get job information from Slurm via scontrol
pub fn get_job_info(jobid: u32) -> Result<JobInfo> {
    let output = Command::new("scontrol")
        .args(["show", "job", &jobid.to_string(), "--oneliner"])
        .output()
        .context("Failed to execute scontrol")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Invalid job id") || stderr.contains("not found") {
            return Err(anyhow!("Job {} not found", jobid));
        }
        return Err(anyhow!("scontrol failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_scontrol_output(&stdout, jobid)
}

/// Parse scontrol show job output (oneliner format)
fn parse_scontrol_output(output: &str, jobid: u32) -> Result<JobInfo> {
    let line = output
        .lines()
        .find(|l| !l.is_empty())
        .ok_or_else(|| anyhow!("Empty scontrol output"))?;

    let mut info = JobInfo {
        job_id: jobid,
        name: String::new(),
        state: JobState::Unknown("".to_string()),
        user: String::new(),
        nodelist: String::new(),
        num_nodes: 0,
        partition: String::new(),
        exit_code: None,
    };

    // Parse key=value pairs
    for part in line.split_whitespace() {
        if let Some((key, value)) = part.split_once('=') {
            match key {
                "JobId" => {
                    info.job_id = value.parse().unwrap_or(jobid);
                }
                "JobName" | "Name" => {
                    info.name = value.to_string();
                }
                "JobState" => {
                    info.state = JobState::from(value);
                }
                "UserId" => {
                    // UserId is often "user(uid)", extract just the username
                    info.user = value.split('(').next().unwrap_or(value).to_string();
                }
                "NodeList" => {
                    info.nodelist = value.to_string();
                }
                "NumNodes" => {
                    info.num_nodes = value.parse().unwrap_or(0);
                }
                "Partition" => {
                    info.partition = value.to_string();
                }
                "ExitCode" => {
                    // ExitCode is often "exit_code:signal", extract exit code
                    if let Some(code_str) = value.split(':').next() {
                        info.exit_code = code_str.parse().ok();
                    }
                }
                _ => {}
            }
        }
    }

    Ok(info)
}

/// Expand a Slurm nodelist to individual hostnames
///
/// Uses `scontrol show hostnames` which handles all Slurm nodelist formats.
pub fn expand_nodelist(nodelist: &str) -> Result<Vec<String>> {
    if nodelist.is_empty() || nodelist == "(null)" {
        return Ok(Vec::new());
    }

    let output = Command::new("scontrol")
        .args(["show", "hostnames", nodelist])
        .output()
        .context("Failed to execute scontrol show hostnames")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to expand nodelist '{}': {}", nodelist, stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Check if Slurm commands are available
#[allow(dead_code)]
pub fn is_slurm_available() -> bool {
    Command::new("scontrol")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_state_from_str() {
        assert_eq!(JobState::from("RUNNING"), JobState::Running);
        assert_eq!(JobState::from("R"), JobState::Running);
        assert_eq!(JobState::from("PENDING"), JobState::Pending);
        assert_eq!(JobState::from("COMPLETED"), JobState::Completed);
        assert!(matches!(JobState::from("WEIRD"), JobState::Unknown(_)));
    }

    #[test]
    fn test_job_state_is_running() {
        assert!(JobState::Running.is_running());
        assert!(JobState::Completing.is_running());
        assert!(!JobState::Pending.is_running());
        assert!(!JobState::Completed.is_running());
    }

    #[test]
    fn test_job_state_is_finished() {
        assert!(JobState::Completed.is_finished());
        assert!(JobState::Cancelled.is_finished());
        assert!(JobState::Failed.is_finished());
        assert!(!JobState::Running.is_finished());
        assert!(!JobState::Pending.is_finished());
    }

    #[test]
    fn test_parse_scontrol_output() {
        let output = "JobId=12345 JobName=test_job UserId=testuser(1000) GroupId=users(100) \
                      MCS_label=N/A Priority=4294901755 Nice=0 Account=default QOS=normal \
                      JobState=RUNNING Reason=None Dependency=(null) Requeue=1 Restarts=0 \
                      BatchFlag=1 Reboot=0 ExitCode=0:0 RunTime=00:10:00 TimeLimit=01:00:00 \
                      TimeMin=N/A SubmitTime=2024-01-01T10:00:00 EligibleTime=2024-01-01T10:00:00 \
                      AccrueTime=2024-01-01T10:00:00 StartTime=2024-01-01T10:00:00 \
                      EndTime=2024-01-01T11:00:00 Deadline=N/A SuspendTime=None \
                      SecsPreSuspend=0 LastSchedEval=2024-01-01T10:00:00 Scheduler=Main \
                      Partition=compute AllocNode:Sid=login:12345 ReqNodeList=(null) \
                      ExcNodeList=(null) NodeList=node[001-003] BatchHost=node001 \
                      NumNodes=3 NumCPUs=12 NumTasks=3 CPUs/Task=1 ReqB:S:C:T=0:0:*:*";

        let info = parse_scontrol_output(output, 12345).unwrap();
        assert_eq!(info.job_id, 12345);
        assert_eq!(info.name, "test_job");
        assert_eq!(info.state, JobState::Running);
        assert_eq!(info.user, "testuser");
        assert_eq!(info.nodelist, "node[001-003]");
        assert_eq!(info.num_nodes, 3);
        assert_eq!(info.partition, "compute");
    }

    #[test]
    fn test_parse_scontrol_output_completed() {
        let output = "JobId=99999 JobName=finished_job UserId=user(1001) \
                      JobState=COMPLETED ExitCode=0:0 NodeList=node001 NumNodes=1 Partition=debug";

        let info = parse_scontrol_output(output, 99999).unwrap();
        assert_eq!(info.state, JobState::Completed);
        assert_eq!(info.exit_code, Some(0));
    }

    #[test]
    fn test_parse_scontrol_output_failed() {
        let output = "JobId=88888 JobName=failed_job UserId=user(1001) \
                      JobState=FAILED ExitCode=1:0 NodeList=node002 NumNodes=1 Partition=debug";

        let info = parse_scontrol_output(output, 88888).unwrap();
        assert_eq!(info.state, JobState::Failed);
        assert_eq!(info.exit_code, Some(1));
    }
}
