//! Recorded Host drill-down with exact process identity within a retained page.

use super::{WorkCommand, WorkInput, workspace};
use crate::{EvidenceRow, EvidenceSpec, EvidenceTable};
use agenturm_kernel::{
    HostProcess, HostSample, MetricRow, Observation, ObservationKind, ObservationPage,
};
use ratatui::crossterm::event::KeyCode;
use ratatui::widgets::ListState;

#[derive(Default)]
pub(super) enum HostLevel {
    #[default]
    Samples,
    Processes(usize),
    ProcessHistory {
        sample: usize,
        process: usize,
    },
}

pub(super) struct Presentation {
    pub table: EvidenceTable,
    pub keys: &'static str,
}

const SAMPLES: EvidenceSpec = EvidenceSpec {
    title: "Host history",
    inspector: "Collection status",
    columns: &[
        ("Sample", 3),
        ("Source", 5),
        ("Processes", 3),
        ("Coverage", 3),
    ],
    empty: "No recorded host samples. Run agenturm host record.",
};

const PROCESSES: EvidenceSpec = EvidenceSpec {
    title: "Host processes",
    inspector: "Process inspector",
    columns: &[
        ("Process", 3),
        ("PID", 2),
        ("State", 3),
        ("RAM", 3),
        ("CPU ticks", 3),
    ],
    empty: "No known same-user processes were sampled.",
};

const PROCESS_HISTORY: EvidenceSpec = EvidenceSpec {
    title: "Process history · loaded samples",
    inspector: "Process inspector",
    columns: &[("Sample", 3), ("RAM", 3), ("CPU ticks", 3)],
    empty: "No samples for this exact process.",
};

impl HostLevel {
    pub fn len(&self, page: &ObservationPage) -> usize {
        match *self {
            Self::Samples => page.observations.len(),
            Self::Processes(index) => {
                sample(page, index).map_or(0, |(_, data)| data.processes.len())
            }
            Self::ProcessHistory { sample, process } => occurrences(page, sample, process).count(),
        }
    }

    pub fn action(
        &mut self,
        page: &ObservationPage,
        cursor: &mut ListState,
        key: KeyCode,
    ) -> WorkInput {
        let index = cursor.selected().unwrap_or_default();
        let (next, selected) = match (&*self, key) {
            (Self::Samples, KeyCode::Esc) => return WorkInput::Close,
            (Self::Samples, KeyCode::Char('r' | 'R' | 'l')) => {
                return WorkInput::Command(WorkCommand::HostHistory(None));
            }
            (Self::Samples, KeyCode::Char(']')) if page.has_more => {
                return WorkInput::Command(WorkCommand::HostHistory(page.older_position()));
            }
            (Self::Samples, KeyCode::Enter) if sample(page, index).is_some() => {
                (Self::Processes(index), 0)
            }
            (Self::Processes(sample_index), KeyCode::Esc) => (Self::Samples, *sample_index),
            (Self::Processes(sample_index), KeyCode::Enter) if index < self.len(page) => (
                Self::ProcessHistory {
                    sample: *sample_index,
                    process: index,
                },
                0,
            ),
            (Self::ProcessHistory { sample, process }, KeyCode::Esc) => {
                (Self::Processes(*sample), *process)
            }
            _ => return WorkInput::None,
        };
        *self = next;
        *cursor = ListState::default().with_selected((self.len(page) != 0).then_some(selected));
        WorkInput::None
    }

    pub fn present(&self, page: &ObservationPage) -> Presentation {
        match *self {
            Self::Samples => Presentation {
                table: SAMPLES.table(
                    page.observations
                        .iter()
                        .rev()
                        .map(|row| {
                            let (processes, coverage, mut details) = match &row.kind {
                                ObservationKind::HostSample(data) => (
                                    data.processes.len().to_string(),
                                    sample_coverage(data).into(),
                                    sample_details(data),
                                ),
                                _ => (
                                    "Unknown".into(),
                                    "Unknown".into(),
                                    vec![detail("Sample", "Unavailable")],
                                ),
                            };
                            details.push(detail("Source", row.harness_id.as_str()));
                            EvidenceRow {
                                cells: vec![
                                    workspace::age(row.observed_at_unix_ns),
                                    row.harness_id.as_str().into(),
                                    processes,
                                    coverage,
                                ],
                                details,
                            }
                        })
                        .collect(),
                ),
                keys: if page.has_more {
                    "Enter processes · ] older · l latest · Esc back"
                } else {
                    "Enter processes · R refresh · Esc back"
                },
            },
            Self::Processes(index) => {
                let data = sample(page, index).map(|(_, data)| data);
                let mut spec = PROCESSES;
                if data.is_some_and(|data| data.processes_partial) {
                    spec.title = "Host processes · partial";
                }
                Presentation {
                    table: spec.table(
                        data.into_iter()
                            .flat_map(|sample| {
                                sample.processes.iter().map(|process| EvidenceRow {
                                    cells: vec![
                                        process.runtime.label().into(),
                                        process.pid.to_string(),
                                        process
                                            .state
                                            .map_or("Unknown", |state| state.label())
                                            .into(),
                                        memory(process.resident_bytes),
                                        total_ticks(process),
                                    ],
                                    details: process_details(process, sample),
                                })
                            })
                            .collect(),
                    ),
                    keys: "Enter process history · Esc samples",
                }
            }
            Self::ProcessHistory { sample, process } => Presentation {
                table: PROCESS_HISTORY.table(
                    occurrences(page, sample, process)
                        .map(|(row, data, process)| {
                            let mut details = process_details(process, data);
                            details.push(detail("Boot", &data.boot_id));
                            EvidenceRow {
                                cells: vec![
                                    workspace::age(row.observed_at_unix_ns),
                                    memory(process.resident_bytes),
                                    total_ticks(process),
                                ],
                                details,
                            }
                        })
                        .collect(),
                ),
                keys: "Esc processes",
            },
        }
    }
}

fn sample(page: &ObservationPage, index: usize) -> Option<(&Observation, &HostSample)> {
    let row = page.observations.iter().rev().nth(index)?;
    if let ObservationKind::HostSample(data) = &row.kind {
        return Some((row, data));
    }
    None
}

fn occurrences(
    page: &ObservationPage,
    sample_index: usize,
    process_index: usize,
) -> impl Iterator<Item = (&Observation, &HostSample, &HostProcess)> {
    let identity = sample(page, sample_index).and_then(|(row, data)| {
        let process = data.processes.get(process_index)?;
        Some((
            &row.harness_id,
            data.boot_id.as_str(),
            process.pid,
            process.start_ticks,
        ))
    });
    (0..page.observations.len()).filter_map(move |index| {
        let (source, boot_id, pid, start_ticks) = identity?;
        let (row, data) = sample(page, index)?;
        if &row.harness_id != source || data.boot_id != boot_id {
            return None;
        }
        let process = data
            .processes
            .iter()
            .find(|process| process.pid == pid && process.start_ticks == start_ticks)?;
        Some((row, data, process))
    })
}

fn sample_coverage(sample: &HostSample) -> &'static str {
    if sample.processes_partial {
        "Partial"
    } else {
        "Sampled"
    }
}

fn sample_details(sample: &HostSample) -> Vec<MetricRow> {
    let load = |amount: Option<u64>| {
        amount.map_or_else(
            || "unknown".into(),
            |value| format!("{}.{:03}", value / 1000, value % 1000),
        )
    };
    vec![
        detail(
            "Process coverage",
            format!(
                "{} rows · {}",
                sample.processes.len(),
                sample_coverage(sample)
            ),
        ),
        detail(
            "Memory available",
            format!(
                "{} / {}",
                memory(sample.memory_available_bytes),
                memory(sample.memory_total_bytes)
            ),
        ),
        detail(
            "Load",
            format!(
                "1m {} · 5m {} · 15m {}",
                load(sample.load_1m_milli),
                load(sample.load_5m_milli),
                load(sample.load_15m_milli)
            ),
        ),
        detail(
            "CPU",
            sample.cpu.map_or_else(
                || "counters unknown".into(),
                |cpu| {
                    format!(
                        "user {} · idle {} ticks",
                        counter(cpu.user_ticks),
                        counter(cpu.idle_ticks)
                    )
                },
            ),
        ),
    ]
}

fn process_details(process: &HostProcess, sample: &HostSample) -> Vec<MetricRow> {
    vec![
        detail(
            "Process",
            format!(
                "{} · PID {} · {}",
                process.runtime.label(),
                process.pid,
                process.state.map_or("Unknown state", |state| state.label())
            ),
        ),
        detail(
            "RSS",
            format!(
                "{} · start {} ticks",
                memory(process.resident_bytes),
                process.start_ticks
            ),
        ),
        detail(
            "CPU",
            format!(
                "user {} · system {} ticks",
                counter(process.user_ticks),
                counter(process.system_ticks)
            ),
        ),
        detail(
            "Clock",
            format!("{} ticks/sec", counter(sample.clock_ticks_per_second)),
        ),
    ]
}

fn detail(label: &str, value: impl Into<String>) -> MetricRow {
    MetricRow {
        label: label.into(),
        value: value.into(),
    }
}

fn total_ticks(process: &HostProcess) -> String {
    let total = process
        .user_ticks
        .zip(process.system_ticks)
        .and_then(|(user, system)| user.checked_add(system));
    counter(total)
}

fn counter(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".into(), |number| number.to_string())
}

fn memory(bytes: Option<u64>) -> String {
    bytes.map_or_else(
        || "unknown".into(),
        |bytes| {
            if bytes < 1024 * 1024 {
                format!("{} KiB", bytes / 1024)
            } else {
                format!("{} MiB", bytes / (1024 * 1024))
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{ExtensionId, HostRuntime, ObservationIdentity};

    #[test]
    fn process_history_keeps_source_boot_and_start_identity() {
        let event = |id: &str, source: &str, boot_id: &str, start_ticks| Observation {
            harness_id: ExtensionId::parse(source).unwrap(),
            event_id: id.into(),
            revision: 0,
            identity: ObservationIdentity::source(),
            observed_at_unix_ns: 0,
            kind: ObservationKind::HostSample(HostSample {
                boot_id: boot_id.into(),
                clock_ticks_per_second: Some(100),
                cpu: None,
                load_1m_milli: Some(0),
                load_5m_milli: None,
                load_15m_milli: None,
                memory_available_bytes: Some(0),
                memory_total_bytes: None,
                processes_partial: false,
                processes: vec![HostProcess {
                    pid: 12,
                    start_ticks,
                    runtime: HostRuntime::Codex,
                    state: None,
                    resident_bytes: None,
                    user_ticks: Some(0),
                    system_ticks: None,
                }],
            }),
        };
        let page = ObservationPage {
            observations: vec![
                event("reused-pid", "host.a", "boot-a", 10),
                event("other-boot", "host.a", "boot-b", 11),
                event("other-source", "host.b", "boot-a", 11),
                event("same-process", "host.a", "boot-a", 11),
                event("selected", "host.a", "boot-a", 11),
            ],
            ..ObservationPage::default()
        };
        let events: Vec<_> = occurrences(&page, 0, 0)
            .map(|(row, _, _)| row.event_id.as_str())
            .collect();
        assert_eq!(events, ["selected", "same-process"]);
        let view = HostLevel::ProcessHistory {
            sample: 0,
            process: 0,
        }
        .present(&page);
        assert!(
            view.table.rows[0].details[2]
                .value
                .contains("user 0 · system unknown ticks")
        );
    }
}
