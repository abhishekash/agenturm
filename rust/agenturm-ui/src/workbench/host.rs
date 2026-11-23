//! Recorded Host drill-down with exact process identity within a retained page.

use super::{CurrentHostSample, WorkCommand, WorkInput, workspace};
use crate::{
    EvidenceRow, EvidenceSpec, EvidenceTable,
    pane_filter::{self, Filter, FilterKey},
};
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
        ("Cwd", 5),
        ("State", 3),
        ("RAM", 3),
        ("CPU ticks", 3),
    ],
    empty: "No known same-user processes were sampled.",
};

const PROCESS_HISTORY: EvidenceSpec = EvidenceSpec {
    title: "Process history · loaded samples",
    inspector: "Process inspector",
    columns: &[("Sample", 3), ("Cwd", 5), ("RAM", 3), ("CPU ticks", 3)],
    empty: "No samples for this exact process.",
};

pub(super) fn current_indices(sample: &HostSample, filter: &Filter) -> Vec<usize> {
    sample
        .processes
        .iter()
        .enumerate()
        .filter_map(|(index, process)| {
            let pid = process.pid.to_string();
            filter
                .matches([process.runtime.label(), &pid, cwd_name(process)])
                .then_some(index)
        })
        .collect()
}

pub(super) fn current_present(
    capture: Option<&CurrentHostSample>,
    filter: &Filter,
) -> Presentation {
    let mut spec = PROCESSES;
    spec.title = "Host now";
    spec.inspector = "Current sample";
    if filter.active() {
        spec.empty = "No matching processes in this sample.";
    }
    let Some(capture) = capture else {
        spec.empty = "Current host sample is unavailable from this reader.";
        return Presentation {
            table: spec.table(Vec::new()),
            keys: "R refresh · Esc back",
        };
    };
    let sample = &capture.sample;
    if sample.processes_partial {
        spec.title = "Host now · partial";
    }
    let mut table = spec.table(
        current_indices(sample, filter)
            .into_iter()
            .map(|index| process_row(&sample.processes[index], sample))
            .collect(),
    );
    table.context = vec![detail(
        "Captured",
        workspace::age(capture.captured_at_unix_ns),
    )];
    table.context.extend(sample_details(sample));
    Presentation {
        table,
        keys: filter
            .keys()
            .unwrap_or("/ search · Enter inspect · R refresh · Esc back"),
    }
}

fn process_row(process: &HostProcess, sample: &HostSample) -> EvidenceRow {
    EvidenceRow {
        cells: vec![
            process.runtime.label().into(),
            process.pid.to_string(),
            cwd_name(process).into(),
            process
                .state
                .map_or("Unknown", |state| state.label())
                .into(),
            memory(process.resident_bytes),
            total_ticks(process),
        ],
        details: process_details(process, sample),
    }
}

impl HostLevel {
    pub fn len(&self, page: &ObservationPage, filter: &Filter) -> usize {
        match *self {
            Self::Samples => page.observations.len(),
            Self::Processes(index) => process_indices(page, index, filter).len(),
            Self::ProcessHistory { sample, process } => occurrences(page, sample, process).count(),
        }
    }

    pub fn filter_key(
        &self,
        page: &ObservationPage,
        filter: &mut Filter,
        cursor: &mut ListState,
        key: KeyCode,
    ) -> bool {
        let Self::Processes(index) = *self else {
            return false;
        };
        let previous = cursor
            .selected()
            .and_then(|row| process_indices(page, index, filter).get(row).copied());
        match filter.key(key) {
            FilterKey::Changed => {
                let visible = process_indices(page, index, filter);
                *cursor =
                    ListState::default().with_selected(pane_filter::selection(&visible, previous));
                true
            }
            FilterKey::Handled => true,
            FilterKey::Ignored => false,
        }
    }

    pub fn action(
        &mut self,
        page: &ObservationPage,
        cursor: &mut ListState,
        filter: &Filter,
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
            (Self::Processes(sample_index), KeyCode::Enter) => {
                let visible = process_indices(page, *sample_index, filter);
                let Some(&process) = visible.get(index) else {
                    return WorkInput::None;
                };
                (
                    Self::ProcessHistory {
                        sample: *sample_index,
                        process,
                    },
                    0,
                )
            }
            (Self::ProcessHistory { sample, process }, KeyCode::Esc) => {
                let visible = process_indices(page, *sample, filter);
                (
                    Self::Processes(*sample),
                    pane_filter::selection(&visible, Some(*process)).unwrap_or(0),
                )
            }
            _ => return WorkInput::None,
        };
        *self = next;
        *cursor =
            ListState::default().with_selected((self.len(page, filter) != 0).then_some(selected));
        WorkInput::None
    }

    pub fn present(&self, page: &ObservationPage, filter: &Filter) -> Presentation {
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
                if filter.active() {
                    spec.empty = "No matching processes in this sample.";
                }
                let visible = process_indices(page, index, filter);
                Presentation {
                    table: spec.table(
                        data.into_iter()
                            .flat_map(|sample| {
                                visible
                                    .iter()
                                    .map(|index| process_row(&sample.processes[*index], sample))
                            })
                            .collect(),
                    ),
                    keys: filter
                        .keys()
                        .unwrap_or("/ search · Enter history · Esc samples"),
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
                                    cwd_name(process).into(),
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

fn process_indices(page: &ObservationPage, index: usize, filter: &Filter) -> Vec<usize> {
    sample(page, index).map_or_else(Vec::new, |(_, sample)| current_indices(sample, filter))
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

fn cwd_name(process: &HostProcess) -> &str {
    process.working_directory_name.as_deref().unwrap_or("—")
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
            "Working directory",
            process
                .working_directory_name
                .as_deref()
                .unwrap_or("Unavailable"),
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
    use ratatui::{Terminal, backend::TestBackend};

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
                    working_directory_name: (id == "selected").then(|| "fixture-project".into()),
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
        .present(&page, &Filter::default());
        let selected = &view.table.rows[0];
        assert_eq!(selected.cells[1], "fixture-project");
        assert!(
            selected
                .details
                .iter()
                .any(|row| row.label == "Working directory" && row.value == "fixture-project")
        );
        assert!(
            selected.details.iter().any(
                |row| row.label == "CPU" && row.value.contains("user 0 · system unknown ticks")
            )
        );
        assert_eq!(view.table.rows[1].cells[1], "—");
        assert!(
            view.table.rows[1]
                .details
                .iter()
                .any(|row| row.label == "Working directory" && row.value == "Unavailable")
        );
        let processes = HostLevel::Processes(0).present(&page, &Filter::default());
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for table in [&processes.table, &view.table] {
                terminal
                    .draw(|frame| table.draw(frame, frame.area(), 0))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let screen = buffer
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>();
                assert!(
                    screen.contains("Cwd") && screen.contains("fixture-project"),
                    "{width}×{height}: {screen}"
                );
            }
        }

        let current = |mut sample: HostSample| {
            sample.processes.insert(
                0,
                HostProcess {
                    working_directory_name: Some("unrelated-project".into()),
                    start_ticks: 3,
                    ..sample.processes[0].clone()
                },
            );
            super::super::WorkResult::HostNow(Some(Box::new(CurrentHostSample {
                captured_at_unix_ns: 0,
                sample,
            })))
        };
        let ObservationKind::HostSample(sample) = event("selected", "host.a", "boot-a", 11).kind
        else {
            unreachable!()
        };
        let mut work = super::super::Workbench::new(current(sample.clone()), None);
        for key in std::iter::once(KeyCode::Char('/'))
            .chain("CODEX 12 fixture-project".chars().map(KeyCode::Char))
            .chain([KeyCode::Enter])
        {
            assert!(work.host_filter_key(key));
        }
        assert_eq!(
            work.current_process_identity(),
            Some(("boot-a".into(), 12, 11))
        );
        assert!(work.host_filter_key(KeyCode::Esc));
        assert_eq!(work.cursor.selected(), Some(1));
        let mut next = current(sample);
        let super::super::WorkResult::HostNow(Some(capture)) = &mut next else {
            unreachable!()
        };
        capture.sample.processes.swap(0, 1);
        work.apply(next);
        assert_eq!(
            work.current_process_identity(),
            Some(("boot-a".into(), 12, 11))
        );
        assert_eq!(work.cursor.selected(), Some(0));
    }
}
