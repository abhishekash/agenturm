//! Bounded metadata browser for a receiver without durable Rust read cursors.

use super::*;
use crate::keymap::{self, Destination};
use crate::workbench::{PromptKind, PromptReceiver, prompt_channel};
use crate::workspace::prompt::PromptScreen;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveOwner {
    pub service: String,
    pub session_id: String,
    pub agent_id: Option<String>,
}

pub struct LiveRow {
    pub owner: Option<LiveOwner>,
    pub internal_review: bool,
    pub title: String,
    pub detail: Option<String>,
    pub review: Option<LiveReviewTarget>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveReviewTarget {
    pub receiver_id: String,
    pub owner: LiveOwner,
    pub trace_id: String,
    pub turn_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveReviewMode {
    Run,
    Turn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveReviewSelection {
    pub target: LiveReviewTarget,
    pub mode: LiveReviewMode,
}

pub enum LiveRequest {
    Refresh,
    Review {
        selection: Box<LiveReviewSelection>,
        delivery: PromptDelivery,
    },
}

pub enum LiveResponse {
    Page(LivePage),
    ReviewClosed,
}

pub struct LivePage {
    pub sessions: Vec<LiveRow>,
    pub activity: Vec<LiveRow>,
    pub traces: Vec<LiveRow>,
    pub pending: Vec<LiveRow>,
    pub ready: bool,
    pub partial: bool,
    pub attention_ready: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Tab {
    Agents,
    Activity,
    Traces,
    Pending,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Self::Agents => "Agents",
            Self::Activity => "Activity",
            Self::Traces => "Traces",
            Self::Pending => "Pending",
        }
    }
}

struct LiveView {
    page: Option<LivePage>,
    tab: Tab,
    owner: Option<LiveOwner>,
    agents: ListState,
    detail: ListState,
    pending_read: bool,
    paused: bool,
    help: bool,
    include_reviews: bool,
    notice: Option<&'static str>,
    error: Option<String>,
    investigation_notice: bool,
    review_supported: bool,
    review: LiveReviewDisplay,
}

#[derive(Default)]
struct LiveReviewDisplay {
    receiver: Option<PromptReceiver>,
    screen: Option<PromptScreen>,
    redact_then_ack: Option<mpsc::Sender<()>>,
    pending: bool,
    cancelled: bool,
    message: Option<String>,
}

impl LiveReviewDisplay {
    fn receive_frame(&mut self) -> bool {
        let Some(bytes) = self
            .receiver
            .as_ref()
            .and_then(|receiver| receiver.frames.try_recv().ok())
        else {
            return false;
        };
        let receiver = self
            .receiver
            .take()
            .expect("received a queued Review frame");
        match serde_json::from_slice::<String>(&bytes) {
            Ok(body) => self.screen = Some(PromptScreen::new(body, receiver, Instant::now())),
            Err(_) => self.message = Some("Invalid protected Review delivery".into()),
        }
        true
    }

    fn redact(&mut self) -> bool {
        let Some(mut screen) = self.screen.take() else {
            return false;
        };
        self.redact_then_ack = screen.acknowledge.take();
        true
    }

    fn acknowledge_redaction(&mut self) {
        if let Some(acknowledge) = self.redact_then_ack.take() {
            let _ = acknowledge.send(());
        }
    }
}

impl LiveView {
    fn visible_agents(&self) -> impl Iterator<Item = &LiveRow> {
        self.page
            .iter()
            .flat_map(|page| page.sessions.iter())
            .filter(|row| self.include_reviews || !row.internal_review)
    }

    fn selected_agent(&self) -> Option<&LiveRow> {
        self.visible_agents().nth(self.agents.selected()?)
    }

    fn toggle_internal_reviews(&mut self) {
        let selected = self.selected_agent().and_then(|row| row.owner.clone());
        self.include_reviews = !self.include_reviews;
        if !self.include_reviews
            && self.owner.as_ref().is_some_and(|owner| {
                owner.service == "codex-review"
                    || self.page.as_ref().is_some_and(|page| {
                        page.sessions
                            .iter()
                            .any(|row| row.internal_review && row.owner.as_ref() == Some(owner))
                    })
            })
        {
            self.owner = None;
        }
        let next = selected
            .as_ref()
            .and_then(|owner| {
                self.visible_agents()
                    .position(|row| row.owner.as_ref() == Some(owner))
            })
            .or_else(|| self.visible_agents().next().map(|_| 0));
        self.agents.select(next);
        self.detail.select(None);
    }

    fn rows(&self) -> Vec<&LiveRow> {
        self.page
            .as_ref()
            .into_iter()
            .flat_map(|page| match self.tab {
                Tab::Agents => &page.sessions,
                Tab::Activity => &page.activity,
                Tab::Traces => &page.traces,
                Tab::Pending => &page.pending,
            })
            .filter(|row| self.include_reviews || !row.internal_review)
            .filter(|row| {
                self.tab == Tab::Agents
                    || self
                        .owner
                        .as_ref()
                        .is_none_or(|owner| row.owner.as_ref() == Some(owner))
            })
            .collect()
    }

    fn set_page(&mut self, page: Result<LivePage, String>) {
        self.pending_read = false;
        match page {
            Ok(page) => {
                let trace_selection = if self.tab == Tab::Traces {
                    self.detail
                        .selected()
                        .and_then(|index| self.rows().get(index)?.review.clone())
                } else {
                    None
                };
                let selected = self
                    .owner
                    .as_ref()
                    .or_else(|| self.selected_agent()?.owner.as_ref());
                let visible = || {
                    page.sessions
                        .iter()
                        .filter(|row| self.include_reviews || !row.internal_review)
                };
                let selection = selected
                    .and_then(|owner| visible().position(|row| row.owner.as_ref() == Some(owner)))
                    .or_else(|| visible().next().map(|_| 0));
                self.agents.select(selection);
                self.page = Some(page);
                self.error = None;
                if let Some(target) = trace_selection {
                    let selected = self
                        .rows()
                        .iter()
                        .position(|row| row.review.as_ref() == Some(&target));
                    self.detail.select(selected);
                    if selected.is_none() {
                        self.notice = Some("Selected trace is outside the refreshed page");
                    }
                }
            }
            Err(error) => {
                self.page = None;
                self.detail.select(None);
                self.error = Some(error);
            }
        }
    }

    fn review_selection(&self, mode: LiveReviewMode) -> Option<LiveReviewSelection> {
        if !self.review_supported
            || self.tab != Tab::Traces
            || !self.page.as_ref().is_some_and(|page| page.ready)
        {
            return None;
        }
        let rows = self.rows();
        let row = rows.get(self.detail.selected()?)?;
        let target = row.review.as_ref()?;
        if row.owner.as_ref() != Some(&target.owner)
            || mode == LiveReviewMode::Turn && target.turn_id.is_none()
        {
            return None;
        }
        Some(LiveReviewSelection {
            target: target.clone(),
            mode,
        })
    }

    fn key(&mut self, key: KeyCode) {
        if keymap::is_help(key) || (self.help && key == KeyCode::Esc) {
            self.help = !self.help;
            return;
        }
        if self.help {
            return;
        }
        self.notice = None;
        if key == KeyCode::Char('v') {
            self.toggle_internal_reviews();
            return;
        }
        if key == KeyCode::Char('X') {
            self.notice = Some("Attention acknowledgments are unavailable in Python receiver mode");
            return;
        }
        match (keymap::destination(key), key) {
            (Some(Destination::Agents), _) | (_, KeyCode::Esc) => {
                self.tab = Tab::Agents;
                self.owner = None;
            }
            (Some(Destination::Activity), _) => self.tab = Tab::Activity,
            (Some(Destination::Traces), _) | (_, KeyCode::Char('8')) => self.tab = Tab::Traces,
            (_, KeyCode::Char('!' | 'w')) => self.tab = Tab::Pending,
            (_, KeyCode::Char('p')) => self.paused = !self.paused,
            (_, KeyCode::Enter | KeyCode::Char('e' | 's')) if self.tab == Tab::Agents => {
                if let Some(owner) = self
                    .agents
                    .selected()
                    .and_then(|index| self.rows().get(index)?.owner.clone())
                {
                    self.owner = Some(owner);
                    self.tab = Tab::Activity;
                }
            }
            (_, KeyCode::Char('t')) => {
                if self.tab == Tab::Agents {
                    self.owner = self
                        .agents
                        .selected()
                        .and_then(|index| self.rows().get(index)?.owner.clone());
                }
                self.tab = Tab::Traces;
            }
            (_, KeyCode::Down | KeyCode::Char('j')) => return self.move_selection(true),
            (_, KeyCode::Up | KeyCode::Char('k')) => return self.move_selection(false),
            _ => return,
        }
        self.detail.select(None);
    }

    fn move_selection(&mut self, forward: bool) {
        let length = self.rows().len();
        if length == 0 {
            return;
        }
        let cursor = if self.tab == Tab::Agents {
            &mut self.agents
        } else {
            &mut self.detail
        };
        let current = cursor.selected().unwrap_or(0).min(length - 1);
        let selected = if forward {
            (current + 1).min(length - 1)
        } else {
            current.saturating_sub(1)
        };
        cursor.select(Some(selected));
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            frame.render_widget(
                Paragraph::new("Terminal too small · resize or q quit"),
                area,
            );
            return;
        }
        frame.render_widget(
            Block::default().style(surface(theme::palette().canvas)),
            area,
        );
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(2),
            Constraint::Length(3),
        ])
        .areas(area);
        let connection = if self.paused {
            "Paused"
        } else if self.error.is_some() {
            "Reconnecting"
        } else if self.page.is_none() {
            "Connecting"
        } else {
            "Connected"
        };
        let title = Line::from(vec![
            Span::styled(
                format!("Agenturm / {} · Python receiver", self.tab.title()),
                heading_style(),
            ),
            Span::styled(format!("   {connection}"), label_style()),
        ]);
        let tabs = [
            (Tab::Agents, "1 Agents"),
            (Tab::Activity, "2 Activity"),
            (Tab::Traces, "4 Traces"),
            (Tab::Pending, "w Pending"),
        ]
        .into_iter()
        .map(|(tab, label)| {
            Span::styled(
                format!(" {label} "),
                if self.tab == tab {
                    selection_style()
                } else {
                    Style::default()
                },
            )
        })
        .collect::<Vec<_>>();
        let scope = self.owner.as_ref().map_or_else(
            || "Session: All".to_owned(),
            |o| {
                format!(
                    "Session: {} / {}",
                    visible_text(&o.service),
                    visible_text(&o.session_id)
                )
            },
        );
        frame.render_widget(
            Paragraph::new(vec![title, Line::from(tabs), Line::from(scope)])
                .style(surface(theme::palette().surface)),
            header,
        );
        if self.help {
            let mut keys = String::from(
                "1 Agents · 2 Activity · 4 Traces · w Pending\nEnter Session activity · t Session traces\nv Include/hide internal approval reviews\np Pause/resume · R Refresh · Esc Agents · q Quit",
            );
            if self.review_supported {
                keys.push_str("\nCtrl+R Review trace · Ctrl+T Review recorded turn");
            }
            frame.render_widget(
                Paragraph::new(keys)
                    .wrap(Wrap { trim: false })
                    .block(panel(HELP_CLOSE_HINT)),
                body,
            );
            return;
        }
        let [list, info] = if area.width >= 84 {
            Layout::horizontal([Constraint::Percentage(61), Constraint::Percentage(39)]).areas(body)
        } else {
            Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(if area.height >= 14 { 3 } else { 0 }),
            ])
            .areas(body)
        };
        let list_title = if self.tab == Tab::Agents {
            " Sessions "
        } else {
            self.tab.title()
        };
        let inspector = if self.tab == Tab::Agents {
            " Session inspector "
        } else {
            " Selected evidence "
        };
        let rows = self.rows();
        let status = if self.review.pending && !self.review.cancelled {
            "Verifying selected Review · Esc cancel".into()
        } else if let Some(message) = &self.review.message {
            visible_text(message)
        } else if let Some(notice) = self.notice {
            notice.into()
        } else if let Some(error) = &self.error {
            visible_text(error)
        } else if self.page.is_none() {
            "Connecting to Python receiver…".into()
        } else if self.page.as_ref().is_some_and(|page| !page.ready) {
            "Receiver indexing or capture state is not ready".into()
        } else if self.tab == Tab::Pending
            && self.page.as_ref().is_some_and(|page| !page.attention_ready)
        {
            "Request inventory incomplete · state unknown".into()
        } else if self.page.as_ref().is_some_and(|page| page.partial) {
            "Coverage is partial · loaded rows only".into()
        } else {
            "Loaded rows only".into()
        };
        let inactive = self.page.as_ref().is_none_or(|page| !page.ready) || self.error.is_some();
        if rows.is_empty() {
            let message = if inactive
                || self.tab == Tab::Pending
                    && self.page.as_ref().is_some_and(|page| !page.attention_ready)
            {
                &status
            } else {
                "No matching recorded rows on this page."
            };
            frame.render_widget(
                Paragraph::new(message)
                    .wrap(Wrap { trim: true })
                    .block(panel(list_title)),
                list,
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled("Connection", label_style()),
                    Line::from(status.as_str()),
                    Line::from("R retry now"),
                ])
                .wrap(Wrap { trim: false })
                .block(panel(inspector)),
                info,
            );
        } else {
            let selection = if self.tab == Tab::Agents {
                self.agents.selected()
            } else {
                self.detail.selected()
            };
            let index = selection.unwrap_or(0).min(rows.len() - 1);
            let detail = rows[index]
                .detail
                .as_deref()
                .map(visible_text)
                .unwrap_or_default();
            let items = rows
                .iter()
                .map(|row| ListItem::new(visible_text(&row.title)))
                .collect::<Vec<_>>();
            let cursor = if self.tab == Tab::Agents {
                &mut self.agents
            } else {
                &mut self.detail
            };
            cursor.select(Some(index));

            frame.render_stateful_widget(
                List::new(items)
                    .block(panel(list_title))
                    .highlight_symbol("› ")
                    .highlight_style(selection_style()),
                list,
                cursor,
            );
            frame.render_widget(
                Paragraph::new(detail)
                    .wrap(Wrap { trim: false })
                    .block(panel(inspector)),
                info,
            );
        }
        let keys = if self.review_selection(LiveReviewMode::Turn).is_some() {
            "↑↓ select · Ctrl+R Review trace · Ctrl+T Review turn"
        } else if self.review_selection(LiveReviewMode::Run).is_some() {
            "↑↓ select · Ctrl+R Review trace · Esc Agents"
        } else if self.tab == Tab::Agents {
            "↑↓ select · Enter session · 2 Activity · 4 Traces · w Pending"
        } else {
            "↑↓ select · 2 Activity · 4 Traces · w Pending · Esc Agents"
        };
        if self.investigation_notice {
            frame.render_widget(
                Paragraph::new("Investigations require retained Rust history. This receiver supplies live metadata.")
                    .wrap(Wrap { trim: true }),
                footer,
            );
            return;
        }
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(if self.include_reviews && self.notice.is_none() {
                    format!("{status} · Reviews included")
                } else {
                    status
                }),
                Line::from(keys),
                Line::from(if self.paused {
                    "p resume · R refresh · F1 help · q quit"
                } else {
                    "p pause · R refresh · F1 help · q quit"
                }),
            ]),
            footer,
        );
    }
}

/// Interactive view over live metadata pages loaded by one bounded worker.
pub fn run_live_receiver(
    mut load: impl FnMut() -> Result<LivePage, String> + Send + 'static,
) -> io::Result<()> {
    run_live(false, move |request| match request {
        LiveRequest::Refresh => load().map(LiveResponse::Page),
        LiveRequest::Review { .. } => Err("Protected Review is unavailable".into()),
    })
}

/// A bounded receiver loader may explicitly support protected Review reads.
/// A Review callback uses `PromptDelivery::flush_guarded` for a verified frame.
pub fn run_live_receiver_with_review(
    load: impl FnMut(LiveRequest) -> Result<LiveResponse, String> + Send + 'static,
) -> io::Result<()> {
    run_live(true, load)
}

fn run_live(
    review_supported: bool,
    mut load: impl FnMut(LiveRequest) -> Result<LiveResponse, String> + Send + 'static,
) -> io::Result<()> {
    let (request, requests) = mpsc::sync_channel(1);
    let (send, results) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let worker = std::thread::Builder::new()
        .name("agenturm-live-receiver".into())
        .spawn(move || {
            while let Ok(command) = requests.recv() {
                if worker_cancelled.load(Ordering::Acquire) {
                    break;
                }
                let response = load(command);
                if worker_cancelled.load(Ordering::Acquire) || send.try_send(response).is_err() {
                    break;
                }
            }
        })?;
    let mut appearance = theme::Picker::new();
    let result = ratatui::run(|terminal| {
        let mut view = LiveView {
            page: None,
            tab: Tab::Agents,
            owner: None,
            agents: ListState::default(),
            detail: ListState::default(),
            pending_read: false,
            paused: false,
            help: false,
            include_reviews: false,
            notice: None,
            error: None,
            investigation_notice: false,
            review_supported,
            review: LiveReviewDisplay::default(),
        };
        let mut next_read = Instant::now();
        let mut explicit_refresh = false;
        let mut dirty = true;
        let interaction = (|| -> io::Result<()> {
            loop {
                dirty |= view.review.receive_frame();
                if let Ok(result) = results.try_recv() {
                    if view.review.pending {
                        view.review.pending = false;
                        view.review.receiver = None;
                        view.review.redact();
                        if !view.review.cancelled {
                            view.review.message = match result {
                                Ok(LiveResponse::ReviewClosed) => None,
                                Ok(LiveResponse::Page(_)) => {
                                    Some("Unexpected Review response".into())
                                }
                                Err(error) => Some(error),
                            };
                        }
                        view.review.cancelled = false;
                    } else {
                        view.set_page(result.and_then(|reply| match reply {
                            LiveResponse::Page(page) => Ok(page),
                            LiveResponse::ReviewClosed => {
                                Err("Unexpected receiver response".into())
                            }
                        }));
                    }
                    next_read = Instant::now() + Duration::from_secs(2);
                    dirty = true;
                }
                if !view.pending_read
                    && !view.review.pending
                    && (explicit_refresh || !view.paused && Instant::now() >= next_read)
                {
                    request
                        .try_send(LiveRequest::Refresh)
                        .map_err(|_| io::Error::other("receiver reader stopped"))?;
                    view.pending_read = true;
                    explicit_refresh = false;
                }
                if view
                    .review
                    .screen
                    .as_ref()
                    .is_some_and(|screen| screen.expired(Instant::now()))
                {
                    dirty |= view.review.redact();
                }
                let gate = view
                    .review
                    .screen
                    .as_ref()
                    .map(|screen| Arc::clone(&screen.active));
                let guard = gate.as_ref().and_then(|active| active.lock().ok());
                if gate.is_some() && !guard.as_deref().copied().unwrap_or(false) {
                    dirty |= view.review.redact();
                }
                if dirty {
                    terminal.draw(|frame| {
                        if let Some(screen) = &mut view.review.screen {
                            screen.draw(frame);
                        } else {
                            view.draw(frame);
                            appearance.draw(frame);
                        }
                    })?;
                    view.review.acknowledge_redaction();
                    dirty = false;
                }
                drop(guard);
                if !event::poll(Duration::from_millis(80))? {
                    continue;
                }
                match event::read()? {
                    Event::Resize(_, _) => dirty = true,
                    Event::Key(key)
                        if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                    {
                        if !view.review.pending && appearance.key(key) {
                            dirty = true;
                            continue;
                        }
                        let code = keymap::plain(key).unwrap_or_else(|| {
                            if keymap::back(key) {
                                KeyCode::Esc
                            } else {
                                KeyCode::Null
                            }
                        });
                        if keymap::interrupt(key) || code == KeyCode::Char('q') {
                            break Ok(());
                        }
                        if view.review.pending {
                            if code == KeyCode::Esc {
                                if !view.review.redact() {
                                    view.review.receiver = None;
                                    view.review.cancelled = true;
                                }
                                dirty = true;
                            } else if let Some(screen) = &mut view.review.screen {
                                screen.key(code);
                                dirty = true;
                            }
                            continue;
                        }
                        let review_mode = if keymap::review(key) {
                            Some(LiveReviewMode::Run)
                        } else if key.modifiers == KeyModifiers::CONTROL
                            && matches!(key.code, KeyCode::Char('t' | 'T'))
                        {
                            Some(LiveReviewMode::Turn)
                        } else {
                            None
                        };
                        if !view.help
                            && view.review_supported
                            && let Some(mode) = review_mode
                        {
                            view.review.message = if view.pending_read {
                                Some("Wait for the current receiver read".into())
                            } else if let Some(selection) = view.review_selection(mode) {
                                let (delivery, receiver) = prompt_channel(PromptKind::LegacyReview);
                                request
                                    .try_send(LiveRequest::Review {
                                        selection: Box::new(selection),
                                        delivery,
                                    })
                                    .map_err(|_| io::Error::other("receiver reader stopped"))?;
                                view.review.receiver = Some(receiver);
                                view.review.pending = true;
                                None
                            } else {
                                Some(
                                    match mode {
                                        LiveReviewMode::Run => {
                                            "Select an exact retained trace owner for Review"
                                        }
                                        LiveReviewMode::Turn => {
                                            "Select a trace with an exact recorded turn for Review"
                                        }
                                    }
                                    .into(),
                                )
                            };
                            dirty = true;
                            continue;
                        }
                        view.review.message = None;
                        view.investigation_notice = !view.help
                            && key.code == KeyCode::Char('f')
                            && key.modifiers == KeyModifiers::CONTROL;
                        if view.investigation_notice {
                            dirty = true;
                            continue;
                        }
                        if !view.help && matches!(code, KeyCode::Char('R') | keymap::LATEST) {
                            explicit_refresh = true;
                        }
                        view.key(code);
                        dirty = true;
                    }
                    _ => {}
                }
            }
        })();
        // For q, Control-C and recoverable input errors, clear the private
        // frame while the PTY is still active, then release the delivery.
        view.review.receiver = None;
        view.review.redact();
        if view.review.redact_then_ack.is_some() {
            let redrawn = terminal.draw(|frame| view.draw(frame));
            if redrawn.is_ok() {
                view.review.acknowledge_redaction();
            }
            interaction.and(redrawn.map(|_| ()))
        } else {
            interaction
        }
    });
    cancelled.store(true, Ordering::Release);
    drop(request);
    drop(results);
    // In-flight reads retain their own deadlines when the terminal closes.
    drop(worker);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn review_selects_only_the_bound_trace_owner_and_explicit_turn() {
        let owner = LiveOwner {
            service: "opencode".into(),
            session_id: "saved-session".into(),
            agent_id: Some("saved-actor".into()),
        };
        let target = LiveReviewTarget {
            receiver_id: "receiver".into(),
            owner: owner.clone(),
            trace_id: "first-trace".into(),
            turn_id: None,
        };
        let mut sibling = target.clone();
        sibling.trace_id = "sibling-trace".into();
        sibling.turn_id = Some("recorded-turn".into());
        sibling.owner.service = "codex".into();
        let row = |target| LiveRow {
            owner: Some(owner.clone()),
            internal_review: false,
            title: "Visible trace label".into(),
            detail: None,
            review: Some(target),
        };
        let mut view = LiveView {
            page: Some(LivePage {
                sessions: Vec::new(),
                activity: Vec::new(),
                traces: vec![row(target.clone()), row(sibling.clone())],
                pending: Vec::new(),
                ready: true,
                partial: true,
                attention_ready: false,
            }),
            tab: Tab::Traces,
            owner: Some(owner.clone()),
            agents: ListState::default(),
            detail: ListState::default().with_selected(Some(0)),
            pending_read: false,
            paused: false,
            help: false,
            include_reviews: false,
            notice: None,
            error: None,
            investigation_notice: false,
            review_supported: true,
            review: LiveReviewDisplay::default(),
        };
        assert_eq!(
            view.review_selection(LiveReviewMode::Run),
            Some(LiveReviewSelection {
                target: target.clone(),
                mode: LiveReviewMode::Run,
            })
        );
        assert!(view.review_selection(LiveReviewMode::Turn).is_none());
        view.detail.select(Some(1));
        assert!(view.review_selection(LiveReviewMode::Run).is_none());
        view.page.as_mut().unwrap().traces[1].owner = Some(sibling.owner.clone());
        view.owner = None;
        assert_eq!(
            view.review_selection(LiveReviewMode::Turn),
            Some(LiveReviewSelection {
                target: sibling.clone(),
                mode: LiveReviewMode::Turn,
            })
        );
        let exact = LiveRow {
            owner: Some(sibling.owner.clone()),
            internal_review: false,
            title: "Moved after refresh".into(),
            detail: None,
            review: Some(sibling.clone()),
        };
        view.set_page(Ok(LivePage {
            sessions: Vec::new(),
            activity: Vec::new(),
            traces: vec![exact, row(target)],
            pending: Vec::new(),
            ready: true,
            partial: false,
            attention_ready: false,
        }));
        assert_eq!(view.detail.selected(), Some(0));
        assert_eq!(
            view.review_selection(LiveReviewMode::Turn).unwrap().target,
            sibling
        );
        view.page.as_mut().unwrap().ready = false;
        assert!(view.review_selection(LiveReviewMode::Run).is_none());
    }

    #[test]
    fn guarded_review_rechecks_before_display_and_waits_for_redaction_on_revocation() {
        let body = "fixture-private-review-content";
        let (mut refused, receiver) = prompt_channel(PromptKind::LegacyReview);
        serde_json::to_writer(&mut refused, body).unwrap();
        assert_eq!(
            refused.flush_guarded(|_| Err("policy denies preview".into())),
            Err("policy denies preview".into())
        );
        assert!(receiver.frames.try_recv().is_err());

        let (mut delivery, receiver) = prompt_channel(PromptKind::LegacyReview);
        serde_json::to_writer(&mut delivery, body).unwrap();
        let worker = std::thread::spawn(move || {
            let mut checked = false;
            delivery.flush_guarded(|deadline| {
                assert!(deadline > Instant::now());
                if std::mem::replace(&mut checked, true) {
                    Err("receiver capture policy changed".into())
                } else {
                    Ok(())
                }
            })
        });
        let bytes = receiver
            .frames
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let gate = Arc::clone(&receiver.active);
        let mut display = LiveReviewDisplay {
            screen: Some(PromptScreen::new(
                serde_json::from_slice(&bytes).unwrap(),
                receiver,
                Instant::now(),
            )),
            ..LiveReviewDisplay::default()
        };
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| display.screen.as_mut().unwrap().draw(frame))
                .unwrap();
            let text = terminal.backend().to_string();
            assert!(text.contains("verified Review") && text.contains(body));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while *gate.lock().unwrap() {
            assert!(Instant::now() < deadline, "review authority did not close");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(display.redact());
        assert!(!worker.is_finished());
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new("Selected trace"), frame.area()))
            .unwrap();
        assert!(!terminal.backend().to_string().contains(body));
        display.acknowledge_redaction();
        assert_eq!(
            worker.join().unwrap(),
            Err("receiver capture policy changed".into())
        );
    }

    #[test]
    fn explicit_review_visibility_preserves_the_regular_selected_actor() {
        let owner = |service: &str| LiveOwner {
            service: service.into(),
            session_id: "shared-session".into(),
            agent_id: None,
        };
        let regular = owner("codex");
        let review = owner("codex-review");
        let row = |owner: LiveOwner, internal_review| LiveRow {
            title: owner.service.clone(),
            owner: Some(owner),
            internal_review,
            detail: None,
            review: None,
        };
        let mut view = LiveView {
            page: None,
            tab: Tab::Agents,
            owner: None,
            agents: ListState::default(),
            detail: ListState::default(),
            pending_read: false,
            paused: false,
            help: false,
            include_reviews: false,
            notice: None,
            error: None,
            investigation_notice: false,
            review_supported: false,
            review: LiveReviewDisplay::default(),
        };
        view.set_page(Ok(LivePage {
            sessions: vec![row(review.clone(), true), row(regular.clone(), false)],
            activity: vec![row(review, true), row(regular.clone(), false)],
            traces: vec![],
            pending: vec![],
            ready: true,
            partial: false,
            attention_ready: true,
        }));
        assert_eq!(view.rows().len(), 1);
        assert_eq!(
            view.selected_agent().and_then(|row| row.owner.as_ref()),
            Some(&regular)
        );
        view.key(KeyCode::Char('v'));
        assert_eq!(view.rows().len(), 2);
        assert_eq!(
            view.selected_agent().and_then(|row| row.owner.as_ref()),
            Some(&regular)
        );
        view.key(KeyCode::Char('v'));
        assert_eq!(view.rows().len(), 1);
        assert_eq!(
            view.selected_agent().and_then(|row| row.owner.as_ref()),
            Some(&regular)
        );
        view.key(KeyCode::Char('X'));
        assert!(
            view.notice
                .is_some_and(|notice| notice.contains("unavailable"))
        );
        assert_eq!(view.rows().len(), 1);
    }

    #[test]
    fn disconnected_receiver_keeps_navigation_and_retry_visible() {
        let mut view = LiveView {
            page: None,
            tab: Tab::Agents,
            owner: None,
            agents: ListState::default(),
            detail: ListState::default(),
            pending_read: false,
            paused: false,
            help: false,
            include_reviews: false,
            notice: None,
            error: Some("HTTP 404".into()),
            investigation_notice: false,
            review_supported: false,
            review: LiveReviewDisplay::default(),
        };
        for (width, height) in [(120, 32), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| view.draw(frame)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            for expected in [
                "Reconnecting",
                "HTTP 404",
                "Sessions",
                "Session inspector",
                "1 Agents",
                "4 Traces",
                "refresh",
            ] {
                assert!(text.contains(expected), "{expected} missing at {width}");
            }
        }
    }
}
