//! Recorded Host drill-down with exact process identity within a retained page.

use super::{WorkCommand, WorkInput, visible_text, workspace};
use agenturm_kernel::{HostProcess, HostSample, Observation, ObservationKind, ObservationPage};
use ratatui::crossterm::event::KeyCode;
use ratatui::text::Line;
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
    pub title: &'static str,
    pub keys: &'static str,
    pub options: Vec<String>,
    pub details: Vec<Line<'static>>,
    pub empty: &'static str,
}

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

    pub fn present(&self, page: &ObservationPage, selected: usize) -> Presentation {
        match *self {
            Self::Samples => Presentation {
                title: "Host history",
                keys: if page.has_more {
                    "Enter processes · ] older · l latest · Esc back"
                } else {
                    "Enter processes · R refresh · Esc back"
                },
                options: page
                    .observations
                    .iter()
                    .rev()
                    .map(|row| {
                        format!(
                            "{}  {}",
                            workspace::age(row.observed_at_unix_ns),
                            visible_text(row.harness_id.as_str())
                        )
                    })
                    .collect(),
                details: sample(page, selected)
                    .map_or_else(Vec::new, |(_, data)| sample_details(data)),
                empty: "No recorded host samples. Run agenturm host record.",
            },
            Self::Processes(index) => {
                let data = sample(page, index).map(|(_, data)| data);
                Presentation {
                    title: if data.is_some_and(|data| data.processes_partial) {
                        "Host processes · partial"
                    } else {
                        "Host processes"
                    },
                    keys: "Enter process history · Esc samples",
                    options: data
                        .into_iter()
                        .flat_map(|sample| &sample.processes)
                        .map(|row| {
                            format!(
                                "{}  PID {}  {}",
                                row.runtime.label(),
                                row.pid,
                                memory(row.resident_bytes)
                            )
                        })
                        .collect(),
                    details: data
                        .and_then(|data| {
                            data.processes
                                .get(selected)
                                .map(|process| process_details(process, data))
                        })
                        .unwrap_or_default(),
                    empty: "No known same-user processes were sampled.",
                }
            }
            Self::ProcessHistory { sample, process } => {
                let mut details = Vec::new();
                let options = occurrences(page, sample, process)
                    .enumerate()
                    .map(|(index, (row, data, process))| {
                        if index == selected {
                            details = process_details(process, data);
                            details.push(Line::from(format!("Boot {}", data.boot_id)));
                        }
                        format!(
                            "{}  {}",
                            workspace::age(row.observed_at_unix_ns),
                            memory(process.resident_bytes)
                        )
                    })
                    .collect();
                Presentation {
                    title: "Process history · loaded samples",
                    keys: "Esc processes",
                    options,
                    details,
                    empty: "No samples for this exact process.",
                }
            }
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

fn sample_details(sample: &HostSample) -> Vec<Line<'static>> {
    let load = |amount: Option<u64>| {
        amount.map_or_else(
            || "unknown".into(),
            |value| format!("{}.{:03}", value / 1000, value % 1000),
        )
    };
    vec![
        Line::from(format!(
            "Load 1m {} · 5m {} · 15m {}",
            load(sample.load_1m_milli),
            load(sample.load_5m_milli),
            load(sample.load_15m_milli)
        )),
        Line::from(format!(
            "Memory available {} / {}",
            memory(sample.memory_available_bytes),
            memory(sample.memory_total_bytes)
        )),
        Line::from(sample.cpu.map_or_else(
            || "Host CPU counters unknown".into(),
            |cpu| {
                format!(
                    "CPU user {} · idle {} ticks",
                    counter(cpu.user_ticks),
                    counter(cpu.idle_ticks)
                )
            },
        )),
        Line::from(format!(
            "{} processes{}",
            sample.processes.len(),
            if sample.processes_partial {
                " · partial"
            } else {
                ""
            }
        )),
    ]
}

fn process_details(process: &HostProcess, sample: &HostSample) -> Vec<Line<'static>> {
    vec![
        Line::from(format!(
            "{} · PID {} · {}",
            process.runtime.label(),
            process.pid,
            process.state.map_or("Unknown state", |state| state.label())
        )),
        Line::from(format!(
            "RSS {} · start {} ticks",
            memory(process.resident_bytes),
            process.start_ticks
        )),
        Line::from(format!(
            "CPU user {} · system {} ticks",
            counter(process.user_ticks),
            counter(process.system_ticks)
        )),
        Line::from(format!(
            "Clock {} ticks/sec",
            counter(sample.clock_ticks_per_second)
        )),
    ]
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
        .present(&page, 0);
        assert!(
            view.details[2]
                .to_string()
                .contains("user 0 · system unknown ticks")
        );
    }
}
