//! One action inventory for cockpit key dispatch, footer hints and `F1` help.
//!
//! The shell never hard-codes a provider. It names a destination or an action
//! and lets the per-view modules interpret it. Text editors and private modals
//! own their input: see [`Focus`] and [`text_entry`] for the ownership rules.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Destination {
    Agents,
    Activity,
    Mcp,
    Traces,
    Host,
    Metrics,
    Findings,
    Operations,
    Usage,
}

/// The seven Python 0.2.8 root destinations. `1`..`7` never move.
pub(crate) const NAVIGATION: &[(char, Destination, &str)] = &[
    ('1', Destination::Agents, "Agents"),
    ('2', Destination::Activity, "Activity"),
    ('3', Destination::Mcp, "MCP"),
    ('4', Destination::Traces, "Traces"),
    ('5', Destination::Host, "Host"),
    ('6', Destination::Metrics, "Metrics"),
    ('7', Destination::Findings, "Findings"),
];

/// Rust-only destinations. They stay outside `1`..`7` so the reference root
/// destinations remain recognizable, and are exposed through help and the
/// command palette even when the terminal clips the numbered strip.
pub(crate) const EXTRA_NAVIGATION: &[(char, Destination, &str)] = &[
    ('9', Destination::Operations, "Operations"),
    ('U', Destination::Usage, "Usage"),
];

pub(crate) fn navigation_hints(items: &[(char, Destination, &str)]) -> String {
    items
        .iter()
        .map(|(key, _, label)| format!("{key} {label}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

pub(crate) fn destination(code: KeyCode) -> Option<Destination> {
    let KeyCode::Char(character) = code else {
        return None;
    };
    NAVIGATION
        .iter()
        .chain(EXTRA_NAVIGATION)
        .find_map(|(key, dest, _)| (*key == character).then_some(*dest))
}

/// Stable action identifiers shared by dispatch, footers, help and the palette.
pub(crate) mod id {
    pub(crate) const GO_AGENTS: &str = "go.agents";
    pub(crate) const GO_ACTIVITY: &str = "go.activity";
    pub(crate) const GO_MCP: &str = "go.mcp";
    pub(crate) const GO_TRACES: &str = "go.traces";
    pub(crate) const GO_HOST: &str = "go.host";
    pub(crate) const GO_METRICS: &str = "go.metrics";
    pub(crate) const GO_FINDINGS: &str = "go.findings";
    pub(crate) const GO_OPERATIONS: &str = "go.operations";
    pub(crate) const GO_USAGE: &str = "go.usage";
    pub(crate) const PALETTE: &str = "cockpit.palette";
    pub(crate) const THEMES: &str = "cockpit.themes";
    pub(crate) const HELP: &str = "cockpit.help";
    pub(crate) const SESSION_CHOOSER: &str = "session.choose";
    pub(crate) const SCOPE_TODAY: &str = "session.today";
    pub(crate) const SCOPE_HISTORY: &str = "session.history";
    pub(crate) const SESSION_ERRORS: &str = "session.errors";
    pub(crate) const SESSION_SUBTREE: &str = "evidence.scope.subtree";
    pub(crate) const SCOPE_SELECTED: &str = "evidence.scope.session";
    pub(crate) const SCOPE_ALL: &str = "evidence.scope.all";
    pub(crate) const LIVE_PAUSE: &str = "live.pause";
    pub(crate) const LIVE_REFRESH: &str = "live.refresh";
    pub(crate) const DIAGNOSTICS: &str = "diagnostics.open";
    pub(crate) const RETRY_CONNECTION: &str = "diagnostics.retry";
    pub(crate) const HISTORY_PATHS: &str = "history.paths";
    pub(crate) const HISTORY_COMMANDS: &str = "history.commands";
    pub(crate) const HISTORY_INSTRUCTIONS: &str = "history.instructions";
    pub(crate) const HISTORY_FAILURES: &str = "history.failures";
    pub(crate) const HISTORY_FAILED_COMMAND: &str = "history.failed-command";
    pub(crate) const REVIEW: &str = "view.review";
    pub(crate) const PROMPTS: &str = "view.prompts";
    pub(crate) const TRACES: &str = "view.traces";
    pub(crate) const OLDER_PAGE: &str = "view.older-page";
    pub(crate) const NEWER_PAGE: &str = "view.newer-page";
    pub(crate) const LATEST_PAGE: &str = "view.latest-page";
    pub(crate) const TURN: &str = "view.turn";
    pub(crate) const MCP_PAYLOAD: &str = "view.mcp-payload";
    pub(crate) const FILE_EDITS: &str = "view.file-edits";
    pub(crate) const REVIEW_NOTES: &str = "view.review-notes";
    pub(crate) const MARK_REVIEWED: &str = "view.mark-reviewed";
    pub(crate) const COPY: &str = "view.copy";
    pub(crate) const EXPORT: &str = "view.export";
    pub(crate) const INTERNAL_REVIEWS: &str = "view.internal-reviews";
    pub(crate) const WORKFLOW: &str = "view.workflow";
    pub(crate) const TASKS: &str = "view.tasks";
    pub(crate) const ATTENTION: &str = "session.attention";
    pub(crate) const MARK_SEEN: &str = "session.mark-seen";
    pub(crate) const REVIEW_FILTER: &str = "session.review-filter";
    pub(crate) const GROUP: &str = "session.group";
    pub(crate) const TREE: &str = "session.tree";
    pub(crate) const RESET: &str = "session.reset";
    pub(crate) const FAMILY: &str = "session.family";
    pub(crate) const OVERVIEW: &str = "session.overview";
    pub(crate) const SESSION_ACTIVITY: &str = "session.activity";
    pub(crate) const SESSION_USAGE: &str = "session.usage";
    pub(crate) const HOST_NOW: &str = "host.now";
    pub(crate) const INSPECTOR_MODE: &str = "session.inspector-mode";
    pub(crate) const INSPECTOR_SECTIONS: &str = "session.sections";
    pub(crate) const SEARCH: &str = "view.search";
    pub(crate) const INSPECT: &str = "view.inspect";
    pub(crate) const BACK: &str = "view.back";
    pub(crate) const QUIT: &str = "cockpit.quit";
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Group {
    Navigate,
    Session,
    Evidence,
    Live,
    Search,
    Page,
    Review,
    Workflow,
    System,
}

impl Group {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Navigate => "VIEWS",
            Self::Session => "SESSION",
            Self::Evidence => "EVIDENCE",
            Self::Live => "LIVE AND DIAGNOSTICS",
            Self::Search => "SEARCH",
            Self::Page => "PAGES AND SELECTION",
            Self::Review => "REVIEW AND PROMPTS",
            Self::Workflow => "WORKFLOW AND SAVED WORK",
            Self::System => "SYSTEM",
        }
    }
}

/// Where a key is delivered. This is deliberately coarse: the seam is the
/// shell, while each per-view module owns the controls inside its region.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Context {
    CommandCenter,
    History,
    Workbench,
    Investigation,
    Form,
}

impl Context {
    const fn every() -> &'static [Context] {
        &[
            Context::CommandCenter,
            Context::History,
            Context::Workbench,
            Context::Investigation,
            Context::Form,
        ]
    }
}

pub(crate) struct Action {
    pub(crate) id: &'static str,
    pub(crate) keys: &'static str,
    pub(crate) label: &'static str,
    pub(crate) detail: &'static str,
    pub(crate) group: Group,
    pub(crate) contexts: &'static [Context],
}

const fn action(
    id: &'static str,
    keys: &'static str,
    label: &'static str,
    detail: &'static str,
    group: Group,
    contexts: &'static [Context],
) -> Action {
    Action {
        id,
        keys,
        label,
        detail,
        group,
        contexts,
    }
}

const GO_CONTEXTS: &[Context] = &[
    Context::CommandCenter,
    Context::History,
    Context::Workbench,
    Context::Investigation,
    Context::Form,
];

/// Complete keyboard inventory. Footers and help render from this table; the
/// palette resolves the same identifiers, so a new action needs one row plus
/// one dispatch arm.
pub(crate) const ACTIONS: &[Action] = &[
    action(
        id::GO_AGENTS,
        "1",
        "Agents",
        "Return to the command center and the named session roster",
        Group::Navigate,
        GO_CONTEXTS,
    ),
    action(
        id::GO_ACTIVITY,
        "2",
        "Activity",
        "Recorded session events for the current scope",
        Group::Navigate,
        GO_CONTEXTS,
    ),
    action(
        id::GO_MCP,
        "3",
        "MCP",
        "Configured and observed MCP calls",
        Group::Navigate,
        GO_CONTEXTS,
    ),
    action(
        id::GO_TRACES,
        "4",
        "Traces",
        "Recorded execution traces",
        Group::Navigate,
        GO_CONTEXTS,
    ),
    action(
        id::GO_HOST,
        "5",
        "Host",
        "Current or recorded host evidence",
        Group::Navigate,
        GO_CONTEXTS,
    ),
    action(
        id::GO_METRICS,
        "6",
        "Metrics",
        "Retained metric instruments",
        Group::Navigate,
        GO_CONTEXTS,
    ),
    action(
        id::GO_FINDINGS,
        "7",
        "Findings",
        "Grouped findings and recorded diagnostics",
        Group::Navigate,
        GO_CONTEXTS,
    ),
    action(
        id::GO_OPERATIONS,
        "9",
        "Operations (Rust)",
        "Rust-only retained operation evidence",
        Group::Navigate,
        GO_CONTEXTS,
    ),
    action(
        id::GO_USAGE,
        "U",
        "Usage (Rust)",
        "Rust-only combined retained usage",
        Group::Navigate,
        GO_CONTEXTS,
    ),
    action(
        id::PALETTE,
        "Ctrl+P",
        "Command palette",
        "Search destinations, sessions, scopes, live controls and current-view actions",
        Group::System,
        Context::every(),
    ),
    action(
        id::THEMES,
        "F2",
        "Themes",
        "Preview and apply Observation, Current, Bright, Dark or Solarized",
        Group::System,
        Context::every(),
    ),
    action(
        id::HELP,
        "F1 / ?",
        "Contextual help",
        "Scrollable reference for this cockpit context; returns to the same control",
        Group::System,
        Context::every(),
    ),
    action(
        id::SESSION_CHOOSER,
        "",
        "Choose agent session",
        "Search task, project, provider, session and native context",
        Group::Session,
        &[Context::CommandCenter, Context::History, Context::Workbench],
    ),
    action(
        id::SCOPE_TODAY,
        "T",
        "Show Today",
        "Recorded current work, pending input and today's reported outcomes",
        Group::Session,
        &[Context::CommandCenter],
    ),
    action(
        id::SCOPE_HISTORY,
        "O",
        "Show History",
        "All retained sessions, including earlier unreviewed work",
        Group::Session,
        &[Context::CommandCenter],
    ),
    action(
        id::SESSION_ERRORS,
        "E",
        "Errors only",
        "Recorded failed or interrupted runs on the loaded page",
        Group::Session,
        &[Context::CommandCenter],
    ),
    action(
        id::SESSION_SUBTREE,
        "S",
        "Evidence subtree scope",
        "Add confirmed delegated descendants to the selected agent scope",
        Group::Evidence,
        &[Context::CommandCenter],
    ),
    action(
        id::SCOPE_SELECTED,
        "",
        "Scope evidence to this session",
        "Open Activity for the exact selected session or turn",
        Group::Evidence,
        &[Context::History],
    ),
    action(
        id::SCOPE_ALL,
        "",
        "Clear evidence scope",
        "Return related evidence to all authorized sources",
        Group::Evidence,
        &[Context::History],
    ),
    action(
        id::LIVE_PAUSE,
        "p",
        "Pause or resume live updates",
        "Collection continues while the display is frozen",
        Group::Live,
        &[Context::CommandCenter, Context::History, Context::Workbench],
    ),
    action(
        id::LIVE_REFRESH,
        "R",
        "Refresh now",
        "Re-read the current scope without waiting for the next poll",
        Group::Live,
        &[Context::CommandCenter, Context::History, Context::Workbench],
    ),
    action(
        id::DIAGNOSTICS,
        "d",
        "Open diagnostics",
        "Recorded findings and collection health",
        Group::Live,
        Context::every(),
    ),
    action(
        id::RETRY_CONNECTION,
        "",
        "Retry connection now",
        "Retry the current reader; unavailable data stays explicitly empty",
        Group::Live,
        &[Context::CommandCenter, Context::History, Context::Workbench],
    ),
    action(
        id::HISTORY_PATHS,
        "Ctrl+F",
        "Investigate recorded paths",
        "Search exact recorded paths in the retained scope",
        Group::Search,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::HISTORY_COMMANDS,
        "",
        "Investigate commands",
        "Search literal recorded command phrases",
        Group::Search,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::HISTORY_INSTRUCTIONS,
        "",
        "Investigate instructions",
        "Search literal recorded user instructions",
        Group::Search,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::HISTORY_FAILURES,
        "",
        "Investigate failures",
        "Reported lifecycle, tool, MCP, check, trace and log failures",
        Group::Search,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::HISTORY_FAILED_COMMAND,
        "",
        "Investigate failed commands",
        "Literal source-confirmed failed command phrases",
        Group::Search,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::SEARCH,
        "/",
        "Search this view",
        "Filter the visible page; Esc returns to the list",
        Group::Search,
        &[Context::CommandCenter, Context::History, Context::Workbench],
    ),
    action(
        id::REVIEW,
        "Ctrl+R / V",
        "Review recorded work",
        "Open exact scoped checks, edits and errors",
        Group::Review,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::PROMPTS,
        "P",
        "Prompt navigator",
        "Read recorded requests for the selected session",
        Group::Review,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::TRACES,
        "t",
        "Session traces",
        "Open the exact selected session or turn traces",
        Group::Review,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::TURN,
        "T",
        "Choose recorded turn",
        "Open one exact retained turn from this page",
        Group::Page,
        &[Context::History],
    ),
    action(
        id::MCP_PAYLOAD,
        "M",
        "MCP payloads",
        "Inspect retained call payloads where a grant allows",
        Group::Evidence,
        &[Context::History],
    ),
    action(
        id::FILE_EDITS,
        "e",
        "File edits",
        "Inspect retained file edits for the current turn or review",
        Group::Evidence,
        &[Context::History],
    ),
    action(
        id::REVIEW_NOTES,
        "n",
        "Review notes and decisions",
        "Record or revise this session's review decision",
        Group::Review,
        &[Context::History],
    ),
    action(
        id::MARK_REVIEWED,
        "y",
        "Mark review acknowledged",
        "Acknowledge this exact review version",
        Group::Review,
        &[Context::History],
    ),
    action(
        id::OLDER_PAGE,
        "]",
        "Older page",
        "Load the next older retained page in the same scope",
        Group::Page,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::NEWER_PAGE,
        "[",
        "Newer page",
        "Return to the previously loaded page",
        Group::Page,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::LATEST_PAGE,
        "l",
        "Latest page",
        "Return to the newest page in the same scope",
        Group::Page,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::COPY,
        "c / Ctrl+C",
        "Copy selected view",
        "Copy the loaded metadata view; Ctrl+C uses the same copy when focus permits",
        Group::Evidence,
        &[Context::CommandCenter, Context::History, Context::Workbench],
    ),
    action(
        id::EXPORT,
        "x",
        "Export selected view",
        "Prepare a metadata-only export of the loaded view",
        Group::Evidence,
        &[Context::CommandCenter, Context::History, Context::Workbench],
    ),
    action(
        id::INTERNAL_REVIEWS,
        "v",
        "Internal reviews",
        "Include or hide captured internal review metadata",
        Group::Session,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::WORKFLOW,
        "w",
        "Workflow inbox",
        "Pending requests, incidents and delivery settings",
        Group::Workflow,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::TASKS,
        "b",
        "Saved tasks",
        "Open, continue or prepare a saved task handoff",
        Group::Workflow,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::ATTENTION,
        "!",
        "Needs attention",
        "Filter the roster to pending input and recorded attention states",
        Group::Session,
        &[Context::CommandCenter],
    ),
    action(
        id::MARK_SEEN,
        "X",
        "Mark request seen",
        "Acknowledge one exact current pending request without resolving it",
        Group::Session,
        &[Context::CommandCenter],
    ),
    action(
        id::REVIEW_FILTER,
        "Y",
        "To review",
        "Show completed work that has not been reviewed",
        Group::Session,
        &[Context::CommandCenter],
    ),
    action(
        id::GROUP,
        "G",
        "Grouping",
        "Cycle project, source and ungrouped roster headings",
        Group::Session,
        &[Context::CommandCenter],
    ),
    action(
        id::TREE,
        "f / h",
        "Tree or flat roster",
        "Toggle hierarchy indentation for troubleshooting",
        Group::Session,
        &[Context::CommandCenter],
    ),
    action(
        id::RESET,
        "a",
        "Reset roster filters",
        "Clear search and return to all recorded sessions",
        Group::Session,
        &[Context::CommandCenter],
    ),
    action(
        id::FAMILY,
        "Alt+↑/↓/→",
        "Agent family",
        "Parent, first child or next descendant needing attention",
        Group::Session,
        &[Context::CommandCenter],
    ),
    action(
        id::OVERVIEW,
        "g / m",
        "Session overview",
        "Open the exact selected session overview",
        Group::Review,
        &[Context::CommandCenter],
    ),
    action(
        id::SESSION_ACTIVITY,
        "A / s",
        "Session activity",
        "Open recorded Activity for the exact selected session",
        Group::Review,
        &[Context::CommandCenter, Context::History],
    ),
    action(
        id::SESSION_USAGE,
        "u",
        "Session usage",
        "Open retained usage for the exact selected session",
        Group::Review,
        &[Context::CommandCenter],
    ),
    action(
        id::HOST_NOW,
        "H",
        "Host now",
        "Sample permitted local processes and collection status",
        Group::Live,
        &[Context::CommandCenter],
    ),
    action(
        id::INSPECTOR_MODE,
        "m",
        "Inspector section",
        "Cycle the inspector section from session content",
        Group::Review,
        &[Context::History],
    ),
    action(
        id::INSPECTOR_SECTIONS,
        "←/→ · Ctrl+←/→",
        "Choose inspector section",
        "Move the tab strip between Overview, Prompts and Details; Ctrl works from content",
        Group::Review,
        &[Context::History],
    ),
    action(
        id::INSPECT,
        "Enter",
        "Inspect selection",
        "Open the exact selected session, row or control",
        Group::Page,
        &[
            Context::CommandCenter,
            Context::History,
            Context::Workbench,
            Context::Investigation,
        ],
    ),
    action(
        id::BACK,
        "Esc",
        "Back one layer",
        "A chooser owns Escape before its enclosing screen",
        Group::Page,
        Context::every(),
    ),
    action(
        id::QUIT,
        "q",
        "Quit",
        "Quit from the cockpit; text fields keep ordinary typing",
        Group::System,
        &[
            Context::CommandCenter,
            Context::History,
            Context::Workbench,
            Context::Investigation,
        ],
    ),
];

pub(crate) fn find_action(id: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|action| action.id == id)
}

/// Actions that have a documented key but no unambiguous palette command
/// (modifier-plus-direction combinations stay on their control).
pub(crate) const PALETTE_EXCLUDED: &[&str] = &[id::FAMILY];

pub(crate) fn actions(context: Context) -> impl Iterator<Item = &'static Action> {
    ACTIONS
        .iter()
        .filter(move |action| action.contexts.contains(&context))
}

/// Deterministic footer text for a set of inventory identifiers.
pub(crate) fn hint(ids: &[&str]) -> String {
    ids.iter()
        .filter_map(|id| find_action(id))
        .map(|action| {
            if action.keys.is_empty() {
                action.label.to_owned()
            } else {
                format!("{} {}", action.keys, action.label)
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// One scrollable reference section per action group.
pub(crate) fn help_sections(context: Context) -> Vec<(&'static str, Vec<String>)> {
    let mut sections: Vec<(&'static str, Vec<String>)> = Vec::new();
    for action in actions(context) {
        let line = if action.keys.is_empty() {
            format!("{}  ·  {}", action.label, action.detail)
        } else {
            format!("{:<14}{}  ·  {}", action.keys, action.label, action.detail)
        };
        match sections
            .iter_mut()
            .find(|(group, _)| *group == action.group.label())
        {
            Some((_, lines)) => lines.push(line),
            None => sections.push((action.group.label(), vec![line])),
        }
    }
    sections
}

pub(crate) fn plain(key: KeyEvent) -> Option<KeyCode> {
    (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT).then_some(key.code)
}

pub(crate) fn interrupt(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c' | 'C')) && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub(crate) fn palette(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('p' | 'P')) && key.modifiers == KeyModifiers::CONTROL
}

pub(crate) fn review(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('r' | 'R')) && key.modifiers == KeyModifiers::CONTROL
}

pub(crate) fn back(key: KeyEvent) -> bool {
    key.code == KeyCode::Left && key.modifiers == KeyModifiers::ALT
}

pub(crate) fn is_help(code: KeyCode) -> bool {
    matches!(code, KeyCode::F(1) | KeyCode::Char('?'))
}

/// Enter and Space activate buttons where the platform treats them as one.
pub(crate) fn activate(key: KeyCode) -> bool {
    matches!(key, KeyCode::Enter | KeyCode::Char(' '))
}

/// Printable text keys belong to the focused editor, including `?`, digits,
/// brackets and punctuation.
pub(crate) fn text_key(key: KeyEvent) -> Option<char> {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    match key.code {
        KeyCode::Char(ch) if !ch.is_control() => Some(ch),
        _ => None,
    }
}

pub(crate) const OLDER: KeyCode = KeyCode::Char(']');
pub(crate) const NEWER: KeyCode = KeyCode::Char('[');
pub(crate) const LATEST: KeyCode = KeyCode::Char('l');

/// Keyboard focus owner used to resolve precedence and `Ctrl+C`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Focus {
    CommandCenter,
    AgentSearch,
    SessionChooser,
    HistoryTable,
    HistorySearch,
    EvidenceInspection,
    HistoryChooser,
    Workbench,
    WorkbenchEditor,
    Investigation,
    ExportForm,
    PrivatePrompt,
    Palette,
    Help,
}

impl Focus {
    /// Text fields keep literal letters, digits, brackets and punctuation.
    pub(crate) fn text_entry(self) -> bool {
        matches!(
            self,
            Self::AgentSearch
                | Self::HistorySearch
                | Self::WorkbenchEditor
                | Self::Investigation
                | Self::ExportForm
                | Self::SessionChooser
        )
    }

    /// A chooser consumes Escape before the enclosing form or screen.
    pub(crate) fn chooser(self) -> bool {
        matches!(
            self,
            Self::SessionChooser | Self::HistoryChooser | Self::Palette
        )
    }

    /// Public, copyable evidence focus. Private prompts and editors keep the
    /// terminal interrupt meaning for `Ctrl+C`.
    pub(crate) fn copyable(self) -> bool {
        matches!(
            self,
            Self::CommandCenter | Self::HistoryTable | Self::EvidenceInspection
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_root_destinations_stay_one_to_seven_and_extras_are_explicit() {
        for (key, destination) in [
            ('1', Destination::Agents),
            ('2', Destination::Activity),
            ('3', Destination::Mcp),
            ('4', Destination::Traces),
            ('5', Destination::Host),
            ('6', Destination::Metrics),
            ('7', Destination::Findings),
        ] {
            assert_eq!(self::destination(KeyCode::Char(key)), Some(destination));
        }
        assert_eq!(
            self::destination(KeyCode::Char('9')),
            Some(Destination::Operations)
        );
        assert_eq!(
            self::destination(KeyCode::Char('U')),
            Some(Destination::Usage)
        );
        assert!(navigation_hints(NAVIGATION).contains("7 Findings"));
        assert!(navigation_hints(EXTRA_NAVIGATION).contains("U Usage"));
    }

    #[test]
    fn every_context_has_help_and_every_action_id_is_unique() {
        use std::collections::BTreeSet;
        let mut ids = BTreeSet::new();
        for action in ACTIONS {
            assert!(ids.insert(action.id), "duplicate action {}", action.id);
            assert!(!action.label.is_empty());
            assert!(!action.contexts.is_empty());
        }
        for context in Context::every() {
            let sections = help_sections(*context);
            assert!(!sections.is_empty(), "{context:?} has no help");
            assert!(actions(*context).any(|action| action.id == id::HELP));
            assert!(actions(*context).any(|action| action.id == id::PALETTE));
            assert!(actions(*context).any(|action| action.id == id::THEMES));
        }
    }

    #[test]
    fn focus_rules_keep_text_and_chooser_precedence() {
        assert!(Focus::AgentSearch.text_entry());
        assert!(Focus::HistorySearch.text_entry());
        assert!(!Focus::HistoryTable.text_entry());
        assert!(Focus::SessionChooser.chooser());
        assert!(Focus::SessionChooser.text_entry());
        assert!(!Focus::HistoryTable.chooser());
        assert!(Focus::HistoryTable.copyable());
        assert!(!Focus::PrivatePrompt.copyable());
        assert!(activate(KeyCode::Enter) && activate(KeyCode::Char(' ')));
        assert!(!activate(KeyCode::Char('e')));
        assert_eq!(
            text_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)),
            Some('?')
        );
        assert_eq!(
            text_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn footer_hints_come_from_the_inventory() {
        let hint = hint(&[id::GO_AGENTS, id::THEMES, id::HELP]);
        assert!(hint.contains("1 Agents"));
        assert!(hint.contains("F2 Themes"));
        assert!(hint.contains("F1 / ? Contextual help"));
    }
}
