//! Durable, source-scoped Linux host measurements.

use serde::{Deserialize, Serialize};

pub const MAX_HOST_PROCESSES: usize = 32;

pub fn is_host_source(source: &str) -> bool {
    source
        .strip_prefix("host.")
        .is_some_and(|label| !label.is_empty())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostSample {
    pub boot_id: String,
    pub clock_ticks_per_second: Option<u64>,
    pub cpu: Option<HostCpu>,
    pub load_1m_milli: Option<u64>,
    pub load_5m_milli: Option<u64>,
    pub load_15m_milli: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub processes: Vec<HostProcess>,
    pub processes_partial: bool,
}

/// Cumulative CPU counters across the host CPUs since this boot. Values use
/// Linux USER_HZ ticks. Guest time is included in Linux user/nice counters.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostCpu {
    pub user_ticks: Option<u64>,
    pub nice_ticks: Option<u64>,
    pub system_ticks: Option<u64>,
    pub idle_ticks: Option<u64>,
    pub io_wait_ticks: Option<u64>,
    pub irq_ticks: Option<u64>,
    pub soft_irq_ticks: Option<u64>,
    pub steal_ticks: Option<u64>,
}

impl HostSample {
    pub fn is_valid(&self) -> bool {
        valid_boot_id(&self.boot_id)
            && self.clock_ticks_per_second.is_none_or(|value| value > 0)
            && self.processes.len() <= MAX_HOST_PROCESSES
            && self.processes.iter().all(|process| {
                process.pid > 0
                    && process
                        .parent_pid
                        .is_none_or(|pid| pid > 0 && pid != process.pid)
                    && process
                        .working_directory_name
                        .as_deref()
                        .is_none_or(HostProcess::valid_working_directory_name)
            })
            && self
                .processes
                .windows(2)
                .all(|pair| pair[0].pid < pair[1].pid)
    }
}

fn valid_boot_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => matches!(byte, b'0'..=b'9' | b'a'..=b'f'),
        })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostProcess {
    pub pid: u32,
    /// Parent PID observed during this sample. It does not identify a retained process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_pid: Option<u32>,
    pub start_ticks: u64,
    pub runtime: HostRuntime,
    pub state: Option<HostProcessState>,
    pub resident_bytes: Option<u64>,
    pub user_ticks: Option<u64>,
    pub system_ticks: Option<u64>,
    /// Basename observed during this process sample. The path is discarded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory_name: Option<String>,
}

impl HostProcess {
    pub const MAX_WORKING_DIRECTORY_NAME_BYTES: usize = 128;

    pub fn valid_working_directory_name(value: &str) -> bool {
        !value.is_empty()
            && value.len() <= Self::MAX_WORKING_DIRECTORY_NAME_BYTES
            && value.trim() == value
            && !matches!(value, "." | "..")
            && !value.ends_with(" (deleted)")
            && !value.chars().any(|character| {
                character.is_control()
                    || matches!(character, '/' | '\\' | '\u{061c}' | '\u{200e}' | '\u{200f}'
                        | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostRuntime {
    Agenturm,
    PluginHost,
    StorageService,
    CodexAdapter,
    PiAdapter,
    ClaudeAdapter,
    KiroAdapter,
    OpenCodeAdapter,
    Connector,
    Codex,
    Pi,
    Claude,
    Kiro,
    OpenCode,
}

impl HostRuntime {
    pub fn label(self) -> &'static str {
        match self {
            Self::Agenturm => "Agenturm",
            Self::PluginHost => "Plugin host",
            Self::StorageService => "Storage service",
            Self::CodexAdapter => "Codex adapter",
            Self::PiAdapter => "Pi adapter",
            Self::ClaudeAdapter => "Claude adapter",
            Self::KiroAdapter => "Kiro adapter",
            Self::OpenCodeAdapter => "OpenCode adapter",
            Self::Connector => "Connector",
            Self::Codex => "Codex",
            Self::Pi => "Pi",
            Self::Claude => "Claude",
            Self::Kiro => "Kiro",
            Self::OpenCode => "OpenCode",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostProcessState {
    Running,
    Sleeping,
    Uninterruptible,
    Stopped,
    Tracing,
    Idle,
    Parked,
}

impl HostProcessState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Sleeping => "Sleeping",
            Self::Uninterruptible => "Uninterruptible",
            Self::Stopped => "Stopped",
            Self::Tracing => "Tracing stop",
            Self::Idle => "Idle",
            Self::Parked => "Parked",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_host_process_payload_round_trips_without_a_directory_or_parent() {
        let prior = serde_json::json!({"pid":12,"start_ticks":1,"runtime":"codex","state":null,
            "resident_bytes":null,"user_ticks":null,"system_ticks":null});
        let process: HostProcess = serde_json::from_value(prior.clone()).unwrap();
        assert!(process.working_directory_name.is_none());
        assert!(process.parent_pid.is_none());
        assert_eq!(serde_json::to_value(process).unwrap(), prior);
    }

    #[test]
    fn retained_host_sample_preserves_only_a_valid_parent_pid() {
        let mut retained = serde_json::json!({
            "boot_id": "11111111-1111-1111-1111-111111111111", "processes_partial": false,
            "processes": [{"pid":12,"parent_pid":9,"start_ticks":1,"runtime":"codex",
                "state":null,"resident_bytes":null,"user_ticks":null,"system_ticks":null}]
        });
        let sample: HostSample = serde_json::from_value(retained.clone()).unwrap();
        assert!(sample.is_valid());
        assert_eq!(sample.processes[0].parent_pid, Some(9));
        let restored: HostSample =
            serde_json::from_slice(&serde_json::to_vec(&sample).unwrap()).unwrap();
        assert_eq!(restored, sample);
        for invalid in [0, 12] {
            retained["processes"][0]["parent_pid"] = serde_json::json!(invalid);
            let sample: HostSample = serde_json::from_value(retained.clone()).unwrap();
            assert!(!sample.is_valid());
        }
    }

    #[test]
    fn working_directory_labels_cannot_be_paths_or_controls() {
        assert!(HostProcess::valid_working_directory_name("作業 tree"));
        for invalid in [
            "",
            "/home/user/repo",
            "project (deleted)",
            "a\\b",
            "..",
            " spaced",
            "a\u{202e}b",
            "line\nfeed",
        ] {
            assert!(!HostProcess::valid_working_directory_name(invalid));
        }
        assert!(!HostProcess::valid_working_directory_name(&"x".repeat(129)));
    }
}
