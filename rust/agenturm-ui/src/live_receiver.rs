//! Bounded metadata browser for a receiver without durable Rust read cursors.

use super::*;
use crate::keymap::{self, Destination};
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
            }
            Err(error) => {
                self.page = None;
                self.detail.select(None);
                self.error = Some(error);
            }
        }
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
        frame.render_widget(Block::default().style(surface(CANVAS)), area);
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
                .style(surface(SURFACE)),
            header,
        );
        if self.help {
            frame.render_widget(
                Paragraph::new("1 Agents · 2 Activity · 4 Traces · w Pending\nEnter Session activity · t Session traces\nv Include/hide internal approval reviews\np Pause/resume · R Refresh · Esc Agents · q Quit")
                    .wrap(Wrap { trim: false }).block(panel(HELP_CLOSE_HINT)), body);
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
        let status = if let Some(notice) = self.notice {
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
        let keys = if self.tab == Tab::Agents {
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

/// Interactive view over live pages loaded by one owned worker. The loader
/// must bound each call. A closing terminal discards queued work and replies.
pub fn run_live_receiver(
    mut load: impl FnMut() -> Result<LivePage, String> + Send + 'static,
) -> io::Result<()> {
    let (request, requests) = mpsc::sync_channel(1);
    let (send, results) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let worker = std::thread::Builder::new()
        .name("agenturm-live-receiver".into())
        .spawn(move || {
            while requests.recv().is_ok() && !worker_cancelled.load(Ordering::Acquire) {
                let page = load();
                if worker_cancelled.load(Ordering::Acquire) || send.try_send(page).is_err() {
                    break;
                }
            }
        })?;
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
        };
        let mut next_read = Instant::now();
        let mut explicit_refresh = false;
        let mut dirty = true;
        loop {
            if let Ok(result) = results.try_recv() {
                view.set_page(result);
                next_read = Instant::now() + Duration::from_secs(2);
                dirty = true;
            }
            if !view.pending_read
                && (explicit_refresh || !view.paused && Instant::now() >= next_read)
            {
                request
                    .try_send(())
                    .map_err(|_| io::Error::other("receiver reader stopped"))?;
                view.pending_read = true;
                explicit_refresh = false;
            }
            if dirty {
                terminal.draw(|frame| view.draw(frame))?;
                dirty = false;
            }
            if !event::poll(Duration::from_millis(80))? {
                continue;
            }
            match event::read()? {
                Event::Resize(_, _) => dirty = true,
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
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
    });
    cancelled.store(true, Ordering::Release);
    drop(request);
    drop(results);
    // The in-flight read is bounded by the loader's own deadline. Keep
    // terminal exit independent of the remaining time in that read.
    drop(worker);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

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
