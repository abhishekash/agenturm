//! Retained trace selection and current evaluation metadata presentation.

use super::{WorkCommand, WorkInput, visible_text};
use agenturm_kernel::{ExtensionId, ObservationKind, ReadSnapshot, Snapshot, TraceStatus};
use ratatui::crossterm::event::KeyCode;
use ratatui::text::Line;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceEvaluationEntry {
    pub source_id: ExtensionId,
    pub trace_id: String,
    pub snapshot_id: ReadSnapshot,
    pub observed_ok: usize,
    pub observed_error: usize,
    pub observed_unset: usize,
}

impl TraceEvaluationEntry {
    pub(crate) fn execution(&self) -> String {
        format!(
            "Loaded execution spans · {} OK · {} error · {} unset",
            self.observed_ok, self.observed_error, self.observed_unset
        )
    }
}

pub(crate) fn trace_entries(snapshot: &Snapshot) -> Vec<TraceEvaluationEntry> {
    let mut indices = BTreeMap::new();
    let mut entries: Vec<TraceEvaluationEntry> = Vec::new();
    for row in snapshot.observations.iter().rev() {
        let ObservationKind::TraceSpan(span) = &row.kind else {
            continue;
        };
        if !row.harness_id.as_str().starts_with("otlp.") {
            continue;
        }
        let index = indices
            .entry((&row.harness_id, span.trace_id.as_str()))
            .or_insert_with(|| {
                let index = entries.len();
                entries.push(TraceEvaluationEntry {
                    source_id: row.harness_id.clone(),
                    trace_id: span.trace_id.clone(),
                    snapshot_id: snapshot.history_snapshot_id,
                    observed_ok: 0,
                    observed_error: 0,
                    observed_unset: 0,
                });
                index
            });
        match span.status {
            TraceStatus::Ok => entries[*index].observed_ok += 1,
            TraceStatus::Error => entries[*index].observed_error += 1,
            TraceStatus::Unset => entries[*index].observed_unset += 1,
        }
    }
    entries
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvaluationView {
    Revisions { before: Option<u64> },
    SourceSummary { after: Option<String> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluationRead {
    pub trace: TraceEvaluationEntry,
    pub view: EvaluationView,
}

#[derive(Debug)]
pub struct EvaluationRow {
    pub trace_id: String,
    pub revision: u64,
    pub passed: bool,
    pub details: Vec<String>,
}

#[derive(Debug)]
pub struct EvaluationPage {
    pub request: EvaluationRead,
    pub current_revision: Option<u64>,
    pub rows: Vec<EvaluationRow>,
    pub continuation: Option<EvaluationView>,
}

impl EvaluationPage {
    pub(super) fn summary(&self) -> bool {
        matches!(self.request.view, EvaluationView::SourceSummary { .. })
    }

    pub(super) fn action(&self, key: KeyCode) -> WorkInput {
        let view = match key {
            KeyCode::Char('s') if !self.summary() => EvaluationView::SourceSummary { after: None },
            KeyCode::Char('r') if self.summary() => EvaluationView::SourceSummary { after: None },
            KeyCode::Char('r') => EvaluationView::Revisions { before: None },
            KeyCode::Char(']') if self.continuation.is_some() => {
                self.continuation.clone().expect("continuation checked")
            }
            KeyCode::Esc => return WorkInput::Close,
            _ => return WorkInput::None,
        };
        WorkInput::Command(WorkCommand::Evaluations(Box::new(EvaluationRead {
            trace: self.request.trace.clone(),
            view,
        })))
    }

    pub(super) fn title(&self) -> &'static str {
        if self.summary() {
            "Source evaluation summary · current"
        } else {
            "Trace evaluations · current metadata"
        }
    }

    pub(super) fn keys(&self) -> &'static str {
        match (self.summary(), self.continuation.is_some()) {
            (true, true) => "] more · PgUp/PgDn detail · Esc revisions",
            (true, false) => "r refresh · PgUp/PgDn detail · Esc revisions",
            (false, true) => "s source · ] more · PgUp/PgDn detail · Esc back",
            (false, false) => "s source · r refresh · PgUp/PgDn detail · Esc back",
        }
    }

    pub(super) fn row_labels(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| {
                let outcome = if row.passed { "Passed" } else { "Failed" };
                if self.summary() {
                    format!("{}  r{}  {outcome}", row.trace_id, row.revision)
                } else if Some(row.revision) == self.current_revision {
                    format!("r{} · {outcome} · Current", row.revision)
                } else {
                    format!("r{} · {outcome}", row.revision)
                }
            })
            .collect()
    }

    pub(super) fn details(&self, selected: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if self.summary() {
            let passed = self.rows.iter().filter(|row| row.passed).count();
            let failed = self.rows.len() - passed;
            lines.push(Line::from(format!(
                "Current page · {passed} passed · {failed} failed"
            )));
        } else {
            lines.push(Line::from(self.request.trace.execution()));
            lines.push(Line::from(format!("Trace {}", self.request.trace.trace_id)));
            if let Some(revision) = self.current_revision {
                lines.push(Line::from(format!(
                    "Current evaluation revision r{revision}"
                )));
            }
        }
        lines.push(Line::from(format!(
            "Source {}",
            visible_text(self.request.trace.source_id.as_str())
        )));
        if let Some(row) = self.rows.get(selected) {
            lines.extend(
                row.details
                    .iter()
                    .map(|detail| Line::from(visible_text(detail))),
            );
        }
        lines
    }

    pub(super) fn empty(&self) -> &'static str {
        match &self.request.view {
            EvaluationView::SourceSummary { after: None } => {
                "No retained evaluation outcomes in this source."
            }
            EvaluationView::Revisions { before: None } => {
                "No evaluation outcome has been recorded for this trace."
            }
            _ => "No further retained outcomes on this page.",
        }
    }
}
