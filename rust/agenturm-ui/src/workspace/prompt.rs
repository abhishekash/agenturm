//! Integrated request navigator and the private presentation of captured text.
//!
//! The navigator keeps only typed request metadata: exact source/session/worker/
//! turn identity, snapshot and history identity, and a presentation label that
//! the adapter may supply. It never parses a provider envelope. The reader is
//! the existing grant-fenced [`PromptScreen`]; both the full-screen reader and
//! the inspector's inline reader use the same 25-second grant, redaction
//! acknowledgement and readable/original presentation.

// The shared shell dispatch in `workspace.rs`/`keymap.rs` is owned by UI-03.
// Until that integration pass lands, this module's focused tests exercise the
// navigator and reader; the anchors themselves are listed in the UI-04 lane note.
#![allow(dead_code)]

use super::*;
use crate::workbench::{PromptKind, PromptReceiver, WorkAnchor};
use std::borrow::Cow;
use std::collections::BTreeSet;

const DISPLAY_LIFETIME: Duration = Duration::from_secs(25);
const TAB_COLUMNS: usize = 8;

/// Python switches from the wide index to the compact chooser below 120 columns.
pub(crate) const WIDE_PROMPTS_COLUMNS: u16 = 120;

/// A presentation supplied for one captured request.
///
/// `readable` is produced by adapter normalization (a typed context-block list
/// on the wire). When an adapter does not normalize a capture, `readable` stays
/// `None` and the reader labels the readable form unavailable instead of
/// guessing at provider envelopes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PromptPresentation {
    original: String,
    readable: Option<String>,
    hidden_context: usize,
    truncated: bool,
}

impl PromptPresentation {
    /// Compatibility presentation for the existing delivery path, which has no
    /// adapter-normalized block list yet. Kept so a grant acquired before the
    /// adapter seam lands still renders exactly as it does today.
    pub(crate) fn legacy_envelopes(text: String) -> Self {
        let (readable, hidden_context) = readable_request(&text);
        Self {
            readable: Some(readable.to_owned()),
            original: text,
            hidden_context,
            truncated: false,
        }
    }

    pub(crate) fn normalized(
        original: String,
        readable: Option<String>,
        hidden_context: usize,
        truncated: bool,
    ) -> Self {
        Self {
            original,
            readable,
            hidden_context,
            truncated,
        }
    }

    pub(crate) fn original(&self) -> &str {
        &self.original
    }

    pub(crate) fn readable_text(&self) -> Option<&str> {
        self.readable.as_deref()
    }

    pub(crate) fn hidden_context(&self) -> usize {
        self.hidden_context
    }

    pub(crate) fn truncated(&self) -> bool {
        self.truncated
    }

    /// Generic label from the normalized readable body. No provider tag is
    /// interpreted here; an unnormalized capture returns `None`.
    pub(crate) fn title(&self) -> Option<String> {
        title_from(self.readable_text()?)
    }
}

fn title_from(text: &str) -> Option<String> {
    let first = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let first = first.trim_start_matches('#').trim_start();
    Some(if first.is_empty() {
        "Runtime context".into()
    } else {
        first.into()
    })
}

#[derive(Clone, Debug)]
struct RenderedLine {
    text: String,
    source: usize,
}

/// Owns the wrapped/raw layout, reading position and presentation of one
/// captured text. The shell can render it full-screen or inside the inspector.
pub(crate) struct PromptBody {
    presentation: PromptPresentation,
    kind: PromptKind,
    original: bool,
    readable_row: usize,
    original_row: usize,
    column: usize,
    max_row: usize,
    max_column: usize,
    page: usize,
    width: usize,
    lines: Vec<RenderedLine>,
}

impl PromptBody {
    fn new(presentation: PromptPresentation, kind: PromptKind) -> Self {
        Self {
            presentation,
            kind,
            original: kind != PromptKind::User,
            readable_row: 0,
            original_row: 0,
            column: 0,
            max_row: 0,
            max_column: 0,
            page: 1,
            width: 0,
            lines: Vec::new(),
        }
    }

    fn row_mut(&mut self) -> &mut usize {
        if self.original {
            &mut self.original_row
        } else {
            &mut self.readable_row
        }
    }

    fn row(&self) -> usize {
        if self.original {
            self.original_row
        } else {
            self.readable_row
        }
    }

    pub(crate) fn original(&self) -> bool {
        self.original
    }

    fn set_original(&mut self, original: bool) {
        if self.kind == PromptKind::User {
            self.original = original;
        }
    }

    fn toggle(&mut self) {
        self.set_original(!self.original);
    }

    pub(crate) fn truncated(&self) -> bool {
        self.presentation.truncated()
    }

    pub(crate) fn hidden_context(&self) -> usize {
        self.presentation.hidden_context()
    }

    /// One reader key. Request selection belongs to the navigator: these keys
    /// only move the visible text.
    pub(crate) fn key(&mut self, code: KeyCode) {
        let max_row = self.max_row;
        let page = self.page.max(1);
        let row = self.row_mut();
        match code {
            KeyCode::Down | KeyCode::Char('j') => *row = row.saturating_add(1).min(max_row),
            KeyCode::Up | KeyCode::Char('k') => *row = row.saturating_sub(1),
            KeyCode::PageDown => *row = row.saturating_add(page).min(max_row),
            KeyCode::PageUp => *row = row.saturating_sub(page),
            KeyCode::Home => {
                *row = 0;
                if self.original {
                    self.column = 0;
                }
            }
            KeyCode::End => *row = max_row,
            KeyCode::Left if self.original => {
                self.column = self.column.saturating_sub(TAB_COLUMNS);
            }
            KeyCode::Right if self.original => {
                self.column = self.column.saturating_add(TAB_COLUMNS).min(self.max_column);
            }
            _ => {}
        }
    }

    fn display_text(&self) -> Cow<'_, str> {
        if self.original {
            return Cow::Borrowed(self.presentation.original());
        }
        match self.presentation.readable_text() {
            Some("") if self.presentation.hidden_context() != 0 => Cow::Borrowed(
                "This request contains runtime context only. Press z for the captured text.",
            ),
            Some(text) => Cow::Borrowed(text),
            None => Cow::Borrowed(
                "Readable form unavailable · the adapter did not supply a context-block \
                 normalization.\nPress z for the captured text.",
            ),
        }
    }

    fn build(&self, width: usize) -> Vec<RenderedLine> {
        if width == 0 {
            return Vec::new();
        }
        let text = self.display_text();
        if self.original {
            text.split('\n')
                .enumerate()
                .map(|(source, line)| RenderedLine {
                    text: display_line(line),
                    source,
                })
                .collect()
        } else {
            let mut rows = Vec::new();
            for (source, line) in text.split('\n').enumerate() {
                for wrapped in wrap_line(&display_line(line), width) {
                    rows.push(RenderedLine {
                        text: wrapped,
                        source,
                    });
                }
            }
            rows
        }
    }

    fn anchor(&self) -> Option<(usize, usize)> {
        let row = self.row().min(self.lines.len().checked_sub(1)?);
        let source = self.lines.get(row)?.source;
        let start = self.lines.iter().position(|line| line.source == source)?;
        Some((source, row - start))
    }

    fn restore_anchor(&mut self, source: usize, within: usize) {
        let row = self
            .lines
            .iter()
            .position(|line| line.source == source)
            .or_else(|| {
                self.lines
                    .iter()
                    .position(|line| line.source > source)
                    .or_else(|| self.lines.len().checked_sub(1))
            })
            .unwrap_or_default();
        let count = self
            .lines
            .iter()
            .filter(|line| line.source == self.lines[row].source)
            .count();
        let row = row.saturating_add(within.min(count.saturating_sub(1)));
        *self.row_mut() = row;
    }

    /// Rebuilds the layout for the new viewport. A width change keeps the first
    /// visible source line (and the offset within it), so resizing does not jump
    /// to unrelated text when wrapping changes.
    fn viewport(&mut self, width: usize, height: usize) {
        let width = width.max(1);
        let anchor = if width != self.width && !self.lines.is_empty() {
            self.anchor()
        } else {
            None
        };
        self.width = width;
        self.lines = self.build(width);
        self.page = height.max(1);
        self.max_row = self.lines.len().saturating_sub(self.page);
        if let Some((source, within)) = anchor {
            self.restore_anchor(source, within);
        }
        let max_row = self.max_row;
        let row = self.row_mut();
        *row = (*row).min(max_row);
        self.max_column = self
            .lines
            .iter()
            .map(|line| usize::from(line.text.as_str().cell_width()))
            .max()
            .unwrap_or_default()
            .saturating_sub(width);
        self.column = self.column.min(self.max_column);
    }

    pub(crate) fn first_visible_line(&self) -> Option<&str> {
        self.lines.get(self.row()).map(|line| line.text.as_str())
    }
}

/// Terminal presentation of text already delivered under a content grant.
pub(crate) struct PromptScreen {
    body: PromptBody,
    caption: Option<String>,
    pub(crate) acknowledge: Option<mpsc::Sender<()>>,
    pub(crate) active: Arc<Mutex<bool>>,
    guarded_active: Arc<AtomicBool>,
    expires: Instant,
}

impl PromptScreen {
    pub(crate) fn new(text: String, receiver: PromptReceiver, now: Instant) -> Self {
        Self::new_with_presentation(PromptPresentation::legacy_envelopes(text), receiver, now)
    }

    pub(crate) fn new_with_presentation(
        presentation: PromptPresentation,
        receiver: PromptReceiver,
        now: Instant,
    ) -> Self {
        Self {
            body: PromptBody::new(presentation, receiver.kind),
            caption: None,
            acknowledge: Some(receiver.acknowledgement),
            active: receiver.active,
            guarded_active: receiver.guarded_active,
            expires: now + DISPLAY_LIFETIME,
        }
    }

    pub(crate) fn expired(&self, now: Instant) -> bool {
        now >= self.expires || !self.guarded_active.load(Ordering::Acquire)
    }

    pub(crate) fn expires_at(&self) -> Instant {
        self.expires
    }

    pub(crate) fn set_caption(&mut self, caption: Option<String>) {
        self.caption = caption;
    }

    pub(crate) fn caption(&self) -> Option<&str> {
        self.caption.as_deref()
    }

    pub(crate) fn original(&self) -> bool {
        self.body.original()
    }

    pub(crate) fn set_original(&mut self, original: bool) {
        self.body.set_original(original);
    }

    pub(crate) fn truncated(&self) -> bool {
        self.body.truncated()
    }

    pub(crate) fn hidden_context(&self) -> usize {
        self.body.hidden_context()
    }

    pub(crate) fn title(&self) -> Option<String> {
        self.body.presentation.title()
    }

    pub(crate) fn first_visible_line(&self) -> Option<&str> {
        self.body.first_visible_line()
    }

    pub(crate) fn key(&mut self, code: KeyCode) {
        if code == KeyCode::Char('z') && self.body.kind == PromptKind::User {
            self.body.toggle();
            return;
        }
        self.body.key(code);
    }

    /// Draws the private screen over the whole terminal.
    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        let footer_lines = if self.body.kind == PromptKind::User {
            2
        } else {
            1
        };
        let [main, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(footer_lines)])
            .areas(frame.area());
        let title = match (self.body.kind, self.body.original) {
            (PromptKind::User, true) if main.width >= 51 => {
                "Granted prompt · captured text · original layout".to_owned()
            }
            (PromptKind::User, true) => "Granted prompt · original layout".to_owned(),
            (PromptKind::User, false) => "Granted prompt · readable".to_owned(),
            (kind, _) => kind.title().to_owned(),
        };
        let title = self
            .caption
            .as_ref()
            .map_or(title.clone(), |caption| format!("{title} · {caption}"));
        self.draw_region(frame, main, title);

        let keys = match (self.body.kind, self.body.original) {
            (PromptKind::User, true) => "z Readable · ←→ pan · ↑↓/PgUp/PgDn scroll · Esc close",
            (PromptKind::User, false) => "z Original layout · ↑↓/PgUp/PgDn scroll · Esc close",
            _ => "↑↓/PgUp/PgDn scroll · Esc close · closes after 25s",
        };
        let status = if self.body.kind == PromptKind::User && self.body.original {
            format!("Column {} · closes after 25s", self.body.column + 1)
        } else if self.body.truncated() {
            "Reader limit: text clipped · open Activity for the captured record · closes after 25s"
                .into()
        } else if self.body.hidden_context() == 1 {
            "1 runtime context block hidden · closes after 25s".into()
        } else if self.body.hidden_context() > 1 {
            format!(
                "{} runtime context blocks hidden · closes after 25s",
                self.body.hidden_context()
            )
        } else {
            "Closes after 25s".into()
        };
        frame.render_widget(
            Paragraph::new(if self.body.kind == PromptKind::User {
                vec![Line::from(keys), Line::from(status)]
            } else {
                vec![Line::from(keys)]
            }),
            foot,
        );
    }

    /// Draws the same granted body inside a caller-provided region. The caller
    /// remains responsible for the grant deadline, redaction redraw and
    /// acknowledgement (see the lane note for the exact shell hooks).
    pub(crate) fn draw_region(&mut self, frame: &mut Frame, area: Rect, title: String) {
        let block = panel(title);
        let interior = block.inner(area);
        let width = usize::from(interior.width);
        let height = usize::from(interior.height);
        if width == 0 || height == 0 {
            frame.render_widget(block, area);
            return;
        }
        self.body.viewport(width, height);
        let row = self.body.row();
        let column = if self.body.original {
            self.body.column
        } else {
            0
        };
        let visible = self.body.lines.iter().skip(row).take(height).map(|line| {
            Line::from(if self.body.original {
                slice_line(&line.text, column, width)
            } else {
                line.text.clone()
            })
        });
        frame.render_widget(
            Paragraph::new(visible.collect::<Vec<_>>()).block(block),
            area,
        );
    }
}

/// Exact captured request metadata kept by the navigation model.
#[derive(Clone, Debug)]
pub(crate) struct PromptRequest {
    pub(crate) anchor: WorkAnchor,
    pub(crate) event_id: String,
    pub(crate) revision: u32,
    pub(crate) observed_at_unix_ns: u64,
    pub(crate) snapshot_id: Option<ReadSnapshot>,
    pub(crate) history_identity: Option<String>,
    /// Adapter-normalized label. `None` until the adapter (or a granted
    /// presentation) supplies one; the navigator never invents request text.
    pub(crate) title: Option<String>,
    pub(crate) hidden_context: usize,
    pub(crate) truncated: bool,
}

impl From<&PromptEntry> for PromptRequest {
    fn from(entry: &PromptEntry) -> Self {
        Self {
            anchor: entry.anchor.clone(),
            event_id: entry.event_id.clone(),
            revision: entry.revision,
            observed_at_unix_ns: entry.timestamp,
            snapshot_id: entry.snapshot_id,
            history_identity: entry.history_identity.clone(),
            title: None,
            hidden_context: 0,
            truncated: false,
        }
    }
}

impl PromptRequest {
    pub(crate) fn key(&self) -> (String, u32) {
        (self.event_id.clone(), self.revision)
    }

    fn entry(&self) -> PromptEntry {
        PromptEntry {
            anchor: self.anchor.clone(),
            event_id: self.event_id.clone(),
            revision: self.revision,
            timestamp: self.observed_at_unix_ns,
            snapshot_id: self.snapshot_id,
            history_identity: self.history_identity.clone(),
        }
    }

    fn label(&self) -> String {
        self.title
            .as_deref()
            .map(visible_text)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| {
                self.anchor.turn.as_deref().map_or_else(
                    || "Recorded request".into(),
                    |turn| format!("turn {}", visible_text(turn)),
                )
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptCapture {
    Enabled,
    Disabled,
    MetadataOnly,
}

/// The exact scope every list and read is checked against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PromptScope {
    pub(crate) source: String,
    pub(crate) session: String,
    pub(crate) worker: Option<String>,
    pub(crate) turn: Option<String>,
    pub(crate) capture: PromptCapture,
    /// Only navigated to when the caller verified it belongs to this session.
    pub(crate) main_session: Option<WorkAnchor>,
}

impl PromptScope {
    pub(crate) fn session(anchor: &WorkAnchor, capture: PromptCapture) -> Self {
        Self {
            source: anchor.source.clone(),
            session: anchor.session.clone(),
            worker: anchor.worker.clone(),
            turn: anchor.turn.clone(),
            capture,
            main_session: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptFocus {
    Navigator,
    Chooser,
    Reader,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PromptStatus {
    Loading {
        older: bool,
    },
    Loaded {
        loaded: usize,
        retained_cap: Option<usize>,
        earlier_available: bool,
        limited: bool,
    },
    EmptyScope,
    CaptureDisabled,
    MissingIdentity,
    Denied(String),
    Revoked,
    Expired,
    MissingCapture,
    OutOfScope(String),
    Stale(String),
    Error(String),
    Unavailable(String),
}

impl PromptStatus {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Loading { older: true } => "Loading earlier requests…".into(),
            Self::Loading { older: false } => "Loading prompt history…".into(),
            Self::Loaded {
                loaded,
                retained_cap,
                earlier_available,
                limited,
            } => {
                if *limited
                    && let Some(cap) = retained_cap
                {
                    return format!("Retained window cap {cap} · older history in Activity");
                }
                let mut message = format!("{loaded} loaded · oldest first");
                if *earlier_available {
                    message.push_str(" · earlier available");
                }
                message
            }
            Self::EmptyScope => "No user prompts were captured for this session.".into(),
            Self::CaptureDisabled => "Content capture is disabled for this source.".into(),
            Self::MissingIdentity => "Prompt history needs a session or agent ID.".into(),
            Self::Denied(reason) => format!("Prompt access denied · {reason}"),
            Self::Revoked => "Prompt access was revoked · captured text cleared.".into(),
            Self::Expired => "The prompt grant expired · captured text cleared.".into(),
            Self::MissingCapture => {
                "The selected request is no longer in the captured source · older history is in Activity."
                    .into()
            }
            Self::OutOfScope(reason) => format!("That read is outside the selected request · {reason}"),
            Self::Stale(reason) => format!("The captured source changed · {reason}"),
            Self::Error(reason) => format!("Could not load prompts · {reason}"),
            Self::Unavailable(reason) => format!("Prompt history is unavailable · {reason}"),
        }
    }
}

/// Retained window facts supplied with a page. The navigator never claims a
/// complete transcript: `limited` says the adapter's cap was reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub(crate) struct PromptPageMeta {
    pub(crate) retained_cap: Option<usize>,
    pub(crate) earlier_available: bool,
    pub(crate) limited: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PromptAccepted {
    Accepted,
    Late(PromptLate),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptLate {
    /// The reader was closed or the section navigated away before delivery.
    Superseded,
    /// The delivery does not match the selected request or exact scope.
    OutOfScope,
}

/// What the shell must do next. The navigator never performs a read itself.
#[derive(Debug)]
pub(crate) enum PromptInput {
    Ignored,
    Handled,
    Read {
        entry: PromptEntry,
        original: bool,
        /// Echo this epoch when the delivery arrives.
        epoch: u64,
    },
    /// Return one layer: reader -> navigator/chooser was already handled; this
    /// one leaves the Prompts section for the inspector tabs.
    Back,
    ReaderClosed,
    MainSession(WorkAnchor),
    LoadEarlier,
    Retry,
    NoRequests,
}

#[derive(Clone, Debug, Default)]
struct PromptChooser {
    open: bool,
    highlighted: usize,
}

pub(crate) struct PromptNavigator {
    scope: PromptScope,
    requests: Vec<PromptRequest>,
    selected: usize,
    focus: PromptFocus,
    chooser: PromptChooser,
    reader: Option<PromptScreen>,
    reader_key: Option<(String, u32)>,
    reading: bool,
    original: BTreeSet<(String, u32)>,
    status: PromptStatus,
    notice: Option<String>,
    retained_cap: Option<usize>,
    earlier_available: bool,
    limited: bool,
    load_generation: u64,
    loading_older: bool,
    reader_epoch: u64,
    index_offset: usize,
    wide: bool,
    page_size: usize,
}

impl PromptNavigator {
    pub(crate) fn new(scope: PromptScope) -> Self {
        let focus = PromptFocus::Navigator;
        Self {
            scope,
            requests: Vec::new(),
            selected: 0,
            focus,
            chooser: PromptChooser::default(),
            reader: None,
            reader_key: None,
            reading: false,
            original: BTreeSet::new(),
            status: PromptStatus::Loading { older: false },
            notice: None,
            retained_cap: None,
            earlier_available: false,
            limited: false,
            load_generation: 0,
            loading_older: false,
            reader_epoch: 0,
            index_offset: 0,
            wide: true,
            page_size: 0,
        }
    }

    pub(crate) fn scope(&self) -> &PromptScope {
        &self.scope
    }

    pub(crate) fn set_capture(&mut self, capture: PromptCapture) {
        self.scope.capture = capture;
    }

    pub(crate) fn focus(&self) -> PromptFocus {
        self.focus
    }

    pub(crate) fn status(&self) -> &PromptStatus {
        &self.status
    }

    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub(crate) fn requests(&self) -> &[PromptRequest] {
        &self.requests
    }

    pub(crate) fn selected_index(&self) -> Option<usize> {
        (!self.requests.is_empty()).then_some(self.selected)
    }

    pub(crate) fn selected_request(&self) -> Option<&PromptRequest> {
        self.requests.get(self.selected)
    }

    pub(crate) fn selected_ordinal(&self) -> Option<usize> {
        self.selected_request().map(|_| self.selected + 1)
    }

    pub(crate) fn set_page_size(&mut self, page: usize) {
        self.page_size = page.max(1);
    }

    pub(crate) fn page_size(&self) -> usize {
        self.page_size
    }

    pub(crate) fn begin_load(&mut self) -> u64 {
        self.load_generation = self.load_generation.wrapping_add(1);
        self.loading_older = false;
        self.status = PromptStatus::Loading { older: false };
        self.load_generation
    }

    /// Loads older retained references. Returns `None` when there is nothing to
    /// ask for; the shell must not invent a page request.
    pub(crate) fn begin_older(&mut self) -> Option<u64> {
        if !self.earlier_available
            || self.loading_older
            || matches!(self.status, PromptStatus::Loading { .. })
        {
            return None;
        }
        self.load_generation = self.load_generation.wrapping_add(1);
        self.loading_older = true;
        self.status = PromptStatus::Loading { older: true };
        Some(self.load_generation)
    }

    pub(crate) fn begin_retry(&mut self) -> u64 {
        self.begin_load()
    }

    pub(crate) fn retry_available(&self) -> bool {
        matches!(
            self.status,
            PromptStatus::Denied(_)
                | PromptStatus::Unavailable(_)
                | PromptStatus::Error(_)
                | PromptStatus::MissingIdentity
        )
    }

    fn in_scope(&self, request: &PromptRequest) -> bool {
        request.anchor.source == self.scope.source
            && request.anchor.session == self.scope.session
            && request.anchor.worker == self.scope.worker
            && self
                .scope
                .turn
                .as_ref()
                .is_none_or(|turn| request.anchor.turn.as_ref() == Some(turn))
    }

    fn merge(&mut self, mut incoming: Vec<PromptRequest>, older: bool) {
        if !older {
            self.requests.clear();
        }
        self.requests.append(&mut incoming);
        self.requests.sort_by(|left, right| {
            left.observed_at_unix_ns
                .cmp(&right.observed_at_unix_ns)
                .then_with(|| left.event_id.cmp(&right.event_id))
                .then_with(|| left.revision.cmp(&right.revision))
        });
        self.requests
            .dedup_by(|left, right| left.key() == right.key());
    }

    /// Accepts one bounded page. `token` is the value returned by
    /// [`Self::begin_load`]/[`Self::begin_older`]; a superseded token is
    /// discarded with a visible reason.
    pub(crate) fn accept_page(
        &mut self,
        token: u64,
        requests: Vec<PromptRequest>,
        meta: PromptPageMeta,
    ) -> bool {
        if token != self.load_generation {
            self.notice = Some("Discarded a late prompt response for a superseded view.".into());
            return false;
        }
        let older = self.loading_older;
        let selected_key = self.selected_request().map(PromptRequest::key);
        let previous = self.selected;
        let (requests, outside): (Vec<_>, Vec<_>) = requests
            .into_iter()
            .partition(|request| self.in_scope(request));
        if !outside.is_empty() {
            self.notice = Some(format!(
                "Ignored {} request(s) outside the selected source, session or worker scope.",
                outside.len()
            ));
        }
        self.merge(requests, older);
        if self.requests.is_empty() {
            self.selected = 0;
            self.status = PromptStatus::EmptyScope;
            self.earlier_available = false;
            return true;
        }
        self.selected = match selected_key.as_ref() {
            Some(key) => self
                .requests
                .iter()
                .position(|row| row.key() == *key)
                .unwrap_or_else(|| {
                    self.notice = Some(
                        "The selected request is no longer recorded · kept the nearest request."
                            .into(),
                    );
                    previous.min(self.requests.len() - 1)
                }),
            // No prior selection: newest first, as Python opens Prompts.
            None => self.requests.len() - 1,
        };
        self.retained_cap = meta.retained_cap;
        self.earlier_available = meta.earlier_available;
        self.limited = meta.limited;
        self.loading_older = false;
        self.status = PromptStatus::Loaded {
            loaded: self.requests.len(),
            retained_cap: self.retained_cap,
            earlier_available: self.earlier_available,
            limited: self.limited,
        };
        true
    }

    pub(crate) fn deny(&mut self, reason: impl Into<String>) {
        self.loading_older = false;
        self.status = PromptStatus::Denied(reason.into());
    }

    pub(crate) fn unavailable(&mut self, reason: impl Into<String>) {
        self.loading_older = false;
        self.status = PromptStatus::Unavailable(reason.into());
    }

    pub(crate) fn capture_disabled(&mut self) {
        self.status = PromptStatus::CaptureDisabled;
    }

    pub(crate) fn missing_identity(&mut self) {
        self.status = PromptStatus::MissingIdentity;
    }

    pub(crate) fn missing_capture(&mut self) {
        self.status = PromptStatus::MissingCapture;
    }

    pub(crate) fn out_of_scope(&mut self, reason: impl Into<String>) {
        self.status = PromptStatus::OutOfScope(reason.into());
    }

    pub(crate) fn stale(&mut self, reason: impl Into<String>) {
        self.loading_older = false;
        self.status = PromptStatus::Stale(reason.into());
    }

    pub(crate) fn fail(&mut self, reason: impl Into<String>) {
        self.status = PromptStatus::Error(reason.into());
    }

    /// Disclosure shown next to the navigator, separate from the loaded status.
    pub(crate) fn disclosure(&self) -> Option<String> {
        if self.scope.capture == PromptCapture::Disabled {
            return Some("Content capture is disabled for this source.".into());
        }
        if self.scope.capture == PromptCapture::MetadataOnly {
            return Some("Metadata only · captured text is unavailable for this source.".into());
        }
        self.scope.worker.as_deref().map(|worker| {
            let mut disclosure = format!(
                "Worker-scoped · exact worker {} · main-session requests are not included.",
                visible_text(worker)
            );
            if self.scope.main_session.is_some() {
                disclosure.push_str(" m Main session");
            }
            disclosure
        })
    }

    pub(crate) fn set_layout(&mut self, width: u16) {
        let wide = width >= WIDE_PROMPTS_COLUMNS;
        if wide == self.wide {
            return;
        }
        self.wide = wide;
        if self.reader.is_some() {
            // A focused reader stays a reader across the breakpoint.
            return;
        }
        match (self.focus, wide) {
            (PromptFocus::Navigator, false) => self.focus = PromptFocus::Chooser,
            (PromptFocus::Chooser, true) => {
                self.chooser.open = false;
                self.focus = PromptFocus::Navigator;
            }
            _ => {}
        }
    }

    pub(crate) fn is_wide(&self) -> bool {
        self.wide
    }

    fn default_focus(&self) -> PromptFocus {
        if self.wide {
            PromptFocus::Navigator
        } else {
            PromptFocus::Chooser
        }
    }

    fn preferred_original(&self, key: &(String, u32)) -> bool {
        self.original.contains(key)
    }

    fn current_key(&self) -> Option<(String, u32)> {
        self.selected_request().map(PromptRequest::key)
    }

    fn read(&mut self, original: bool) -> PromptInput {
        if self.scope.capture != PromptCapture::Enabled {
            return PromptInput::NoRequests;
        }
        let Some(request) = self.selected_request() else {
            return PromptInput::NoRequests;
        };
        let entry = request.entry();
        // A new request must not keep the previous body.
        self.reader = None;
        self.reader_key = None;
        self.reading = true;
        self.focus = PromptFocus::Reader;
        PromptInput::Read {
            entry,
            original,
            epoch: self.reader_epoch,
        }
    }

    fn select(&mut self, index: usize) {
        if self.requests.is_empty() {
            self.selected = 0;
        } else {
            self.selected = index.min(self.requests.len() - 1);
        }
    }

    fn step(&mut self, older: bool) -> bool {
        if self.requests.is_empty() {
            return false;
        }
        if older {
            if self.selected == 0 {
                return false;
            }
            self.selected -= 1;
        } else {
            if self.selected + 1 >= self.requests.len() {
                return false;
            }
            self.selected += 1;
        }
        true
    }

    fn select_edge(&mut self, older: bool) {
        if self.requests.is_empty() {
            return;
        }
        self.selected = if older { 0 } else { self.requests.len() - 1 };
    }

    fn toggle_original(&mut self) {
        let Some(key) = self.current_key() else {
            return;
        };
        if self.original.contains(&key) {
            self.original.remove(&key);
        } else {
            self.original.insert(key.clone());
        }
        let original = self.original.contains(&key);
        if self.reader_key.as_ref() == Some(&key)
            && let Some(reader) = &mut self.reader
        {
            reader.set_original(original);
        }
    }

    fn reopen(&mut self) -> PromptInput {
        let original = self
            .current_key()
            .is_some_and(|key| self.preferred_original(&key));
        self.read(original)
    }

    /// Routes one key while the prompt navigator, chooser or reader owns focus.
    /// The shell must not send this key to the history-page actions.
    pub(crate) fn key(&mut self, code: KeyCode) -> PromptInput {
        if self.focus != PromptFocus::Reader && !self.chooser.open {
            match code {
                KeyCode::PageUp if self.earlier_available => return PromptInput::LoadEarlier,
                KeyCode::Char('R') if self.retry_available() => return PromptInput::Retry,
                _ => {}
            }
        }
        if code == KeyCode::Char('m')
            && self.scope.worker.is_some()
            && let Some(main) = &self.scope.main_session
        {
            return PromptInput::MainSession(main.clone());
        }
        match self.focus {
            PromptFocus::Reader => match code {
                KeyCode::Left | KeyCode::Esc => {
                    self.close_reader();
                    PromptInput::ReaderClosed
                }
                KeyCode::Char('z') => {
                    self.toggle_original();
                    PromptInput::Handled
                }
                KeyCode::Char('[') => {
                    self.step(true);
                    self.reopen()
                }
                KeyCode::Char(']') => {
                    self.step(false);
                    self.reopen()
                }
                KeyCode::Char('l') => {
                    self.select_edge(false);
                    self.reopen()
                }
                _ => {
                    if let Some(reader) = &mut self.reader {
                        reader.key(code);
                        PromptInput::Handled
                    } else {
                        PromptInput::Ignored
                    }
                }
            },
            PromptFocus::Chooser => self.chooser_key(code),
            PromptFocus::Navigator => match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.step(true);
                    PromptInput::Handled
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.step(false);
                    PromptInput::Handled
                }
                KeyCode::Home => {
                    self.select_edge(true);
                    PromptInput::Handled
                }
                KeyCode::End => {
                    self.select_edge(false);
                    PromptInput::Handled
                }
                KeyCode::Enter | KeyCode::Right => self.read(
                    self.current_key()
                        .is_some_and(|key| self.preferred_original(&key)),
                ),
                KeyCode::Char('[') => {
                    self.step(true);
                    PromptInput::Handled
                }
                KeyCode::Char(']') => {
                    self.step(false);
                    PromptInput::Handled
                }
                KeyCode::Char('l') => {
                    self.select_edge(false);
                    PromptInput::Handled
                }
                KeyCode::Char('z') => {
                    self.toggle_original();
                    PromptInput::Handled
                }
                KeyCode::Left | KeyCode::Esc => PromptInput::Back,
                _ => PromptInput::Ignored,
            },
        }
    }

    fn chooser_key(&mut self, code: KeyCode) -> PromptInput {
        if self.chooser.open {
            match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.chooser.highlighted = self.chooser.highlighted.saturating_sub(1);
                    PromptInput::Handled
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.chooser.highlighted = self
                        .chooser
                        .highlighted
                        .saturating_add(1)
                        .min(self.requests.len().saturating_sub(1));
                    PromptInput::Handled
                }
                KeyCode::Home => {
                    self.chooser.highlighted = 0;
                    PromptInput::Handled
                }
                KeyCode::End => {
                    self.chooser.highlighted = self.requests.len().saturating_sub(1);
                    PromptInput::Handled
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if self.requests.is_empty() {
                        self.chooser.open = false;
                        return PromptInput::NoRequests;
                    }
                    self.selected = self.chooser.highlighted;
                    self.chooser.open = false;
                    // Confirming the current value reads too.
                    self.reopen()
                }
                KeyCode::Esc => {
                    self.chooser.open = false;
                    PromptInput::Handled
                }
                _ => PromptInput::Handled,
            }
        } else {
            match code {
                KeyCode::Up | KeyCode::Down | KeyCode::Enter | KeyCode::Char(' ') => {
                    if self.requests.is_empty() {
                        return PromptInput::NoRequests;
                    }
                    self.chooser.open = true;
                    self.chooser.highlighted = self.selected;
                    PromptInput::Handled
                }
                KeyCode::Home => {
                    self.select_edge(true);
                    PromptInput::Handled
                }
                KeyCode::End => {
                    self.select_edge(false);
                    PromptInput::Handled
                }
                KeyCode::Right => self.read(
                    self.current_key()
                        .is_some_and(|key| self.preferred_original(&key)),
                ),
                KeyCode::Char('[') => {
                    self.step(true);
                    self.reopen()
                }
                KeyCode::Char(']') => {
                    self.step(false);
                    self.reopen()
                }
                KeyCode::Char('l') => {
                    self.select_edge(false);
                    self.reopen()
                }
                KeyCode::Char('z') => {
                    self.toggle_original();
                    PromptInput::Handled
                }
                KeyCode::Left | KeyCode::Esc => PromptInput::Back,
                _ => PromptInput::Ignored,
            }
        }
    }

    pub(crate) fn chooser_open(&self) -> bool {
        self.chooser.open
    }

    pub(crate) fn highlighted(&self) -> usize {
        self.chooser.highlighted
    }

    pub(crate) fn reader_open(&self) -> bool {
        self.reader.is_some()
    }

    pub(crate) fn read_pending(&self) -> bool {
        self.reading
    }

    pub(crate) fn pending_identity(&self, epoch: u64, key: &(String, u32)) -> bool {
        self.reading && self.reader_epoch == epoch && self.current_key().as_ref() == Some(key)
    }

    pub(crate) fn reader(&mut self) -> Option<&mut PromptScreen> {
        self.reader.as_mut()
    }

    pub(crate) fn reader_ref(&self) -> Option<&PromptScreen> {
        self.reader.as_ref()
    }

    /// Removes the granted body. The caller must redraw before acknowledging
    /// redaction, exactly as the full-screen path does.
    pub(crate) fn take_reader(&mut self) -> Option<PromptScreen> {
        self.reader_epoch = self.reader_epoch.wrapping_add(1);
        self.reader_key = None;
        self.reading = false;
        self.reader.take()
    }

    pub(crate) fn close_reader(&mut self) -> Option<PromptScreen> {
        let reader = self.take_reader();
        self.focus = self.default_focus();
        reader
    }

    /// Delivery of a granted body. Rejects a superseded epoch or an identity
    /// that is not the exact selected request.
    pub(crate) fn accept_reader(
        &mut self,
        epoch: u64,
        key: (String, u32),
        text: String,
        receiver: PromptReceiver,
        now: Instant,
    ) -> PromptAccepted {
        self.accept_reader_presentation(
            epoch,
            key,
            PromptPresentation::normalized(text, None, 0, false),
            receiver,
            now,
        )
    }

    pub(crate) fn accept_reader_presentation(
        &mut self,
        epoch: u64,
        key: (String, u32),
        presentation: PromptPresentation,
        receiver: PromptReceiver,
        now: Instant,
    ) -> PromptAccepted {
        if epoch != self.reader_epoch {
            self.reading = false;
            self.notice = Some("Discarded a late prompt delivery for a closed reader.".into());
            return PromptAccepted::Late(PromptLate::Superseded);
        }
        if self.current_key().as_ref() != Some(&key) {
            self.reading = false;
            self.notice =
                Some("Discarded a prompt delivery that is outside the selected request.".into());
            return PromptAccepted::Late(PromptLate::OutOfScope);
        }
        let hidden = presentation.hidden_context();
        let truncated = presentation.truncated();
        let original = self.preferred_original(&key);
        let mut reader = PromptScreen::new_with_presentation(presentation, receiver, now);
        reader.set_original(original);
        let ordinal = self.selected + 1;
        let total = self.requests.len();
        reader.set_caption(Some(format!(
            "Request {ordinal} of {total} · {}",
            age(self.requests[self.selected].observed_at_unix_ns)
        )));
        if let Some(request) = self.requests.get_mut(self.selected) {
            // Public labels must never retain text derived from a granted body.
            request.hidden_context = hidden;
            request.truncated = truncated;
        }
        self.reader = Some(reader);
        self.reader_key = Some(key);
        self.reading = false;
        self.focus = PromptFocus::Reader;
        PromptAccepted::Accepted
    }

    pub(crate) fn expire(&mut self, now: Instant) -> bool {
        let expired = self
            .reader
            .as_ref()
            .is_some_and(|reader| reader.expired(now));
        if expired {
            self.take_reader();
            self.focus = self.default_focus();
            self.status = PromptStatus::Expired;
        }
        expired
    }

    pub(crate) fn expired_status(&mut self) {
        self.status = PromptStatus::Expired;
    }

    pub(crate) fn revoke(&mut self, reason: impl Into<String>) {
        self.take_reader();
        self.focus = self.default_focus();
        self.status = PromptStatus::Revoked;
        self.notice = Some(reason.into());
    }

    /// Leaves the Prompts section. A late delivery after this call is discarded.
    pub(crate) fn back(&mut self) -> PromptInput {
        self.take_reader();
        self.chooser.open = false;
        self.focus = self.default_focus();
        PromptInput::Back
    }

    pub(crate) fn status_line(&self) -> String {
        let mut line = self.status.message();
        if let Some(notice) = &self.notice {
            line.push_str(" · ");
            line.push_str(notice);
        }
        line
    }

    pub(crate) fn hints(&self) -> &'static str {
        match self.focus {
            PromptFocus::Reader => {
                "↑↓ / PgUp/PgDn Scroll · z Readable/Original · ← Prompts · Esc Back"
            }
            PromptFocus::Chooser if self.chooser.open => {
                "↑↓ Choose · Home/End · Enter/Space Confirm · Esc Cancel"
            }
            PromptFocus::Chooser => "↑↓/Enter/Space Choose · → Read · Esc Back",
            PromptFocus::Navigator => {
                "↑↓ Choose · Enter/→ Read · Home/End · [ ] Older/Newer · l Latest · Esc Back"
            }
        }
    }

    pub(crate) fn hints_for(&self, width: u16) -> &'static str {
        if width >= 70 {
            return self.hints();
        }
        match self.focus {
            PromptFocus::Reader => "↑↓ Scroll · z Mode · ← Prompts · Esc Back",
            PromptFocus::Chooser if self.chooser.open => "↑↓ · Enter Read · Esc Cancel",
            PromptFocus::Chooser => "↑↓ Enter Choose · → Read · Esc Back",
            PromptFocus::Navigator => "↑↓ Choose · Enter Read · Esc Back",
        }
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect) {
        if area.width < crate::MIN_WIDTH || area.height < crate::MIN_HEIGHT {
            frame.render_widget(
                Paragraph::new("Terminal too small · resize · Esc back").centered(),
                area,
            );
            return;
        }
        self.set_layout(area.width);
        let [body, status] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        if self.wide {
            self.draw_wide(frame, body);
        } else {
            self.draw_compact(frame, body);
        }
        frame.render_widget(
            Paragraph::new(roster_cell(&self.status_line(), status.width)).style(theme::muted()),
            status,
        );
        if !self.wide && self.chooser.open {
            self.draw_chooser(frame, body);
        }
    }

    fn draw_wide(&mut self, frame: &mut Frame, area: Rect) {
        let index_width = 34.min(area.width.saturating_sub(12)).max(10);
        let [index, reader] =
            Layout::horizontal([Constraint::Length(index_width), Constraint::Min(10)]).areas(area);
        self.draw_index(frame, index);
        self.draw_reader(frame, reader);
    }

    fn draw_index(&mut self, frame: &mut Frame, area: Rect) {
        let title = if self.requests.is_empty() {
            "Requests".to_owned()
        } else {
            format!("Requests · {} loaded", self.requests.len())
        };
        let block = panel(title);
        let inner = block.inner(area);
        let height = usize::from(inner.height);
        let width = usize::from(inner.width.saturating_sub(1));
        if height == 0 || width == 0 {
            frame.render_widget(block, area);
            return;
        }
        if self.selected < self.index_offset {
            self.index_offset = self.selected;
        } else if self.selected >= self.index_offset + height {
            self.index_offset = self.selected + 1 - height;
        }
        let last = (self.index_offset + height).min(self.requests.len());
        let mut lines = Vec::new();
        if self.requests.is_empty() {
            lines.push(Line::from(self.empty_label()));
        }
        for index in self.index_offset..last {
            let request = &self.requests[index];
            let marker = if index == self.selected { "›" } else { " " };
            let text = roster_cell(
                &format!("{marker}{:02} {}", index + 1, request.label()),
                u16::try_from(width).unwrap_or(u16::MAX),
            );
            let line = Line::from(text);
            lines.push(if index == self.selected {
                line.style(selection_style())
            } else {
                line
            });
        }
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn empty_label(&self) -> String {
        match &self.status {
            PromptStatus::CaptureDisabled => "Capture disabled".into(),
            PromptStatus::MissingIdentity => "Needs a session or agent ID".into(),
            PromptStatus::EmptyScope => "No captured requests".into(),
            PromptStatus::Denied(_) => "Access denied".into(),
            PromptStatus::Loading { .. } => "Loading…".into(),
            _ => "No requests".into(),
        }
    }

    fn draw_reader(&mut self, frame: &mut Frame, area: Rect) {
        let title = if self.requests.is_empty() {
            "Prompt".to_owned()
        } else {
            format!("Prompt {} of {}", self.selected + 1, self.requests.len())
        };
        if self.reader.is_some()
            && self.reader_key == self.current_key()
            && let Some(reader) = &mut self.reader
        {
            let title = reader.caption().map(str::to_owned).unwrap_or(title);
            reader.draw_region(frame, area, title);
            return;
        }
        let lines = self.metadata_lines();
        frame.render_widget(
            Paragraph::new(lines)
                .block(panel(title))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn metadata_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let Some(request) = self.selected_request() else {
            lines.push(Line::from(self.empty_label()));
            if matches!(self.status, PromptStatus::Error(_)) {
                lines.push(Line::from(
                    "Retry prompts is available from the shared action.",
                ));
            }
            if let Some(disclosure) = self.disclosure() {
                lines.push(Line::styled(disclosure, theme::muted()));
            }
            return lines;
        };
        lines.push(Line::styled(
            format!(
                "Request {} of {} · {}",
                self.selected + 1,
                self.requests.len(),
                age(request.observed_at_unix_ns)
            ),
            heading_style(),
        ));
        lines.push(Line::from(request.label()));
        if let Some(turn) = &request.anchor.turn {
            lines.push(Line::styled(
                format!("Turn  {}", visible_text(turn)),
                label_style(),
            ));
        }
        let mut identity = format!(
            "Source  {} · Session  {}",
            visible_text(&request.anchor.source),
            visible_text(&request.anchor.session)
        );
        if let Some(worker) = &request.anchor.worker {
            identity.push_str(&format!(" · Worker  {}", visible_text(worker)));
        }
        lines.push(Line::styled(identity, label_style()));
        if request.hidden_context != 0 {
            lines.push(Line::styled(
                format!(
                    "Context  {} runtime block(s) · z Original/readable",
                    request.hidden_context
                ),
                theme::muted(),
            ));
        }
        if request.truncated {
            lines.push(Line::styled(
                "Reader limit  text clipped · open Activity for the captured record",
                theme::muted(),
            ));
        }
        if let Some(disclosure) = self.disclosure() {
            lines.push(Line::styled(disclosure, theme::muted()));
        }
        match self.scope.capture {
            PromptCapture::Disabled | PromptCapture::MetadataOnly => {}
            PromptCapture::Enabled => {
                if matches!(self.status, PromptStatus::Denied(_)) {
                    lines.push(Line::styled(
                        "Captured metadata remains available in Activity.",
                        theme::muted(),
                    ));
                } else if self.reader.is_some() {
                    lines.push(Line::styled(
                        "Reading under the current grant · Esc returns here.",
                        theme::muted(),
                    ));
                } else if self.reading {
                    lines.push(Line::styled(
                        "Waiting for the grant-fenced read…",
                        theme::muted(),
                    ));
                } else {
                    lines.push(Line::from(
                        "Enter or Right reads this request under the current grant.",
                    ));
                }
            }
        }
        lines
    }

    fn draw_compact(&mut self, frame: &mut Frame, area: Rect) {
        let [chooser, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
        let text = if self.requests.is_empty() {
            format!("Request 0 of 0 · {}", self.empty_label())
        } else {
            let highlighted = if self.chooser.open {
                self.chooser.highlighted
            } else {
                self.selected
            };
            let marker = if self.chooser.open { "▴" } else { "▾" };
            format!(
                "Request {} of {}  ·  {}  {marker}",
                highlighted + 1,
                self.requests.len(),
                self.requests[highlighted].label()
            )
        };
        let mut line = Line::from(roster_cell(&text, chooser.width));
        if self.focus == PromptFocus::Chooser {
            line = line.style(selection_style());
        }
        frame.render_widget(Paragraph::new(line), chooser);
        self.draw_reader(frame, body);
    }

    fn draw_chooser(&mut self, frame: &mut Frame, area: Rect) {
        let rows = self.requests.len().min(10);
        if rows == 0 {
            return;
        }
        let width = area.width.saturating_sub(2).clamp(20, 78);
        let height = u16::try_from(rows + 2)
            .unwrap_or(u16::MAX)
            .min(area.height.saturating_sub(1))
            .max(3);
        let popup = centered(area, width, height);
        frame.render_widget(ratatui::widgets::Clear, popup);
        let items = self.requests.iter().enumerate().map(|(index, request)| {
            ListItem::new(roster_cell(
                &format!("{:02} {}", index + 1, request.label()),
                width.saturating_sub(4),
            ))
        });
        let mut state = ListState::default()
            .with_selected(Some(self.chooser.highlighted.min(rows.saturating_sub(1))));
        // Keep the highlighted choice visible when the retained list is long.
        *state.offset_mut() = self.chooser.highlighted.saturating_sub(rows - 1);
        frame.render_stateful_widget(
            List::new(items)
                .block(panel("Requests"))
                .highlight_symbol("› ")
                .highlight_style(selection_style()),
            popup,
            &mut state,
        );
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

/// Match only complete leading runtime envelopes recognized by the Python
/// reader. Any context in the middle of the user's request remains part of the
/// request. Retained as the compatibility path until adapters normalize.
fn readable_request(text: &str) -> (&str, usize) {
    let mut remaining = text.trim_start();
    let mut blocks = 0;
    while let Some(rest) = remove_context(remaining) {
        remaining = rest.trim_start();
        blocks += 1;
    }
    if blocks == 0 {
        (text, 0)
    } else {
        (remaining, blocks)
    }
}

fn remove_context(text: &str) -> Option<&str> {
    let (candidate, repository_header) =
        if let Some(header) = text.strip_prefix("# AGENTS.md instructions for ") {
            let (path, remainder) = header.split_once('\n')?;
            if path.is_empty() {
                return None;
            }
            (remainder.trim_start(), true)
        } else {
            (text, false)
        };
    let (open, content) = candidate.strip_prefix('<')?.split_once('>')?;
    let tag = open
        .split(|character: char| character.is_ascii_whitespace())
        .next()?;
    if !matches!(
        tag,
        "environment_context"
            | "environment_details"
            | "skills_instructions"
            | "user_instructions"
            | "developer_instructions"
            | "system_instructions"
            | "system-reminder"
            | "INSTRUCTIONS"
            | "skill"
            | "local-command-caveat"
            | "local-command-stdout"
            | "local-command-stderr"
    ) || open.trim_ascii_end().ends_with('/')
        || (repository_header && tag != "INSTRUCTIONS")
    {
        return None;
    }
    let closing = format!("</{tag}>");
    let (body, tail) = content.split_once(&closing)?;
    // A same-name opener makes the first close ambiguous. Preserve that text.
    let nested = format!("<{tag}");
    if body.match_indices(&nested).any(|(index, _)| {
        body[index + nested.len()..]
            .starts_with(|character: char| character == '>' || character.is_ascii_whitespace())
    }) {
        return None;
    }
    Some(tail)
}

fn display_line(line: &str) -> String {
    let mut expanded = String::new();
    let mut column = 0;
    for (index, segment) in line.split('\t').enumerate() {
        if index != 0 {
            let spaces = TAB_COLUMNS - column % TAB_COLUMNS;
            expanded.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        }
        let segment = visible_text(segment);
        column += usize::from(segment.as_str().cell_width());
        expanded.push_str(&segment);
    }
    expanded
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    let span = Span::raw(line);
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut columns = 0;
    let mut word_break = None;
    for item in span.styled_graphemes(Style::default()) {
        let cells = usize::from(item.symbol.cell_width());
        if columns + cells > width && !current.is_empty() {
            if let Some((byte, previous_columns)) = word_break.take() {
                let tail = current.split_off(byte);
                rows.push(std::mem::replace(&mut current, tail));
                columns -= previous_columns;
            }
            if columns + cells > width && !current.is_empty() {
                rows.push(std::mem::take(&mut current));
                columns = 0;
            }
        }
        current.push_str(item.symbol);
        columns += cells;
        if item.symbol.chars().all(char::is_whitespace) {
            word_break = Some((current.len(), columns));
        }
    }
    rows.push(current);
    rows
}

fn slice_line(line: &str, column: usize, width: usize) -> String {
    let span = Span::raw(line);
    let mut visible = String::new();
    let mut cursor = 0;
    let end = column + width;
    for item in span.styled_graphemes(Style::default()) {
        let next = cursor + usize::from(item.symbol.cell_width());
        if next > column && cursor < end {
            if cursor < column || next > end {
                let cells = next.min(end) - cursor.max(column);
                visible.extend(std::iter::repeat_n(' ', cells));
            } else {
                visible.push_str(item.symbol);
            }
        }
        if next >= end {
            break;
        }
        cursor = next;
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::prompt_channel;
    use ratatui::{Terminal, backend::TestBackend};

    fn scope() -> PromptScope {
        PromptScope {
            source: "harness.fixture".into(),
            session: "session-1".into(),
            worker: None,
            turn: None,
            capture: PromptCapture::Enabled,
            main_session: None,
        }
    }

    fn request(index: usize) -> PromptRequest {
        PromptRequest {
            anchor: WorkAnchor {
                source: "harness.fixture".into(),
                session: "session-1".into(),
                worker: None,
                turn: Some(format!("turn-{index}")),
            },
            event_id: format!("event-{index}"),
            revision: 0,
            observed_at_unix_ns: index as u64 * 60_000_000_000,
            snapshot_id: Some(7.into()),
            history_identity: Some("history".into()),
            title: Some(format!("Fixture request {index}")),
            hidden_context: 0,
            truncated: false,
        }
    }

    fn loaded(entries: Vec<PromptRequest>) -> (PromptNavigator, PromptPageMeta) {
        let mut navigator = PromptNavigator::new(scope());
        let token = navigator.begin_load();
        let meta = PromptPageMeta {
            retained_cap: Some(256),
            earlier_available: false,
            limited: false,
        };
        assert!(navigator.accept_page(token, entries, meta));
        (navigator, meta)
    }

    fn draw(navigator: &mut PromptNavigator, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let buffer = terminal
            .draw(|frame| navigator.draw(frame, frame.area()))
            .unwrap()
            .buffer;
        buffer
            .content()
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn granted(navigator: &mut PromptNavigator, epoch: u64, text: String) -> PromptAccepted {
        let key = navigator.current_key().unwrap();
        let (_delivery, receiver) = prompt_channel(PromptKind::User);
        navigator.accept_reader_presentation(
            epoch,
            key,
            PromptPresentation::normalized(text.clone(), Some(text), 0, false),
            receiver,
            Instant::now(),
        )
    }

    #[test]
    fn wide_navigation_starts_at_latest_and_reader_scrolling_keeps_the_request() {
        let mut navigator = loaded((1..=5).map(request).collect()).0;
        navigator.set_layout(120);
        assert_eq!(navigator.focus(), PromptFocus::Navigator);
        assert_eq!(navigator.selected_ordinal(), Some(5));
        let framed = draw(&mut navigator, 120, 32);
        assert!(framed.contains("Requests · 5 loaded"));
        assert!(framed.contains("Prompt 5 of 5"));

        assert!(matches!(navigator.key(KeyCode::Up), PromptInput::Handled));
        assert_eq!(navigator.selected_ordinal(), Some(4));
        assert!(matches!(navigator.key(KeyCode::Home), PromptInput::Handled));
        assert_eq!(navigator.selected_ordinal(), Some(1));
        assert!(matches!(navigator.key(KeyCode::End), PromptInput::Handled));
        assert_eq!(navigator.selected_ordinal(), Some(5));

        // Expanded Prompts: brackets move the selection, `l` returns to latest.
        navigator.key(KeyCode::Char('['));
        assert_eq!(navigator.selected_ordinal(), Some(4));
        navigator.key(KeyCode::Char(']'));
        assert_eq!(navigator.selected_ordinal(), Some(5));
        navigator.key(KeyCode::Home);
        navigator.key(KeyCode::Char('l'));
        assert_eq!(navigator.selected_ordinal(), Some(5));
        // `z` records the presentation choice for this request only.
        navigator.key(KeyCode::Char('z'));
        assert!(navigator.preferred_original(&("event-5".into(), 0)));

        let input = navigator.key(KeyCode::Enter);
        let PromptInput::Read {
            entry,
            original,
            epoch,
        } = input
        else {
            panic!("Enter must read the selected request");
        };
        assert_eq!(entry.event_id, "event-5");
        assert!(original);
        assert_eq!(navigator.focus(), PromptFocus::Reader);
        assert!(navigator.read_pending());

        let text = (0..80)
            .map(|index| format!("Visible line {index}\n"))
            .collect::<String>();
        assert_eq!(
            granted(&mut navigator, epoch, text),
            PromptAccepted::Accepted
        );
        let before = navigator.selected_ordinal();
        for _ in 0..10 {
            navigator.key(KeyCode::Down);
        }
        assert!(navigator.key(KeyCode::PageDown).is_handled());
        assert_eq!(navigator.selected_ordinal(), before);
        let framed = draw(&mut navigator, 120, 32);
        assert!(navigator.reader().unwrap().first_visible_line().is_some());
        assert!(framed.contains("Request 5 of 5"));

        assert!(matches!(
            navigator.key(KeyCode::Left),
            PromptInput::ReaderClosed
        ));
        assert_eq!(navigator.focus(), PromptFocus::Navigator);
        assert!(!navigator.reader_open());
        assert_eq!(navigator.selected_ordinal(), Some(5));
    }

    #[test]
    fn compact_chooser_confirms_unchanged_and_maps_focus_across_resize() {
        let mut navigator = loaded((1..=5).map(request).collect()).0;
        navigator.set_layout(80);
        assert_eq!(navigator.focus(), PromptFocus::Chooser);
        assert_eq!(navigator.selected_ordinal(), Some(5));
        let framed = draw(&mut navigator, 80, 24);
        assert!(framed.contains("Request 5 of 5"));

        // Enter opens the chooser, a second Enter confirms the unchanged value.
        assert!(matches!(
            navigator.key(KeyCode::Enter),
            PromptInput::Handled
        ));
        assert!(navigator.chooser_open());
        let input = navigator.key(KeyCode::Enter);
        let PromptInput::Read { entry, epoch, .. } = input else {
            panic!("confirming the chooser must read even when unchanged");
        };
        assert_eq!(entry.event_id, "event-5");
        assert!(!navigator.chooser_open());
        assert_eq!(navigator.focus(), PromptFocus::Reader);
        assert_eq!(
            granted(&mut navigator, epoch, "Body".into()),
            PromptAccepted::Accepted
        );
        let framed = draw(&mut navigator, 60, 18);
        assert!(framed.contains("Request 5 of 5"));
        assert_eq!(navigator.focus(), PromptFocus::Reader);

        // A focused reader stays a reader across the breakpoint.
        draw(&mut navigator, 120, 32);
        assert_eq!(navigator.focus(), PromptFocus::Reader);
        navigator.key(KeyCode::Left);
        assert_eq!(navigator.focus(), PromptFocus::Navigator);
        draw(&mut navigator, 80, 24);
        assert_eq!(navigator.focus(), PromptFocus::Chooser);
        assert_eq!(navigator.selected_ordinal(), Some(5));

        // Space opens; each compact choice stays a single line.
        navigator.key(KeyCode::Char(' '));
        assert!(navigator.chooser_open());
        let framed = draw(&mut navigator, 60, 18);
        for index in 1..=5 {
            assert!(
                framed.contains(&format!("Fixture request {index}")),
                "compact chooser must show request {index}"
            );
        }
        assert!(framed.contains("Request 5 of 5"));
        navigator.key(KeyCode::Up);
        assert_eq!(navigator.highlighted(), 3);
        navigator.key(KeyCode::Esc);
        assert!(!navigator.chooser_open());
        assert_eq!(navigator.selected_ordinal(), Some(5));

        // Right reads without opening the chooser.
        let input = navigator.key(KeyCode::Right);
        assert!(matches!(input, PromptInput::Read { .. }));
        assert!(!navigator.chooser_open());
    }

    #[test]
    fn reader_keeps_the_visible_source_line_when_width_changes() {
        let mut navigator = loaded(vec![request(1)]).0;
        navigator.set_layout(60);
        let input = navigator.key(KeyCode::Right);
        let PromptInput::Read { epoch, .. } = input else {
            panic!("Right reads");
        };
        let text = (0..40)
            .map(|index| {
                format!("SOURCE-LINE-{index:02} with enough words to wrap at sixty columns\n")
            })
            .collect::<String>();
        assert_eq!(
            granted(&mut navigator, epoch, text),
            PromptAccepted::Accepted
        );
        draw(&mut navigator, 60, 18);
        for _ in 0..12 {
            navigator.key(KeyCode::Down);
        }
        draw(&mut navigator, 60, 18);
        let before = navigator
            .reader()
            .unwrap()
            .first_visible_line()
            .unwrap()
            .to_owned();
        let marker = before
            .split_whitespace()
            .find(|word| word.starts_with("SOURCE-LINE-"))
            .unwrap()
            .to_owned();
        draw(&mut navigator, 120, 32);
        let after = navigator.reader().unwrap().first_visible_line().unwrap();
        assert!(
            after.contains(&marker),
            "resize must keep the visible source line: {marker} vs {after}"
        );
    }

    #[test]
    fn pages_preserve_request_identity_and_disclose_loaded_and_capped_history() {
        let mut navigator = PromptNavigator::new(scope());
        let token = navigator.begin_load();
        assert!(navigator.accept_page(
            token,
            (3..=5).map(request).collect(),
            PromptPageMeta {
                retained_cap: Some(256),
                earlier_available: true,
                limited: false,
            },
        ));
        assert_eq!(navigator.selected_ordinal(), Some(3));
        navigator.key(KeyCode::Up);
        assert_eq!(navigator.selected_ordinal(), Some(2));
        let token = navigator.begin_older().expect("earlier page is available");
        assert!(matches!(
            navigator.status(),
            PromptStatus::Loading { older: true }
        ));
        assert!(navigator.accept_page(
            token,
            (1..=2).map(request).collect(),
            PromptPageMeta {
                retained_cap: Some(256),
                earlier_available: false,
                limited: true,
            },
        ));
        assert_eq!(navigator.selected_ordinal(), Some(4));
        assert_eq!(navigator.requests().len(), 5);
        assert!(navigator.status_line().contains("Retained window cap 256"));
        assert!(
            navigator
                .status_line()
                .contains("older history in Activity")
        );

        // A superseded page response never replaces the current page.
        let stale = navigator.begin_load();
        let current = navigator.begin_load();
        assert!(!navigator.accept_page(stale, vec![request(9)], PromptPageMeta::default(),));
        assert!(navigator.notice().unwrap().contains("Discarded"));
        assert!(navigator.accept_page(current, vec![request(9)], PromptPageMeta::default(),));
        assert_eq!(navigator.selected_ordinal(), Some(1));

        // An exact empty scope is distinct from loading and from a cap.
        let empty = navigator.begin_load();
        assert!(navigator.accept_page(empty, Vec::new(), PromptPageMeta::default()));
        assert_eq!(navigator.status(), &PromptStatus::EmptyScope);
        assert!(draw(&mut navigator, 60, 18).contains("No user prompts were captured"));
    }

    #[test]
    fn live_arrivals_preserve_the_selected_request_until_it_disappears() {
        let mut navigator = loaded(vec![request(1), request(2)]).0;
        navigator.set_layout(120);
        assert_eq!(navigator.selected_ordinal(), Some(2));

        // A newer request arrives while the second request is selected.
        let token = navigator.begin_load();
        assert!(navigator.accept_page(
            token,
            vec![request(1), request(2), request(3)],
            PromptPageMeta {
                retained_cap: Some(256),
                earlier_available: false,
                limited: false,
            },
        ));
        assert_eq!(navigator.selected_ordinal(), Some(2));
        assert_eq!(navigator.selected_request().unwrap().event_id, "event-2");
        assert!(navigator.notice().is_none());

        // The selected identity disappears: keep the nearest request and say so.
        let token = navigator.begin_load();
        assert!(navigator.accept_page(
            token,
            vec![request(1), request(3)],
            PromptPageMeta {
                retained_cap: Some(256),
                earlier_available: false,
                limited: false,
            },
        ));
        assert!(navigator.notice().unwrap().contains("no longer recorded"));
        assert!(navigator.selected_ordinal().is_some());
    }

    #[test]
    fn grants_are_scoped_revoked_expired_and_late_deliveries_are_scoped() {
        let mut navigator = loaded(vec![request(1), request(2)]).0;
        navigator.set_layout(80);
        let framed = draw(&mut navigator, 80, 24);
        assert!(framed.contains("Fixture request 2"));
        navigator.deny("requires approval for the current source");
        assert!(navigator.status_line().contains("Prompt access denied"));
        assert!(navigator.status_line().contains("requires approval"));
        assert!(
            navigator.selected_request().is_some(),
            "metadata stays useful"
        );

        let PromptInput::Read { epoch, .. } = navigator.key(KeyCode::Right) else {
            panic!("Right reads");
        };
        navigator.revoke("capture policy changed");
        assert!(!navigator.reader_open());
        assert_eq!(navigator.status(), &PromptStatus::Revoked);
        assert_eq!(
            granted(&mut navigator, epoch, "late".into()),
            PromptAccepted::Late(PromptLate::Superseded)
        );

        let reload = navigator.begin_load();
        navigator.accept_page(
            reload,
            vec![request(1), request(2)],
            PromptPageMeta::default(),
        );
        let PromptInput::Read { epoch, .. } = navigator.key(KeyCode::Right) else {
            panic!("Right reads");
        };
        let wrong = ("other-event".into(), 0);
        let (_delivery, receiver) = prompt_channel(PromptKind::User);
        assert_eq!(
            navigator.accept_reader(epoch, wrong, "outside".into(), receiver, Instant::now()),
            PromptAccepted::Late(PromptLate::OutOfScope)
        );
        assert_eq!(
            granted(&mut navigator, epoch, "body".into()),
            PromptAccepted::Accepted
        );
        let start = Instant::now();
        assert!(!navigator.expire(start + Duration::from_millis(24_999)));
        assert!(navigator.reader_open());
        assert!(navigator.expire(start + Duration::from_secs(25)));
        assert!(!navigator.reader_open());
        assert_eq!(navigator.status(), &PromptStatus::Expired);
    }

    #[test]
    fn worker_scope_discloses_main_session_navigation_without_broadening() {
        let main = WorkAnchor {
            source: "harness.fixture".into(),
            session: "session-1".into(),
            worker: None,
            turn: None,
        };
        let mut worker = scope();
        worker.worker = Some("worker-a".into());
        worker.main_session = Some(main.clone());
        let mut navigator = PromptNavigator::new(worker.clone());
        let token = navigator.begin_load();
        let mut entry = request(1);
        entry.anchor.worker = Some("worker-a".into());
        assert!(navigator.accept_page(
            token,
            vec![entry],
            PromptPageMeta {
                retained_cap: Some(256),
                earlier_available: false,
                limited: false,
            },
        ));
        let disclosure = navigator.disclosure().unwrap();
        assert!(disclosure.contains("Worker-scoped"));
        assert!(disclosure.contains("worker-a"));
        assert!(disclosure.contains("main-session requests are not included"));
        assert_eq!(
            navigator
                .selected_request()
                .unwrap()
                .anchor
                .worker
                .as_deref(),
            Some("worker-a")
        );
        assert!(matches!(
            navigator.key(KeyCode::Char('m')),
            PromptInput::MainSession(anchor) if anchor == main
        ));

        worker.main_session = None;
        let mut lone = PromptNavigator::new(worker);
        lone.set_layout(60);
        assert!(lone.disclosure().unwrap().contains("Worker-scoped"));
        assert!(matches!(lone.key(KeyCode::Char('m')), PromptInput::Ignored));
    }

    #[test]
    fn out_of_scope_pages_and_missing_capture_are_scoped_to_the_attempted_action() {
        let mut navigator = loaded(vec![request(1)]).0;
        let token = navigator.begin_load();
        let mut other_session = request(2);
        other_session.anchor.session = "other-session".into();
        let mut other_worker = request(3);
        other_worker.anchor.worker = Some("worker-x".into());
        assert!(navigator.accept_page(
            token,
            vec![other_session, other_worker],
            PromptPageMeta::default(),
        ));
        assert!(navigator.requests().is_empty());
        assert_eq!(navigator.status(), &PromptStatus::EmptyScope);
        assert!(navigator.notice().unwrap().contains("outside"));

        navigator.missing_capture();
        assert!(
            navigator
                .status_line()
                .contains("no longer in the captured source")
        );
        navigator.stale("checkpoint moved");
        assert!(navigator.status_line().contains("source changed"));
        assert!(navigator.status_line().contains("checkpoint moved"));
        navigator.out_of_scope("worker changed");
        assert!(
            navigator
                .status_line()
                .contains("outside the selected request")
        );
    }

    #[test]
    fn availability_states_render_distinctly_at_every_supported_size() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut disabled = PromptNavigator::new(PromptScope {
                capture: PromptCapture::Disabled,
                ..scope()
            });
            let framed = draw(&mut disabled, width, height);
            assert!(framed.contains("Content capture is disabled"));
            assert!(!framed.contains("Enter or Right reads"));

            let mut missing = PromptNavigator::new(scope());
            missing.missing_identity();
            assert!(
                draw(&mut missing, width, height).contains("needs a session or agent ID")
                    || draw(&mut missing, width, height).contains("session or agent ID")
            );

            let mut failed = PromptNavigator::new(scope());
            failed.fail("receiver unavailable");
            let framed = draw(&mut failed, width, height);
            assert!(framed.contains("receiver unavailable"));
            assert!(framed.contains("Retry") || framed.contains("retry"));

            let mut metadata = PromptNavigator::new(PromptScope {
                capture: PromptCapture::MetadataOnly,
                ..scope()
            });
            metadata.set_layout(width);
            assert!(metadata.disclosure().unwrap().contains("Metadata only"));
        }
    }

    #[test]
    fn readable_omits_only_complete_leading_context_envelopes() {
        let capture = " \n<environment_context>fixture data</environment_context>\n\
            # AGENTS.md instructions for fixture/repo\n<INSTRUCTIONS>fixture rules</INSTRUCTIONS>\n\
            Write this request.\n<environment_context>literal example</environment_context>";
        let (body, hidden) = readable_request(capture);
        assert_eq!(hidden, 2);
        assert_eq!(
            body,
            "Write this request.\n<environment_context>literal example</environment_context>"
        );
        for text in [
            "<environment_context>truncated",
            "< environment_context>not a tag</environment_context>",
            "<environment_context / >self closing</environment_context>",
            "<environment_context-extra>similar tag</environment_context>",
            "<environment_context>ambiguous <environment_context>inner</environment_context>",
            "<project>arbitrary XML</project>",
            "# AGENTS.md instructions for fixture/repo\n<skill>separate tag</skill>",
            "Example:\n<system-reminder>inline</system-reminder>",
            "```xml\n<skill>in a fence</skill>\n```",
        ] {
            assert_eq!(readable_request(text), (text, 0));
        }
    }

    #[test]
    fn original_layout_preserves_tab_stops_and_clips_graphemes_at_cell_edges() {
        assert_eq!(display_line("界\tA\tB"), "界      A       B");
        assert_eq!(display_line("safe\x1b[31m"), r"safe\u{1b}[31m");
        assert_eq!(slice_line("界e\u{301}終", 1, 3), " e\u{301} ");
        assert_eq!(slice_line("界e\u{301}終", 2, 3), "e\u{301}終");
        assert_eq!(wrap_line("word another", 10), ["word ", "another"]);
    }

    fn draw_prompt(terminal: &mut Terminal<TestBackend>, prompt: &mut PromptScreen) -> String {
        let buffer = terminal.draw(|frame| prompt.draw(frame)).unwrap().buffer;
        buffer
            .content()
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn prompt_modes_render_and_scroll_at_wide_and_compact_sizes() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let (_delivery, receiver) = prompt_channel(PromptKind::User);
            let start = Instant::now();
            let text = format!(
                "<environment_context>FIXTURE CONTEXT</environment_context>\n\
                 Request\t{}TAIL_MARKER\n\n{}\nLAST_APPROVED_LINE",
                "界".repeat(70),
                (0..38)
                    .map(|i| format!("List item {i}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            let mut prompt = PromptScreen::new(text, receiver, start);
            let readable = draw_prompt(&mut terminal, &mut prompt);
            assert!(readable.contains("Granted prompt · readable"));
            assert!(readable.contains("1 runtime context block hidden"));
            assert!(readable.contains("TAIL_MARKER"));
            assert!(!readable.contains("FIXTURE CONTEXT"));

            prompt.key(KeyCode::Down);
            let moved_readable = draw_prompt(&mut terminal, &mut prompt);
            prompt.key(KeyCode::Char('z'));
            let original = draw_prompt(&mut terminal, &mut prompt);
            assert!(original.contains("captured text · original layout"));
            assert!(original.contains("FIXTURE CONTEXT"));
            assert!(!original.contains("TAIL_MARKER"));
            assert!(original.contains("Column 1"));
            for _ in 0..32 {
                prompt.key(KeyCode::Right);
            }
            assert!(draw_prompt(&mut terminal, &mut prompt).contains("TAIL_MARKER"));

            // The next layout clamps the existing horizontal position.
            terminal.backend_mut().resize(200, height);
            assert!(draw_prompt(&mut terminal, &mut prompt).contains("Column 1"));
            terminal.backend_mut().resize(width, height);
            assert!(draw_prompt(&mut terminal, &mut prompt).contains("FIXTURE CONTEXT"));
            for _ in 0..48 {
                prompt.key(KeyCode::Down);
            }
            assert!(draw_prompt(&mut terminal, &mut prompt).contains("LAST_APPROVED_LINE"));
            prompt.key(KeyCode::Home);
            assert_eq!(draw_prompt(&mut terminal, &mut prompt), original);
            prompt.key(KeyCode::Char('z'));
            assert_eq!(draw_prompt(&mut terminal, &mut prompt), moved_readable);
            assert!(!prompt.expired(start + Duration::from_millis(24_999)));
            assert!(prompt.expired(start + Duration::from_secs(25)));
        }
    }

    #[test]
    fn reader_page_and_end_keys_move_without_changing_the_request() {
        let mut navigator = loaded(vec![request(1)]).0;
        navigator.set_layout(120);
        let PromptInput::Read { epoch, .. } = navigator.key(KeyCode::Enter) else {
            panic!("Enter reads");
        };
        let text = (0..100)
            .map(|index| format!("Line {index}\n"))
            .collect::<String>();
        assert_eq!(
            granted(&mut navigator, epoch, text),
            PromptAccepted::Accepted
        );
        draw(&mut navigator, 120, 20);
        let reader = navigator.reader().unwrap();
        assert!(reader.first_visible_line().unwrap().contains("Line 0"));
        reader.key(KeyCode::PageDown);
        assert!(!reader.first_visible_line().unwrap().contains("Line 0"));
        reader.key(KeyCode::End);
        let end = reader.first_visible_line().unwrap().to_owned();
        assert!(end.starts_with("Line 8"), "{end}");
        reader.key(KeyCode::Home);
        assert!(reader.first_visible_line().unwrap().contains("Line 0"));
        assert_eq!(navigator.selected_ordinal(), Some(1));
    }

    #[test]
    fn unnormalized_capture_labels_the_readable_form_unavailable() {
        let presentation = PromptPresentation::normalized(
            "<environment_context>raw</environment_context>\nRequest".into(),
            None,
            1,
            true,
        );
        let (_delivery, receiver) = prompt_channel(PromptKind::User);
        let mut prompt =
            PromptScreen::new_with_presentation(presentation, receiver, Instant::now());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let framed = draw_prompt(&mut terminal, &mut prompt);
        assert!(framed.contains("Readable form unavailable"));
        assert!(!framed.contains("raw"));
        prompt.key(KeyCode::Char('z'));
        let framed = draw_prompt(&mut terminal, &mut prompt);
        assert!(framed.contains("raw"));
        assert!(framed.contains("Reader limit") || prompt.truncated());
    }

    #[test]
    fn other_private_text_keeps_its_single_presentation() {
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        let (_delivery, receiver) = prompt_channel(PromptKind::Mcp);
        let mut prompt = PromptScreen::new(
            "<environment_context>shown literally</environment_context>".into(),
            receiver,
            Instant::now(),
        );
        let before = draw_prompt(&mut terminal, &mut prompt);
        assert!(before.contains("Granted MCP payload"));
        assert!(before.contains("shown literally"));
        assert!(!before.contains("Original layout"));
        prompt.key(KeyCode::Char('z'));
        assert_eq!(draw_prompt(&mut terminal, &mut prompt), before);
    }

    trait Handled {
        fn is_handled(&self) -> bool;
    }

    impl Handled for PromptInput {
        fn is_handled(&self) -> bool {
            matches!(self, Self::Handled | Self::Read { .. })
        }
    }
}
