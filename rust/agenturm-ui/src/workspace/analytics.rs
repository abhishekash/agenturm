//! Async retained metric navigation and the pinned Usage chart.

use super::*;
use crate::metric_history::{MetricBrowser, MetricInput};
use crate::usage_chart::UsageChart;
use agenturm_kernel::retained_history::MetricHistoryPage;

pub(super) enum HistoryAnalytics {
    Metrics(Box<MetricBrowser>),
    Usage(Box<UsageChart>),
}

impl HistoryAnalytics {
    pub(super) fn take(selection: &HistorySelection, page: &mut HistoryPage) -> Option<Self> {
        let snapshot = page.snapshot.history_snapshot_id;
        match selection {
            HistorySelection::All(WorkspaceView::Metrics)
            | HistorySelection::Session(_, SessionView::Metrics) => {
                let series = page.features.metric_history.take()?;
                if !matches!(series.as_ref(), MetricHistoryPage::Series { snapshot_id, .. } if *snapshot_id == snapshot)
                {
                    return None;
                }
                let source_id = match selection {
                    HistorySelection::Session(session, SessionView::Metrics) => {
                        Some(agenturm_kernel::ExtensionId::parse(session.harness_id.clone()).ok()?)
                    }
                    _ => None,
                };
                MetricBrowser::scoped(*series, source_id)
                    .map(|browser| Self::Metrics(Box::new(browser)))
            }
            HistorySelection::All(WorkspaceView::Usage)
            | HistorySelection::Session(_, SessionView::Usage) => {
                let data = page.features.usage_history.take()?;
                (data.page.snapshot_id == snapshot)
                    .then(|| Self::Usage(Box::new(UsageChart::new(*data))))
            }
            _ => None,
        }
    }

    pub(super) fn loaded_view(&self) -> LoadedView {
        match self {
            Self::Metrics(browser) => browser.loaded_view(),
            Self::Usage(chart) => chart.loaded_view(),
        }
    }

    pub(super) fn help(&self) -> &'static str {
        match self {
            Self::Metrics(_) => concat!(
                "/ Search retained catalog · d Series details\n",
                "↑/↓ Select or scroll · Home/End First / last · Enter Open selected row\n",
                "[ / ] Previous / next retained page\n",
                "e Next exemplar · Enter Open exact trace span\n",
                "B Capability metrics · R Refresh snapshot\n",
                "c Copy · x Export · Esc Back · q Quit",
            ),
            Self::Usage(_) => concat!(
                "v Chart / list view\n",
                "s Combined / native / span / trace / log reports\n",
                "t Token counters · n Totals · C Cost currency\n",
                "B Usage evidence · R Refresh snapshot\n",
                "c Copy · x Export · Esc Back · q Quit",
            ),
        }
    }

    pub(super) fn draw(&mut self, frame: &mut Frame, pending: bool, message: Option<&str>) {
        match self {
            Self::Metrics(browser) => browser.draw(frame, pending, message),
            Self::Usage(chart) => {
                let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(2)])
                    .areas(frame.area());
                chart.draw(frame, body);
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("B evidence · v list · c copy · x export · Esc back · q quit")
                            .style(label_style()),
                        Line::from(message.map_or_else(
                            || {
                                if pending {
                                    "Loading retained history…".into()
                                } else {
                                    String::new()
                                }
                            },
                            visible_text,
                        )),
                    ]),
                    footer,
                );
            }
        }
    }
}

impl Workspace {
    pub(super) fn finish_metric_read(&mut self, result: Result<WorkspaceResponse, String>) {
        let Some(HistoryAnalytics::Metrics(browser)) = self
            .history
            .last_mut()
            .and_then(HistoryView::active_analytics_mut)
        else {
            return;
        };
        match result {
            Ok(WorkspaceResponse::MetricHistory(page)) => {
                self.message = if browser.apply(page) {
                    None
                } else {
                    Some("Metric history response did not match the pinned selection".into())
                };
            }
            result => {
                browser.cancel_read();
                self.message = match result {
                    Err(error) => Some(error),
                    Ok(_) => Some("History reader returned an unexpected metric response".into()),
                };
            }
        }
    }

    pub(super) fn handle_metric_input(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        input: MetricInput,
        cancel: bool,
    ) -> bool {
        match input {
            MetricInput::Read(query) => {
                self.request(
                    sender,
                    WorkspaceRequest::MetricHistory(query),
                    Change::MetricHistory { cancelled: false },
                );
                if self.pending.is_none()
                    && let Some(HistoryAnalytics::Metrics(browser)) = self
                        .history
                        .last_mut()
                        .and_then(HistoryView::active_analytics_mut)
                {
                    browser.cancel_read();
                }
                true
            }
            MetricInput::Back | MetricInput::Handled => {
                if cancel && matches!(input, MetricInput::Back) {
                    self.history.pop();
                }
                self.message = None;
                true
            }
            MetricInput::Unhandled => false,
        }
    }

    /// These views own local navigation while sharing the global request slot.
    pub(super) fn analytics_key(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        key: event::KeyEvent,
        code: KeyCode,
    ) -> bool {
        let Some(view) = self.history.last_mut() else {
            return false;
        };
        if view.analytics.is_some() && code == KeyCode::Char('B') {
            if self.pending.is_none() {
                view.show_evidence = !view.show_evidence;
                self.message = None;
            }
            return true;
        }
        let cancel = keymap::back(key) || code == KeyCode::Esc;
        let Some(analytics) = view.active_analytics_mut() else {
            return false;
        };
        if matches!(
            key.code,
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab
        ) && !cancel
        {
            return true;
        }
        match analytics {
            HistoryAnalytics::Usage(chart) => {
                if chart.key(code) {
                    self.message = None;
                    return true;
                }
            }
            HistoryAnalytics::Metrics(browser) => {
                if cancel {
                    match &mut self.pending {
                        Some(Change::MetricHistory { cancelled }) if !*cancelled => {
                            *cancelled = true;
                            browser.cancel_read();
                            self.message = None;
                            return true;
                        }
                        None | Some(Change::MetricHistory { .. }) => {}
                        Some(_) => return true,
                    }
                }
                let code = if cancel { KeyCode::Esc } else { code };
                let input = browser.key(code, self.pending.is_some());
                return self.handle_metric_input(sender, input, cancel);
            }
        }
        false
    }
}
