//! Inspect the independent typed Workflow alert inventory.

use super::*;
use crate::evidence_view::{EvidenceRow, EvidenceSpec, EvidenceTable, Inspection};
use agenturm_kernel::workflow_alerts::{
    WorkflowAlert, WorkflowAlertClock, WorkflowAlertCoverage, WorkflowAlertKind,
};

const SPEC: EvidenceSpec = EvidenceSpec {
    title: "Workflow incidents",
    inspector: "Selected incident",
    columns: &[("Incident", 3), ("Session", 5), ("Delivery", 3)],
    empty: "No incidents established by this history page.",
};

pub(super) struct Alerts {
    inbox: Box<WorkflowAlertInbox>,
    selected: Option<usize>,
    detail: Option<Inspection>,
    scroll_bounds: (usize, usize),
}

impl Alerts {
    pub(super) fn new(inbox: Box<WorkflowAlertInbox>) -> Self {
        let selected = (!inbox.page.alerts.is_empty()).then_some(0);
        Self {
            inbox,
            selected,
            detail: None,
            scroll_bounds: (1, 0),
        }
    }

    pub(super) fn apply(&mut self, inbox: Box<WorkflowAlertInbox>) {
        let same_history = inbox.page.history_identity == self.inbox.page.history_identity
            && inbox.page.snapshot_id.incarnation() == self.inbox.page.snapshot_id.incarnation();
        let selected = self
            .selected
            .and_then(|index| self.inbox.page.alerts.get(index))
            .filter(|_| same_history)
            .and_then(|prior| {
                inbox.page.alerts.iter().position(|next| {
                    next.id == prior.id
                        && next.scope == prior.scope
                        && next.evidence == prior.evidence
                })
            });
        if selected.is_none() {
            self.detail = None;
        }
        self.selected = selected.or_else(|| (!inbox.page.alerts.is_empty()).then_some(0));
        self.inbox = inbox;
    }

    fn selected_alert(&self) -> Option<&WorkflowAlert> {
        self.inbox.page.alerts.get(self.selected?)
    }

    fn delivery(&self, alert: &WorkflowAlert) -> Option<&WorkflowDelivery> {
        self.inbox
            .deliveries
            .iter()
            .find(|(id, delivery)| {
                id == &alert.id
                    && delivery.authority.history_identity == self.inbox.page.history_identity
                    && delivery.authority.snapshot_id == self.inbox.page.snapshot_id
            })
            .map(|(_, delivery)| delivery)
    }

    pub(super) fn retry(&self) -> Option<WorkCommand> {
        retry_command(self.delivery(self.selected_alert()?)?)
    }

    pub(super) fn accept_delivery(&mut self, updated: &WorkflowDelivery) -> bool {
        updated.authority.history_identity == self.inbox.page.history_identity
            && updated.authority.snapshot_id == self.inbox.page.snapshot_id
            && self.inbox.deliveries.iter_mut().any(|(id, stored)| {
                self.inbox.page.alerts.iter().any(|alert| &alert.id == id)
                    && replace_delivery_state(stored, updated)
            })
    }

    pub(super) fn key(&mut self, code: KeyCode) -> WorkInput {
        if let Some(detail) = &mut self.detail {
            if code == KeyCode::Esc {
                self.detail = None;
            } else {
                let (page, limit) = self.scroll_bounds;
                detail.key(code, page, limit);
            }
            return WorkInput::None;
        }
        let Some(selected) = self.selected else {
            return WorkInput::None;
        };
        match code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected =
                    Some((selected + 1).min(self.inbox.page.alerts.len().saturating_sub(1)))
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = Some(selected.saturating_sub(1)),
            KeyCode::Enter => self.detail = self.table().open_inspection(selected),
            _ => {}
        }
        WorkInput::None
    }

    pub(super) fn inspecting(&self) -> bool {
        self.detail.is_some()
    }

    fn table(&self) -> EvidenceTable {
        let mut spec = SPEC;
        if self.inbox.page.partial {
            spec.title = "Workflow incidents · partial";
            spec.empty = "No incidents established in available evidence. Coverage is partial.";
        }
        let rows = self
            .inbox
            .page
            .alerts
            .iter()
            .map(|alert| {
                let delivery = self
                    .delivery(alert)
                    .map_or("Unrecorded", |delivery| delivery_label(delivery.state));
                let label = kind(&alert.kind);
                let mut details = vec![
                    value("Incident", label),
                    value("Source", alert.scope.source_id.as_str()),
                    value("Session", &alert.scope.session_id),
                    value("Turn", &alert.scope.turn_id),
                    value("Worker", alert.scope.worker_id.as_deref().unwrap_or("Main")),
                    value(
                        "Evidence",
                        format!(
                            "{} · revision {}",
                            alert.evidence.event_id, alert.evidence.revision
                        ),
                    ),
                    value(
                        "Coverage",
                        match alert.coverage {
                            WorkflowAlertCoverage::ExactFact => "Exact recorded fact",
                            WorkflowAlertCoverage::ReportedLowerBound => "Reported lower bound",
                        },
                    ),
                    value("Delivery", delivery),
                ];
                let clock = match alert.evidence.clock {
                    WorkflowAlertClock::SourceReported => "Source reported",
                    WorkflowAlertClock::ReceiverReceipt => "Receiver receipt",
                };
                details.push(value(
                    "Observed Unix ns",
                    format!("{} · {clock}", alert.evidence.observed_at_unix_ns),
                ));
                if alert.quiet {
                    details.push(value("Notification", "Quiet"));
                }
                if let Some(measured) = alert.measured {
                    details.push(value("Measured", amount(&alert.kind, measured)));
                }
                if let Some(threshold) = alert.threshold {
                    details.push(value("Threshold", amount(&alert.kind, threshold)));
                }
                EvidenceRow {
                    cells: vec![
                        label.into(),
                        alert.scope.session_id.clone(),
                        delivery.into(),
                    ],
                    details,
                }
            })
            .collect();
        let mut table = spec.table(rows);
        let gaps = &self.inbox.page.gaps;
        table.context = [
            ("No exact turn", gaps.no_turn),
            ("Tokens unreported", gaps.tokens_unreported),
            ("USD unreported", gaps.usd_unreported),
            ("Elapsed unproven", gaps.elapsed_unproven),
        ]
        .into_iter()
        .filter(|(_, count)| *count != 0)
        .map(|(label, count)| value(label, count.to_string()))
        .collect();
        table
    }

    pub(super) fn draw(&mut self, frame: &mut Frame, pending: bool, status: Option<&str>) {
        let [header, body, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .areas(frame.area());
        let table = self.table();
        frame.render_widget(
            Paragraph::new(format!(" Agenturm / {}", table.spec.title)).style(heading_style()),
            header,
        );
        let index = self.selected.unwrap_or_default();
        let keys = if let Some(detail) = &mut self.detail {
            let page = usize::from(body.height.saturating_sub(2)).max(1);
            let limit = table.inspection_limit(index, body.width, page);
            self.scroll_bounds = (page, limit);
            detail.clamp(limit);
            table.draw_expanded(frame, body, index, detail);
            "↑↓ scroll · PgUp/PgDn · Esc incidents"
        } else {
            table.draw(frame, body, index);
            if self.retry().is_some() {
                "Enter inspect · T retry delivery · R refresh\nTab requests · S settings · Esc requests"
            } else {
                "Enter inspect · R refresh\nTab requests · S settings · Esc requests"
            }
        };
        draw_footer(frame, foot, keys, pending, status);
    }
}

fn kind(value: &WorkflowAlertKind) -> &'static str {
    match value {
        WorkflowAlertKind::Failure => "Run failed",
        WorkflowAlertKind::Interruption => "Run interrupted",
        WorkflowAlertKind::Completion => "Run completed",
        WorkflowAlertKind::Tokens => "Token threshold",
        WorkflowAlertKind::UsdCost => "USD threshold",
        WorkflowAlertKind::Elapsed => "Time threshold",
    }
}

fn amount(kind: &WorkflowAlertKind, value: u64) -> String {
    match kind {
        WorkflowAlertKind::Tokens => format!("{value} tokens"),
        WorkflowAlertKind::UsdCost => format!(
            "{} USD",
            super::workflow_settings::format_decimal(value, 1_000_000_000)
        ),
        WorkflowAlertKind::Elapsed => format!("{value} ns"),
        _ => value.to_string(),
    }
}

fn value(label: &str, value: impl Into<String>) -> MetricRow {
    MetricRow {
        label: label.into(),
        value: value.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::workflow_alerts::{
        WorkflowAlertEvidence, WorkflowAlertGaps, WorkflowAlertScope,
    };
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn incidents_preserve_exact_scope_and_only_the_current_failed_delivery_can_retry() {
        let alert = |id: &str, worker: &str| WorkflowAlert {
            id: id.into(),
            scope: WorkflowAlertScope {
                source_id: ExtensionId::parse("harness.codex.fixture").unwrap(),
                session_id: "same-session".into(),
                turn_id: "recorded-turn".into(),
                worker_id: Some(worker.into()),
            },
            kind: WorkflowAlertKind::UsdCost,
            evidence: WorkflowAlertEvidence {
                event_id: format!("{id}-evidence"),
                revision: 2,
                observed_at_unix_ns: 19,
                clock: WorkflowAlertClock::SourceReported,
            },
            measured: Some(3_000_000_000),
            threshold: Some(2_000_000_000),
            coverage: WorkflowAlertCoverage::ReportedLowerBound,
            quiet: false,
        };
        let chosen = alert("chosen", "worker-a");
        let sibling = alert("sibling", "worker-b");
        let exact_authority = WorkflowNotificationAuthority {
            history_identity: "exact-history".into(),
            snapshot_id: 23.into(),
        };
        let delivery = WorkflowDelivery {
            authority: exact_authority.clone(),
            key: [8; 32],
            policy_revision: 7,
            attempt: 1,
            state: WorkflowDeliveryState::Failed,
        };
        let page = WorkflowAlertPage {
            history_identity: exact_authority.history_identity.clone(),
            snapshot_id: exact_authority.snapshot_id,
            alerts: vec![chosen.clone(), sibling],
            partial: true,
            gaps: WorkflowAlertGaps {
                usd_unreported: 3,
                ..Default::default()
            },
        };
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut browser = Alerts::new(Box::new(WorkflowAlertInbox {
                page: page.clone(),
                deliveries: vec![(chosen.id.clone(), delivery.clone())],
            }));
            assert!(matches!(browser.retry(), Some(WorkCommand::RetryWorkflow {
                authority, key, expected_revision: 7, expected_attempt: 1,
            }) if authority == exact_authority && key == [8; 32]));
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| browser.draw(frame, false, None))
                .unwrap();
            let text = terminal.backend().to_string();
            assert!(text.contains("Workflow incidents · partial"));
            assert!(text.contains("USD threshold"));
            assert!(text.contains("T retry delivery"));
            browser.key(KeyCode::Enter);
            terminal
                .draw(|frame| browser.draw(frame, false, None))
                .unwrap();
            let text = terminal.backend().to_string();
            assert!(text.contains("worker-a"));
            assert!(!text.contains("worker-b"));
            browser.key(KeyCode::Esc);
            let mut updated = delivery.clone();
            updated.attempt = 2;
            updated.state = WorkflowDeliveryState::Delivered;
            assert!(browser.accept_delivery(&updated));
            assert_eq!(browser.selected_alert(), Some(&chosen));
            assert_eq!(browser.delivery(&chosen), Some(&updated));
            assert!(browser.retry().is_none());
            let mut next = page.clone();
            next.alerts.reverse();
            next.snapshot_id = 24.into();
            browser.apply(Box::new(WorkflowAlertInbox {
                page: next,
                deliveries: vec![(chosen.id.clone(), delivery.clone())],
            }));
            assert_eq!(browser.selected_alert(), Some(&chosen));
            assert!(!browser.accept_delivery(&updated));
            assert!(browser.retry().is_none());
        }
    }
}
