//! Shared navigation keys. Text editors and private modals own their input.

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
}

pub(crate) const NAVIGATION: &[(char, Destination, &str)] = &[
    ('1', Destination::Agents, "Agents"),
    ('2', Destination::Activity, "Activity"),
    ('3', Destination::Mcp, "MCP"),
    ('4', Destination::Traces, "Traces"),
    ('5', Destination::Host, "Host"),
    ('6', Destination::Metrics, "Metrics"),
    ('7', Destination::Findings, "Findings"),
    ('9', Destination::Operations, "Operations"),
];

pub(crate) fn navigation_hints(items: &[(char, Destination, &str)]) -> String {
    items
        .iter()
        .map(|(key, _, label)| format!("{key} {label}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

pub(crate) fn destination(code: KeyCode) -> Option<Destination> {
    let KeyCode::Char(number) = code else {
        return None;
    };
    NAVIGATION
        .iter()
        .find_map(|(key, dest, _)| (*key == number).then_some(*dest))
}

pub(crate) fn plain(key: KeyEvent) -> Option<KeyCode> {
    (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT).then_some(key.code)
}

pub(crate) fn interrupt(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c' | 'C')) && key.modifiers.contains(KeyModifiers::CONTROL)
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

pub(crate) const OLDER: KeyCode = KeyCode::Char(']');
pub(crate) const NEWER: KeyCode = KeyCode::Char('[');
pub(crate) const LATEST: KeyCode = KeyCode::Char('l');
