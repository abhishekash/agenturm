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
            && self.processes.iter().all(|process| process.pid > 0)
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostProcess {
    pub pid: u32,
    pub start_ticks: u64,
    pub runtime: HostRuntime,
    pub state: Option<HostProcessState>,
    pub resident_bytes: Option<u64>,
    pub user_ticks: Option<u64>,
    pub system_ticks: Option<u64>,
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
