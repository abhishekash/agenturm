//! Keyboard editor and bounded evidence results for the investigation overlay.

use super::WorkAnchor;
use crate::theme;
use crate::{heading_style, label_style, panel, surface, visible_text};
use agenturm_kernel::{ObservationPosition, ReadSnapshot};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthChar;

pub const MAX_INVESTIGATION_QUERY_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvestigationKind {
    Failures,
    Instruction,
    Path,
    Command,
    FailedCommand,
}

impl InvestigationKind {
    const ALL: [Self; 5] = [
        Self::Failures,
        Self::Instruction,
        Self::Path,
        Self::Command,
        Self::FailedCommand,
    ];

    pub fn is_private(self) -> bool {
        self != Self::Failures
    }

    fn label(self) -> &'static str {
        match self {
            Self::Failures => "Failures",
            Self::Instruction => "Instruction",
            Self::Path => "Path",
            Self::Command => "Command",
            Self::FailedCommand => "Failed command",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Failures => "Reported lifecycle, tool, MCP, check, trace and log failures",
            Self::Instruction => "Literal instruction phrase",
            Self::Path => "Exact recorded path",
            Self::Command => "Literal command phrase",
            Self::FailedCommand => "Literal source-confirmed failed command phrase",
        }
    }
}

pub struct InvestigationRead {
    pub kind: InvestigationKind,
    pub query: String,
    pub owner: Option<WorkAnchor>,
    pub history_identity: Option<String>,
    pub snapshot_id: Option<ReadSnapshot>,
    pub cursor: Option<String>,
}

pub struct InvestigationMatch {
    pub position: ObservationPosition,
    pub session: Option<String>,
    pub turn: Option<String>,
    pub worker: Option<String>,
    pub label: &'static str,
    pub source_reference: Option<String>,
}

pub struct InvestigationPage {
    pub rows: Vec<InvestigationMatch>,
    pub history_identity: Option<String>,
    pub snapshot_id: ReadSnapshot,
    pub inspected: usize,
    pub verified: Option<usize>,
    pub unavailable: usize,
    pub next_cursor: Option<String>,
}

pub(crate) enum InvestigationInput {
    Close,
    Read(Box<InvestigationRead>),
    Reveal {
        reference: String,
        history_identity: Option<String>,
    },
    Edited,
    Handled,
}

pub(crate) struct Investigation {
    owner: Option<WorkAnchor>,
    history_identity: Option<String>,
    snapshot_id: Option<ReadSnapshot>,
    all_sources: bool,
    kind: usize,
    query: String,
    page: Option<InvestigationPage>,
    previous: Vec<InvestigationPage>,
    page_number: usize,
    advancing: bool,
    selection: ListState,
}

const MAX_PREVIOUS_PAGES: usize = 4;

impl Investigation {
    pub fn new(
        owner: Option<WorkAnchor>,
        history_identity: Option<String>,
        snapshot_id: Option<ReadSnapshot>,
    ) -> Self {
        Self {
            all_sources: owner.is_none(),
            owner,
            history_identity,
            snapshot_id,
            kind: 0,
            query: String::new(),
            page: None,
            previous: Vec::new(),
            page_number: 0,
            advancing: false,
            selection: ListState::default(),
        }
    }

    pub fn apply(&mut self, page: InvestigationPage) {
        self.snapshot_id.get_or_insert(page.snapshot_id);
        if self.history_identity.is_none() {
            self.history_identity.clone_from(&page.history_identity);
        }
        if self.advancing {
            if let Some(previous) = self.page.take() {
                if self.previous.len() == MAX_PREVIOUS_PAGES {
                    self.previous.remove(0);
                }
                self.previous.push(previous);
            }
            self.page_number = self.page_number.saturating_add(1).max(1);
        } else if self.page.is_none() {
            self.page_number = 1;
        }
        self.advancing = false;
        self.selection = ListState::default().with_selected((!page.rows.is_empty()).then_some(0));
        self.page = Some(page);
    }

    fn reset_pages(&mut self) {
        self.page = None;
        self.previous.clear();
        self.page_number = 0;
        self.advancing = false;
    }

    fn request(&self, cursor: Option<String>) -> InvestigationInput {
        let kind = InvestigationKind::ALL[self.kind];
        let all = !kind.is_private() && self.all_sources;
        InvestigationInput::Read(Box::new(InvestigationRead {
            kind,
            query: if kind.is_private() {
                self.query.clone()
            } else {
                String::new()
            },
            owner: if all { None } else { self.owner.clone() },
            history_identity: self.history_identity.clone(),
            snapshot_id: self.snapshot_id,
            cursor,
        }))
    }

    pub fn key(&mut self, event: KeyEvent, loading: bool) -> InvestigationInput {
        let key = crate::keymap::plain(event).unwrap_or(KeyCode::Null);
        if key == KeyCode::Esc {
            return InvestigationInput::Close;
        }
        if loading {
            return InvestigationInput::Handled;
        }
        let kind = InvestigationKind::ALL[self.kind];
        let private = kind.is_private();
        let query_limit = if kind == InvestigationKind::Path {
            512
        } else {
            MAX_INVESTIGATION_QUERY_BYTES
        };
        match key {
            KeyCode::Tab | KeyCode::BackTab => {
                let modes = InvestigationKind::ALL.len();
                let advance = if key == KeyCode::Tab { 1 } else { modes - 1 };
                self.kind = (self.kind + advance) % modes;
                self.query.clear();
                self.reset_pages();
                return InvestigationInput::Edited;
            }
            KeyCode::Char('s') if !private && self.owner.is_some() => {
                self.all_sources = !self.all_sources;
                self.reset_pages();
                return InvestigationInput::Edited;
            }
            KeyCode::Enter => {
                if let Some(reference) = self
                    .page
                    .as_ref()
                    .and_then(|page| page.rows.get(self.selection.selected()?))
                    .and_then(|row| row.source_reference.as_ref())
                {
                    return InvestigationInput::Reveal {
                        reference: reference.clone(),
                        history_identity: self.history_identity.clone(),
                    };
                }
                self.reset_pages();
                return self.request(None);
            }
            KeyCode::PageDown
                if let Some(next) =
                    self.page.as_ref().and_then(|page| page.next_cursor.clone()) =>
            {
                self.advancing = true;
                return self.request(Some(next));
            }
            KeyCode::PageUp if !self.previous.is_empty() => {
                if let Some(previous) = self.previous.pop() {
                    self.selection = ListState::default()
                        .with_selected((!previous.rows.is_empty()).then_some(0));
                    self.page_number = self.page_number.saturating_sub(1).max(1);
                    self.page = Some(previous);
                }
                return InvestigationInput::Handled;
            }
            KeyCode::Up | KeyCode::Down if let Some(page) = &self.page => {
                crate::workspace::move_selection(
                    &mut self.selection,
                    page.rows.len(),
                    key == KeyCode::Down,
                );
            }
            KeyCode::Backspace if private => {
                self.query.pop();
                self.reset_pages();
                return InvestigationInput::Edited;
            }
            KeyCode::Char(ch)
                if private
                    && !ch.is_control()
                    && self.query.len() + ch.len_utf8() <= query_limit =>
            {
                self.query.push(ch);
                self.reset_pages();
                return InvestigationInput::Edited;
            }
            _ => {}
        }
        InvestigationInput::Handled
    }

    pub fn draw(&mut self, frame: &mut Frame, loading: bool, status: Option<&str>) {
        let area = frame.area();
        if area.width < crate::MIN_WIDTH || area.height < crate::MIN_HEIGHT {
            frame.render_widget(
                Paragraph::new("Terminal too small · resize · Esc back"),
                area,
            );
            return;
        }
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .areas(area);
        let kind = InvestigationKind::ALL[self.kind];
        let scope = if !kind.is_private() && self.all_sources {
            "All authorized sources".to_owned()
        } else if let Some(owner) = &self.owner {
            let mut text = format!(
                "{} / {}",
                visible_text(&owner.source),
                visible_text(&owner.session)
            );
            if let Some(turn) = &owner.turn {
                text.push_str(&format!(" / {}", visible_text(turn)));
            }
            if let Some(worker) = &owner.worker {
                text.push_str(&format!(" / {}", visible_text(worker)));
            }
            text
        } else {
            "Select a session or turn for private searches".into()
        };
        let query = if kind.is_private() && !self.query.is_empty() {
            // Keep the tail of the bounded single-line editor visible.
            let query = visible_text(&self.query);
            let mut cells = usize::from(area.width.saturating_sub(9));
            let mut start = query.len();
            for (index, ch) in query.char_indices().rev() {
                let Some(remaining) = cells.checked_sub(ch.width().unwrap_or(0)) else {
                    break;
                };
                cells = remaining;
                start = index;
            }
            format!("Query  {}▏", &query[start..])
        } else {
            kind.hint().to_owned()
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled("Investigate retained history", heading_style()),
                Line::from(vec![
                    Span::styled(kind.label(), heading_style()),
                    Span::raw("  ·  Tab mode"),
                ]),
                Line::from(scope),
                Line::from(query),
            ])
            .style(surface(theme::palette().surface)),
            header,
        );
        let items = self
            .page
            .iter()
            .flat_map(|page| page.rows.iter())
            .map(|row| {
                let owner = row.session.as_deref().map_or_else(
                    || visible_text(&row.position.harness_id),
                    |session| {
                        format!(
                            "{} / {}",
                            visible_text(&row.position.harness_id),
                            visible_text(session)
                        )
                    },
                );
                let mut occurrence = format!(
                    "{} · rev {}",
                    visible_text(&row.position.event_id),
                    row.position.revision
                );
                if let Some(turn) = &row.turn {
                    occurrence.push_str(&format!(" · turn {}", visible_text(turn)));
                }
                if let Some(worker) = &row.worker {
                    occurrence.push_str(&format!(" · worker {}", visible_text(worker)));
                }
                ListItem::new(vec![
                    Line::from(format!("{} · {owner}", row.label)),
                    Line::styled(occurrence, label_style()),
                ])
            });
        let block = panel(" Evidence metadata ");
        let inner = block.inner(body);
        frame.render_stateful_widget(
            List::new(items)
                .block(block)
                .highlight_symbol("› ")
                .highlight_style(crate::selection_style()),
            body,
            &mut self.selection,
        );
        if self.page.as_ref().is_none_or(|page| page.rows.is_empty()) {
            let empty = if loading {
                "Searching retained evidence…"
            } else if self.page.is_some() {
                "No matches on this bounded page."
            } else {
                "Press Enter to search."
            };
            frame.render_widget(Paragraph::new(empty), inner);
        }
        let coverage = self.page.as_ref().map(|page| {
            let mut text = format!("{} matches / {} inspected", page.rows.len(), page.inspected);
            if let Some(verified) = page.verified {
                text.push_str(&format!(
                    " · {verified} verified / {} unavailable",
                    page.unavailable
                ));
            }
            text
        });
        let status = status
            .map(visible_text)
            .or_else(|| loading.then(|| "Searching retained evidence…".into()))
            .or(coverage)
            .unwrap_or_else(|| "Bounded retained evidence · page-local results".into());
        let snapshot = self.page.as_ref().map_or_else(String::new, |page| {
            format!(
                "Page {} · Snapshot {}{}",
                self.page_number,
                page.snapshot_id,
                if page.next_cursor.is_none() {
                    " · end of scope"
                } else {
                    " · page-local"
                }
            )
        });
        let scope_hint = if !kind.is_private() && self.owner.is_some() {
            " · s scope"
        } else {
            ""
        };
        let mut paging = String::new();
        if !self.previous.is_empty() {
            paging.push_str(" · PgUp previous");
        }
        if self
            .page
            .as_ref()
            .is_some_and(|page| page.next_cursor.is_some())
        {
            paging.push_str(" · PgDn more");
        }
        let enter = if self
            .page
            .as_ref()
            .and_then(|page| page.rows.get(self.selection.selected()?))
            .is_some_and(|row| row.source_reference.is_some())
        {
            "Enter open source"
        } else {
            "Enter search"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(status),
                Line::from(format!("{enter}{scope_hint}{paging} · Esc back")),
                Line::from(snapshot),
            ])
            .wrap(Wrap { trim: true }),
            footer,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::{Terminal, backend::TestBackend};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn matched(event: &str) -> InvestigationMatch {
        InvestigationMatch {
            position: ObservationPosition {
                observed_at_unix_ns: 1,
                harness_id: "harness.fixture".into(),
                event_id: event.into(),
                revision: 0,
                snapshot_id: 0.into(),
            },
            session: Some("session".into()),
            turn: None,
            worker: None,
            label: "Recorded failure",
            source_reference: None,
        }
    }

    fn page(events: &[&str], next: Option<&str>) -> InvestigationPage {
        InvestigationPage {
            rows: events.iter().map(|event| matched(event)).collect(),
            history_identity: Some("history".into()),
            snapshot_id: 0.into(),
            inspected: 5,
            verified: None,
            unavailable: 0,
            next_cursor: next.map(str::to_owned),
        }
    }

    fn screen(investigation: &mut Investigation, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| investigation.draw(frame, false, None))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn paging_keeps_the_bounded_query_and_can_return_to_the_previous_page() {
        let mut investigation = Investigation::new(None, Some("history".into()), Some(0.into()));
        let InvestigationInput::Read(request) = investigation.key(key(KeyCode::Enter), false)
        else {
            panic!("Enter should request the first failures page");
        };
        assert_eq!(request.kind, InvestigationKind::Failures);
        assert_eq!(request.cursor, None);
        investigation.apply(page(&["failure-a"], Some("cursor-1")));
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let text = screen(&mut investigation, width, height);
            for marker in [
                "Page 1",
                "1 matches / 5 inspected",
                "PgDn more",
                "page-local",
            ] {
                assert!(text.contains(marker), "{width}×{height}: {marker}\n{text}");
            }
            assert!(!text.contains("PgUp previous"));
        }

        let InvestigationInput::Read(request) = investigation.key(key(KeyCode::PageDown), false)
        else {
            panic!("PageDown should read the older bounded page");
        };
        assert_eq!(request.cursor.as_deref(), Some("cursor-1"));
        investigation.apply(page(&["failure-b"], None));
        let text = screen(&mut investigation, 80, 24);
        for marker in ["Page 2", "failure-b", "PgUp previous", "end of scope"] {
            assert!(text.contains(marker), "{marker}\n{text}");
        }

        assert!(matches!(
            investigation.key(key(KeyCode::PageUp), false),
            InvestigationInput::Handled
        ));
        let text = screen(&mut investigation, 80, 24);
        assert!(
            text.contains("Page 1") && text.contains("failure-a"),
            "{text}"
        );
        assert!(text.contains("PgDn more") && !text.contains("PgUp previous"));
        assert_eq!(investigation.selection.selected(), Some(0));
        // The boundary is explicit: no further PageUp.
        assert!(matches!(
            investigation.key(key(KeyCode::PageUp), false),
            InvestigationInput::Handled
        ));
        assert!(screen(&mut investigation, 80, 24).contains("Page 1"));
    }

    #[test]
    fn paging_preserves_a_private_query_and_mode_across_pages() {
        let mut investigation = Investigation::new(None, Some("history".into()), Some(0.into()));
        assert!(matches!(
            investigation.key(key(KeyCode::Tab), false),
            InvestigationInput::Edited
        ));
        for ch in "needle".chars() {
            assert!(matches!(
                investigation.key(key(KeyCode::Char(ch)), false),
                InvestigationInput::Edited
            ));
        }
        let InvestigationInput::Read(request) = investigation.key(key(KeyCode::Enter), false)
        else {
            panic!("Enter should run the exact private query");
        };
        assert_eq!(request.kind, InvestigationKind::Instruction);
        assert_eq!(request.query, "needle");
        investigation.apply(page(&["match-a"], Some("next")));
        let InvestigationInput::Read(request) = investigation.key(key(KeyCode::PageDown), false)
        else {
            panic!("PageDown should keep the same private query");
        };
        assert_eq!(request.kind, InvestigationKind::Instruction);
        assert_eq!(request.query, "needle");
        assert_eq!(request.cursor.as_deref(), Some("next"));
        investigation.apply(page(&["match-b"], None));
        assert!(screen(&mut investigation, 60, 18).contains("PgUp previous"));
        investigation.key(key(KeyCode::PageUp), false);
        let text = screen(&mut investigation, 60, 18);
        assert!(
            text.contains("match-a") && !text.contains("match-b"),
            "{text}"
        );
        assert!(matches!(
            investigation.key(key(KeyCode::Esc), false),
            InvestigationInput::Close
        ));
    }
}
