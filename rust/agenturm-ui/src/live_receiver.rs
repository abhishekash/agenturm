//! Bounded metadata browser for a receiver without durable Rust read cursors.

use super::*;
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
    error: Option<String>,
}

impl LiveView {
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
                let selected = self.owner.as_ref().or_else(|| {
                    self.agents
                        .selected()
                        .and_then(|index| self.page.as_ref()?.sessions.get(index)?.owner.as_ref())
                });
                let selection = selected.and_then(|owner| {
                    page.sessions
                        .iter()
                        .position(|row| row.owner.as_ref() == Some(owner))
                });
                let selection = selection.or_else(|| (!page.sessions.is_empty()).then_some(0));
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
        match key {
            KeyCode::Char('1') | KeyCode::Esc => {
                self.tab = Tab::Agents;
                self.owner = None;
            }
            KeyCode::Char('2') => self.tab = Tab::Activity,
            KeyCode::Char('8') => self.tab = Tab::Traces,
            KeyCode::Char('w') => self.tab = Tab::Pending,
            KeyCode::Enter if self.tab == Tab::Agents => {
                if let Some(owner) = self
                    .agents
                    .selected()
                    .and_then(|index| self.rows().get(index)?.owner.clone())
                {
                    self.owner = Some(owner);
                    self.tab = Tab::Activity;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => return self.move_selection(true),
            KeyCode::Up | KeyCode::Char('k') => return self.move_selection(false),
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
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(2),
            Constraint::Length(3),
        ])
        .areas(area);
        let title = format!("Agenturm / {} · Python receiver", self.tab.title());
        let scope = self.owner.as_ref().map_or_else(
            || "Bounded live metadata".to_owned(),
            |owner| format!("{} / {}", owner.service, owner.session_id),
        );
        frame.render_widget(
            Paragraph::new(vec![Line::from(title), Line::from(visible_text(&scope))]),
            header,
        );
        let rows = self.rows();
        let status = if let Some(error) = &self.error {
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
                    .block(Block::bordered()),
                body,
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
            let [list, info] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(body);
            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::bordered())
                    .highlight_symbol("› ")
                    .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
                list,
                cursor,
            );
            frame.render_widget(Paragraph::new(detail), info);
        }
        let keys = if self.tab == Tab::Agents {
            "↑↓ select · Enter session · 2 Activity · 8 Traces · w Pending"
        } else {
            "↑↓ select · 2 Activity · 8 Traces · w Pending · Esc Agents"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(status),
                Line::from(keys),
                Line::from("r refresh · q quit"),
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
            error: None,
        };
        let mut next_read = Instant::now();
        let mut dirty = true;
        loop {
            if let Ok(result) = results.try_recv() {
                view.set_page(result);
                next_read = Instant::now() + Duration::from_secs(2);
                dirty = true;
            }
            if !view.pending_read && Instant::now() >= next_read {
                request
                    .try_send(())
                    .map_err(|_| io::Error::other("receiver reader stopped"))?;
                view.pending_read = true;
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
                    if key.code == KeyCode::Char('q')
                        || key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        break Ok(());
                    }
                    if key.code == KeyCode::Char('r') {
                        next_read = Instant::now();
                    }
                    view.key(key.code);
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
