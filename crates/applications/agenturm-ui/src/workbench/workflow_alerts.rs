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

const RECENT_COMPLETION_DAYS: u64 = 10;
const NANOS_PER_DAY: u64 = 86_400_000_000_000;

pub(super) struct Alerts {
    inbox: Box<WorkflowAlertInbox>,
    selected: Option<usize>,
    detail: Option<Inspection>,
    scroll_bounds: (usize, usize),
    notice: Option<&'static str>,
    include_older_completions: bool,
}

impl Alerts {
    pub(super) fn new(inbox: Box<WorkflowAlertInbox>) -> Self {
        let mut alerts = Self {
            inbox,
            selected: None,
            detail: None,
            scroll_bounds: (1, 0),
            notice: None,
            include_older_completions: false,
        };
        alerts.selected = alerts.visible_indices().first().copied();
        alerts
    }

    pub(super) fn apply(&mut self, inbox: Box<WorkflowAlertInbox>) {
        let same_history = inbox.page.history_identity == self.inbox.page.history_identity
            && inbox.page.snapshot_id.incarnation() == self.inbox.page.snapshot_id.incarnation();
        let requested = self.selected.is_some();
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
        self.notice = if !same_history && !inbox.page.alerts.is_empty() {
            Some("Incident history changed · showing the latest page")
        } else if requested && selected.is_none() && !inbox.page.alerts.is_empty() {
            Some("Selected incident is no longer established · nearest incident selected")
        } else {
            None
        };
        self.inbox = inbox;
        let visible = self.visible_indices();
        self.selected = selected
            .filter(|index| visible.contains(index))
            .or_else(|| visible.first().copied());
    }

    fn visible_indices(&self) -> Vec<usize> {
        let cutoff = current_time_unix_ns()
            .saturating_sub(RECENT_COMPLETION_DAYS.saturating_mul(NANOS_PER_DAY));
        self.inbox
            .page
            .alerts
            .iter()
            .enumerate()
            .filter_map(|(index, alert)| {
                (self.include_older_completions
                    || alert.kind != WorkflowAlertKind::Completion
                    || alert.evidence.observed_at_unix_ns >= cutoff)
                    .then_some(index)
            })
            .collect()
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
        if code == KeyCode::Char('h') {
            let previous = self.selected;
            self.include_older_completions = !self.include_older_completions;
            let visible = self.visible_indices();
            self.selected = previous
                .filter(|index| visible.contains(index))
                .or_else(|| visible.first().copied());
            return WorkInput::None;
        }
        let visible = self.visible_indices();
        let Some(position) = self
            .selected
            .and_then(|selected| visible.iter().position(|index| *index == selected))
        else {
            return WorkInput::None;
        };
        match code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = visible
                    .get((position + 1).min(visible.len().saturating_sub(1)))
                    .copied()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = visible.get(position.saturating_sub(1)).copied()
            }
            KeyCode::Enter => self.detail = self.table().open_inspection(position),
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
        let visible = self.visible_indices();
        if visible.is_empty() && !self.inbox.page.alerts.is_empty() {
            spec.empty = "No recent incidents · h includes older completions.";
        }
        let rows = self
            .inbox
            .page
            .alerts
            .iter()
            .enumerate()
            .filter(|(index, _)| visible.contains(index))
            .map(|(_, alert)| {
                let delivery = self
                    .delivery(alert)
                    .map_or("Unrecorded", |delivery| delivery_label(delivery.state));
                let label = kind(&alert.kind);
                let mut details = vec![
                    value("Observed owner", observed_owner(alert)),
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
                    value("Recorded approval state", approval_state(&alert.kind)),
                    value("Delivery", delivery),
                ];
                if let Some(delivery) = self.delivery(alert) {
                    details.push(value(
                        "Delivery attempt",
                        format!(
                            "#{} · policy revision {}",
                            delivery.attempt, delivery.policy_revision
                        ),
                    ));
                }
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
        let status = status.or(self.notice);
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
        let visible = self.visible_indices();
        let index = self
            .selected
            .and_then(|selected| visible.iter().position(|index| *index == selected))
            .unwrap_or_default();
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
                if self.include_older_completions {
                    "Enter inspect · T retry delivery · R refresh\nh hide older · Tab requests · S settings · Esc back"
                } else {
                    "Enter inspect · T retry delivery · R refresh\nh older · Tab requests · S settings · Esc back"
                }
            } else {
                if self.include_older_completions {
                    "Enter inspect · R refresh\nh hide older · Tab requests · S settings · Esc back"
                } else {
                    "Enter inspect · R refresh · h older completions\nTab requests · S settings · Esc back"
                }
            }
        };
        draw_footer(frame, foot, keys, pending, status);
    }
}

fn observed_owner(alert: &WorkflowAlert) -> String {
    let owner = match alert.scope.worker_id.as_deref() {
        Some(worker) => format!("worker {worker} · session {}", alert.scope.session_id),
        None => format!("main session {}", alert.scope.session_id),
    };
    format!(
        "{} · {} · turn {}",
        alert.scope.source_id.as_str(),
        owner,
        alert.scope.turn_id
    )
}

fn approval_state(value: &WorkflowAlertKind) -> &'static str {
    match value {
        WorkflowAlertKind::Failure => "Recorded failure · response stays in the native terminal",
        WorkflowAlertKind::Interruption => {
            "Recorded interruption · response stays in the native terminal"
        }
        WorkflowAlertKind::Completion => {
            "Recorded completion · not a correctness verdict; review acknowledges locally"
        }
        WorkflowAlertKind::Tokens | WorkflowAlertKind::UsdCost | WorkflowAlertKind::Elapsed => {
            "Running threshold · request remains pending"
        }
    }
}

fn current_time_unix_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
        .unwrap_or(0)
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
            assert!(
                text.contains("h older"),
                "default retry footer clipped at {width} columns: {text}"
            );
            assert!(
                text.contains("R refresh"),
                "default retry footer clipped at {width} columns: {text}"
            );
            assert!(
                text.contains("Esc back"),
                "default retry footer clipped at {width} columns: {text}"
            );
            browser.key(KeyCode::Enter);
            terminal
                .draw(|frame| browser.draw(frame, false, None))
                .unwrap();
            let text = terminal.backend().to_string();
            assert!(text.contains("Observed owner"));
            assert!(text.contains("worker worker-a"));
            assert!(text.contains("Recorded approval state"));
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

            let mut replaced = page.clone();
            replaced.snapshot_id = agenturm_kernel::ReadSnapshot::new(
                25,
                Some("00112233445566778899aabbccddeeff".parse().unwrap()),
            );
            browser.apply(Box::new(WorkflowAlertInbox {
                page: replaced,
                deliveries: vec![(chosen.id.clone(), delivery.clone())],
            }));
            assert_eq!(browser.selected_alert(), page.alerts.first());
            terminal
                .draw(|frame| browser.draw(frame, false, None))
                .unwrap();
            assert!(
                terminal
                    .backend()
                    .to_string()
                    .contains("Incident history changed")
            );
        }
    }

    fn fixture_alert(id: &str, kind: WorkflowAlertKind, observed_at_unix_ns: u64) -> WorkflowAlert {
        WorkflowAlert {
            id: id.into(),
            scope: WorkflowAlertScope {
                source_id: ExtensionId::parse("harness.codex.fixture").unwrap(),
                session_id: "session-a".into(),
                turn_id: format!("turn-{id}"),
                worker_id: None,
            },
            kind,
            evidence: WorkflowAlertEvidence {
                event_id: format!("{id}-event"),
                revision: 1,
                observed_at_unix_ns,
                clock: WorkflowAlertClock::SourceReported,
            },
            measured: None,
            threshold: None,
            coverage: WorkflowAlertCoverage::ExactFact,
            quiet: false,
        }
    }

    fn fixture_inbox(alerts: Vec<WorkflowAlert>) -> Box<WorkflowAlertInbox> {
        Box::new(WorkflowAlertInbox {
            page: WorkflowAlertPage {
                history_identity: "history-a".into(),
                snapshot_id: 23.into(),
                alerts,
                partial: false,
                gaps: WorkflowAlertGaps::default(),
            },
            deliveries: Vec::new(),
        })
    }

    fn history_fixture_alerts() -> Vec<WorkflowAlert> {
        let now = current_time_unix_ns();
        let cutoff = now.saturating_sub(RECENT_COMPLETION_DAYS * NANOS_PER_DAY);
        // Keep these on the recent side across the tiny clock advance between
        // fixture construction and the view's independently sampled cutoff.
        let within_window = cutoff.saturating_add(60_000_000_000);
        vec![
            fixture_alert(
                "old-completion",
                WorkflowAlertKind::Completion,
                cutoff.saturating_sub(1),
            ),
            fixture_alert(
                "within-window-completion",
                WorkflowAlertKind::Completion,
                within_window,
            ),
            fixture_alert(
                "recent-completion",
                WorkflowAlertKind::Completion,
                cutoff.saturating_add(NANOS_PER_DAY),
            ),
            fixture_alert("old-failure", WorkflowAlertKind::Failure, 1),
            fixture_alert("old-interruption", WorkflowAlertKind::Interruption, 1),
            fixture_alert("old-tokens", WorkflowAlertKind::Tokens, 1),
            fixture_alert("old-usd", WorkflowAlertKind::UsdCost, 1),
            fixture_alert("old-elapsed", WorkflowAlertKind::Elapsed, 1),
        ]
    }

    #[test]
    fn older_completion_toggle_keeps_recent_completions_and_all_other_incidents_visible() {
        let alerts = history_fixture_alerts();
        let mut browser = Alerts::new(fixture_inbox(alerts.clone()));
        assert_eq!(browser.visible_indices(), vec![1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(browser.selected_alert(), Some(&alerts[1]));

        browser.key(KeyCode::Char('h'));
        assert_eq!(
            browser.visible_indices(),
            (0..alerts.len()).collect::<Vec<_>>()
        );
        assert_eq!(browser.selected_alert(), Some(&alerts[1]));
        browser.key(KeyCode::Char('h'));
        assert_eq!(browser.visible_indices(), vec![1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(browser.selected_alert(), Some(&alerts[1]));
    }

    #[test]
    fn older_completion_toggle_falls_back_only_when_the_selected_row_becomes_hidden() {
        let alerts = history_fixture_alerts();
        let mut browser = Alerts::new(fixture_inbox(alerts.clone()));
        browser.key(KeyCode::Down);
        assert_eq!(browser.selected_alert(), Some(&alerts[2]));
        browser.key(KeyCode::Char('h'));
        assert_eq!(browser.selected_alert(), Some(&alerts[2]));

        browser.selected = Some(0);
        browser.key(KeyCode::Char('h'));
        assert_eq!(browser.selected, Some(1));
        assert_eq!(browser.selected_alert(), Some(&alerts[1]));
    }

    #[test]
    fn refresh_keeps_exact_selected_alert_in_the_same_history_and_drops_stale_detail() {
        let alerts = history_fixture_alerts();
        let chosen = alerts[3].clone();
        let mut browser = Alerts::new(fixture_inbox(alerts.clone()));
        browser.selected = Some(3);
        browser.key(KeyCode::Enter);
        assert!(browser.inspecting());

        let mut reordered = alerts.clone();
        reordered.reverse();
        let mut same_history = fixture_inbox(reordered.clone());
        same_history.page.snapshot_id = 24.into();
        browser.apply(same_history);
        assert_eq!(browser.selected_alert(), Some(&chosen));
        assert!(browser.inspecting());

        let mut other_history = fixture_inbox(reordered.clone());
        other_history.page.history_identity = "history-b".into();
        other_history.page.snapshot_id = 25.into();
        browser.apply(other_history);
        assert!(!browser.inspecting());
        assert_eq!(browser.selected_alert(), Some(&reordered[0]));
    }

    #[test]
    fn history_toggle_is_ignored_in_detail_and_footer_controls_fit_narrow_widths() {
        let now = current_time_unix_ns();
        let cutoff = now.saturating_sub(RECENT_COMPLETION_DAYS * NANOS_PER_DAY);
        let completion = fixture_alert(
            "completion",
            WorkflowAlertKind::Completion,
            cutoff.saturating_sub(1),
        );
        let failure = fixture_alert("failure", WorkflowAlertKind::Failure, 1);
        let inbox = fixture_inbox(vec![completion.clone(), failure]);

        let mut browser = Alerts::new(inbox);
        browser.selected = Some(1);
        browser.key(KeyCode::Enter);
        assert!(browser.inspecting());
        browser.key(KeyCode::Char('h'));
        assert!(browser.inspecting());
        assert!(!browser.include_older_completions);
        browser.key(KeyCode::Esc);
        assert!(!browser.inspecting());
        assert_eq!(
            browser.selected_alert().map(|alert| alert.id.as_str()),
            Some("failure")
        );
        browser.key(KeyCode::Char('h'));
        assert!(browser.include_older_completions);

        let delivery = WorkflowDelivery {
            authority: WorkflowNotificationAuthority {
                history_identity: "history-a".into(),
                snapshot_id: 23.into(),
            },
            key: [3; 32],
            policy_revision: 1,
            attempt: 1,
            state: WorkflowDeliveryState::Failed,
        };
        let mut with_retry = Alerts::new(Box::new(WorkflowAlertInbox {
            page: WorkflowAlertPage {
                history_identity: "history-a".into(),
                snapshot_id: 23.into(),
                alerts: vec![completion],
                partial: false,
                gaps: WorkflowAlertGaps::default(),
            },
            deliveries: vec![("completion".into(), delivery)],
        }));
        with_retry.key(KeyCode::Char('h'));
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| with_retry.draw(frame, false, None))
                .unwrap();
            let text = terminal.backend().to_string();
            assert!(
                text.contains("T retry delivery"),
                "footer clipped at {width} columns: {text}"
            );
            assert!(
                text.contains("h hide older"),
                "footer clipped at {width} columns: {text}"
            );
            assert!(
                text.contains("R refresh"),
                "footer clipped at {width} columns: {text}"
            );
            assert!(
                text.contains("Esc back"),
                "footer clipped at {width} columns: {text}"
            );
        }

        let mut without_retry = Alerts::new(fixture_inbox(vec![fixture_alert(
            "old-completion",
            WorkflowAlertKind::Completion,
            cutoff.saturating_sub(1),
        )]));
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| without_retry.draw(frame, false, None))
                .unwrap();
            let text = terminal.backend().to_string();
            assert!(
                text.contains("h older completions"),
                "default footer clipped at {width} columns: {text}"
            );
            assert!(
                text.contains("R refresh"),
                "default footer clipped at {width} columns: {text}"
            );
            without_retry.key(KeyCode::Char('h'));
            terminal
                .draw(|frame| without_retry.draw(frame, false, None))
                .unwrap();
            let text = terminal.backend().to_string();
            assert!(
                text.contains("h hide older"),
                "revealed footer clipped at {width} columns: {text}"
            );
            assert!(
                text.contains("R refresh"),
                "revealed footer clipped at {width} columns: {text}"
            );
            assert!(
                text.contains("Esc back"),
                "revealed footer clipped at {width} columns: {text}"
            );
            without_retry.key(KeyCode::Char('h'));
        }
    }
}
