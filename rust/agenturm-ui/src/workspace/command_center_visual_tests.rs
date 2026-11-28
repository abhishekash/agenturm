//! Reproducible, public-metadata-only visual fixtures for the command center.
//!
//! Run this test in a fresh process with NO_COLOR=1 for monochrome coverage.
//! AGENTURM_UI_CAPTURE_DIR optionally exports the actual TestBackend cells;
//! scripts/render_rust_command_center.py renders those cells for visual review.

use super::*;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use serde_json::{Value, json};

fn task(id: &str, title: &str, state: &str, project: &str, minutes: u64) -> RecordedSession {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    RecordedSession {
        harness_id: "harness.codex".into(),
        session_id: id.into(),
        worker_id: None,
        task_title: Some(title.into()),
        native_context: None,
        project_path: Some(project.into()),
        project_origin: Some(ProjectOrigin::NativeRecordedWorkspace),
        updated_at_unix_ns: now.saturating_sub(minutes * 60_000_000_000),
        snapshot_id: 42.into(),
        state: Some(state.into()),
        reason: None,
        turn_id: None,
        parent_session_id: None,
        relationship_kind: None,
        review_version: ReviewVersion {
            receipt_sequence: 1,
            retained_observations: 1,
        },
        reviewed: false,
        history_identity: None,
        pending_requests: Vec::new(),
    }
}

fn fixture() -> AgentList {
    let project = "/home/developer/workspaces/platform/team/agenturm";
    let mut parent = task(
        "fixture-parent-opaque",
        "Implement agent tool hierarchy",
        "running",
        project,
        2,
    );
    parent.native_context = Some(Box::new(NativeSessionContext {
        last_activity: Some("Verified tool permissions and session routing".into()),
        model_provider: Some("openai".into()),
        ..NativeSessionContext::default()
    }));
    let mut worker = task("fixture-worker-opaque", "Mira", "waiting", project, 3);
    worker.parent_session_id = Some(parent.session_id.clone());
    worker.relationship_kind = Some(RelationshipKind::Delegated);
    worker.worker_id = Some("worker-mira".into());
    let mut reviewer = task("fixture-reviewer-opaque", "Sol", "completed", project, 4);
    reviewer.parent_session_id = Some(parent.session_id.clone());
    reviewer.relationship_kind = Some(RelationshipKind::Delegated);
    reviewer.worker_id = Some("worker-sol".into());
    let mut unicode = task(
        "fixture-unicode-opaque",
        "修复重试 · Cafe\u{301} 👩‍💻",
        "completed",
        project,
        7,
    );
    unicode.native_context = Some(Box::new(NativeSessionContext {
        agent_name: Some("Unicode checks".into()),
        ..NativeSessionContext::default()
    }));
    let mut claude = task(
        "fixture-claude-opaque",
        "Review release notes",
        "completed",
        project,
        12,
    );
    claude.harness_id = "harness.claude".into();
    let sessions = vec![
        parent,
        worker,
        reviewer,
        task(
            "fixture-prompts-opaque",
            "Repair prompt history",
            "completed",
            project,
            5,
        ),
        unicode,
        task(
            "fixture-release-opaque",
            "Publish native bundle",
            "completed",
            project,
            9,
        ),
        claude,
        task(
            "fixture-failure-opaque",
            "Investigate timeout regression",
            "failed",
            project,
            15,
        ),
        task(
            "fixture-layout-opaque",
            "Refine task list spacing",
            "completed",
            project,
            20,
        ),
        task(
            "fixture-docs-opaque",
            "Document supported terminals",
            "completed",
            project,
            25,
        ),
        task(
            "fixture-unknown-opaque",
            "Imported session",
            "unrecognized",
            project,
            30,
        ),
        task(
            "fixture-idle-opaque",
            "Update packaging checks",
            "idle",
            project,
            35,
        ),
        task(
            "fixture-other-opaque",
            "Assess Rust plugin architecture",
            "completed",
            "/home/developer/workspaces/research/lab/agenturm",
            50,
        ),
        task(
            "fixture-other2-opaque",
            "Prepare API integration",
            "running",
            "/work/customer-portal",
            60,
        ),
    ];
    let mut agents = AgentList::default();
    agents.update(SessionCatalog {
        sessions,
        older: Some(SessionCursor {
            harness_id: "harness.codex".into(),
            session_id: "fixture-cursor-opaque".into(),
            updated_at_unix_ns: 1,
            snapshot_id: 42.into(),
        }),
        notice: None,
        day_start_unix_ns: 1,
    });
    agents
}

fn render(
    agents: &mut AgentList,
    width: u16,
    height: u16,
    pending: bool,
    message: Option<&str>,
) -> Buffer {
    // Optional visual-review runs load a real theme preference from the
    // caller's isolated XDG_CONFIG_HOME. The picker reads it through the same
    // API as the application; no test mutates environment or user settings.
    let _appearance = std::env::var_os("AGENTURM_UI_CAPTURE_THEME").map(|_| theme::Picker::new());
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_widget(
                Block::default().style(surface(theme::palette().canvas)),
                frame.area(),
            );
            draw_agents(frame, agents, pending, message);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn lines(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

fn color(color: Color) -> Value {
    match color {
        Color::Reset => Value::Null,
        Color::Rgb(r, g, b) => json!([r, g, b]),
        Color::Indexed(index) => json!({"indexed": index}),
        other => json!(format!("{other:?}")),
    }
}

fn capture(name: &str, buffer: &Buffer) {
    let Some(directory) = std::env::var_os("AGENTURM_UI_CAPTURE_DIR") else {
        return;
    };
    let directory = std::path::Path::new(&directory);
    std::fs::create_dir_all(directory).unwrap();
    let cells = buffer
        .content()
        .iter()
        .map(|cell| {
            json!({
                "symbol": cell.symbol(),
                "fg": color(cell.fg),
                "bg": color(cell.bg),
                "bold": cell.modifier.contains(Modifier::BOLD),
                "reverse": cell.modifier.contains(Modifier::REVERSED),
                "underline": cell.modifier.contains(Modifier::UNDERLINED),
            })
        })
        .collect::<Vec<_>>();
    let profile = if colors() { "color" } else { "no-color" };
    let appearance =
        std::env::var("AGENTURM_UI_CAPTURE_THEME").unwrap_or_else(|_| "Observation".into());
    let suffix = if appearance == "Observation" {
        String::new()
    } else {
        format!("-{}", appearance.to_ascii_lowercase())
    };
    let path = directory.join(format!(
        "{name}-{}x{}-{profile}{suffix}.json",
        buffer.area.width, buffer.area.height
    ));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "name": name,
            "profile": profile,
            "theme": appearance,
            "width": buffer.area.width,
            "height": buffer.area.height,
            "cells": cells,
            "text": lines(buffer).join("\n"),
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn command_center_pinned_frames_keep_tasks_and_controls_readable() {
    for (width, height) in [(160, 40), (120, 32), (120, 24), (80, 24), (60, 18)] {
        let mut agents = fixture();
        let identity = session_key(agents.current().unwrap());
        let buffer = render(&mut agents, width, height, false, None);
        let rendered = lines(&buffer);
        let text = rendered.join("\n");
        for expected in [
            "Views",
            "1 Agents",
            "Tasks",
            "Implement agent tool",
            "All 14",
            "Errors 1",
            "Enter inspect",
            "? help",
        ] {
            assert!(
                text.contains(expected),
                "{width}×{height}: missing {expected}\n{text}"
            );
        }
        assert_eq!(
            session_key(agents.current().unwrap()),
            identity,
            "resize preserves exact selection"
        );
        assert!(!text.contains("Mira"), "delegated agents start collapsed");
        assert_eq!(
            text.contains("Selected task"),
            width >= 120,
            "preview folds away before reducing task readability"
        );
        assert!(
            !text.contains("fixture-parent-opaque"),
            "opaque identifiers belong in details"
        );
        assert!(
            !text.contains("codex_task_complete"),
            "raw lifecycle codes are unnecessary roster chrome"
        );
        assert!(
            rendered.last().unwrap().contains("? help"),
            "footer remains reachable at the floor"
        );
        if width >= 160 {
            assert!(
                text.contains("team/agenturm"),
                "same-basename projects need distinguishing headings"
            );
            assert!(
                text.contains("lab/agenturm"),
                "same-basename projects need distinguishing headings"
            );
            assert!(
                text.contains("Prompt history"),
                "preview advertises a direct session action"
            );
        }
        if !colors() {
            assert!(
                buffer
                    .content()
                    .iter()
                    .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset),
                "NO_COLOR drops palette values"
            );
            assert!(
                buffer
                    .content()
                    .iter()
                    .any(|cell| cell.modifier.contains(Modifier::REVERSED)),
                "selection remains visible without color"
            );
        }
        capture("tasks", &buffer);
    }
}

#[test]
fn command_center_navigation_is_one_row_and_preserves_task_screen_budget() {
    for (width, height) in [(120, 32), (80, 24), (60, 18)] {
        let mut agents = fixture();
        let rendered = lines(&render(&mut agents, width, height, false, None));
        let navigation = &rendered[1];
        assert!(
            navigation.contains("Views"),
            "{width}×{height}: {navigation}"
        );
        assert!(
            navigation.contains("[1 Agents]")
                && navigation.contains("2 Activity")
                && navigation.contains("3 MCP"),
            "primary navigation remains discoverable at {width}×{height}: {navigation}"
        );
        if width < 100 {
            assert!(navigation.contains(": More views"), "{navigation}");
        } else {
            assert!(navigation.contains("7 Findings"), "{navigation}");
        }
        assert!(
            !navigation.contains("╭") && !navigation.contains("╰"),
            "navigation is a single flat row, not a nested panel"
        );
        assert!(
            rendered[4].contains("Tasks"),
            "task list begins at row 5 after the one-row navigation: {width}×{height}\n{}",
            rendered.join("\n")
        );
    }

    let mut agents = fixture();
    let rendered = lines(&render(&mut agents, 80, 24, false, None)).join("\n");
    for task in [
        "Implement agent tool hierarchy",
        "Repair prompt history",
        "Publish native bundle",
        "Investigate timeout regression",
        "Prepare API integration",
    ] {
        assert!(
            rendered.contains(task),
            "compact task list lost {task}:\n{rendered}"
        );
    }
}

#[test]
fn command_center_pinned_frames_distinguish_loading_empty_and_failure() {
    for (width, height) in [(80, 24), (60, 18)] {
        for (name, loaded, pending, message, expected) in [
            ("loading", false, true, None, "Loading session evidence"),
            ("empty", true, false, None, "No sessions collected yet"),
            (
                "unavailable",
                false,
                false,
                Some("Receiver disconnected"),
                "Session history unavailable",
            ),
        ] {
            let mut agents = AgentList::default();
            if loaded {
                agents.update(SessionCatalog {
                    sessions: vec![],
                    older: None,
                    notice: None,
                    day_start_unix_ns: 1,
                });
            }
            let buffer = render(&mut agents, width, height, pending, message);
            let text = lines(&buffer).join("\n");
            assert!(text.contains(expected), "{width}×{height}: {name}\n{text}");
            if let Some(message) = message {
                assert!(text.contains(message), "collection failures remain visible");
            }
            capture(name, &buffer);
        }
        let mut agents = fixture();
        let buffer = render(
            &mut agents,
            width,
            height,
            false,
            Some(
                "2 source failures · 0 collection failures\nFull diagnostics remain available in help.",
            ),
        );
        let text = lines(&buffer).join("\n");
        assert!(text.contains("2 source failures"));
        assert!(text.contains("? details"));
        assert!(
            text.contains("Implement agent tool"),
            "partial collection retains usable history"
        );
        capture("partial", &buffer);
    }
}

#[test]
fn command_center_pinned_frames_preserve_unicode_graphemes_when_filtered() {
    for (width, height) in [(120, 32), (80, 24), (60, 18)] {
        let mut agents = fixture();
        agents.search = "Cafe\u{301}".into();
        agents.apply_filters(None);
        assert_eq!(agents.visible.len(), 1);
        let selected = session_key(agents.current().unwrap());
        let buffer = render(&mut agents, width, height, false, None);
        for symbol in ["修", "复", "重", "试", "e\u{301}", "👩‍💻"] {
            assert!(
                buffer.content().iter().any(|cell| cell.symbol() == symbol),
                "{width}×{height}: the grapheme {symbol} remains intact"
            );
        }
        assert_eq!(session_key(agents.current().unwrap()), selected);
        assert!(
            lines(&buffer).join("\n").contains("/ Cafe"),
            "active search stays visible at every width"
        );
        capture("unicode-search", &buffer);
    }
}

#[test]
fn command_center_pinned_frames_single_provider_avoids_repeated_boilerplate() {
    for (width, height) in [(160, 40), (120, 32), (120, 24), (80, 24), (60, 18)] {
        let mut agents = fixture();
        let mut catalog = agents.catalog.take().unwrap();
        for row in &mut catalog.sessions {
            row.harness_id = "harness.codex".into();
        }
        agents.update(catalog);
        let buffer = render(&mut agents, width, height, false, None);
        let text = lines(&buffer).join("\n");
        assert!(
            text.contains("Codex"),
            "the single provider remains discoverable"
        );
        assert!(
            text.matches("Codex").count() <= 2,
            "{width}×{height}: provider is shared context, not repeated beneath every task\n{text}"
        );
        assert!(
            text.contains("Repair prompt history"),
            "the floor keeps several tasks visible"
        );
        capture("tasks-single-provider", &buffer);
    }
}
