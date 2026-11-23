//! The tagged receiver supplies records, never assessments of the current worktree.

mod commands;

use super::{CapturePolicy, Event, Run, Scope, unsafe_display, valid_identity, valid_trace};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

const TITLE: &str = "Selected execution";
const CAPTURE_NOTE: &str =
    "Captured records do not establish complete runtime capture or the current worktree state.";
const USAGE_NOTE: &str = "Recorded run usage only; session lifetime, descendant totals and provider billing are separate.";

pub(super) fn build(
    scope: &Scope,
    run: Option<&Run>,
    events: &[Event],
    cursor: Option<String>,
    mut reasons: Vec<&'static str>,
    policy: &CapturePolicy,
    captured_at: f64,
) -> Value {
    let edits: Vec<_> = events
        .iter()
        .filter_map(|event| edit(event, scope))
        .collect();
    let mut checks = Vec::new();
    let mut unclassified = Vec::new();
    let mut errors = Vec::new();
    let mut context = Vec::new();
    let mut remaining_reference_bytes = 128 * 1024;
    let mut comparable = BTreeMap::<String, Vec<(usize, u64)>>::new();
    for event in events {
        let source = || reference(event, scope);
        let facts = commands::Facts::from_event(event);
        if event.domain == "shell" {
            if let Some(analysis) = commands::analyze(&facts.command) {
                let terminal =
                    if !facts.command_complete || (analysis.compound && facts.result != "passed") {
                        "unknown"
                    } else {
                        facts.result
                    };
                let completed = if terminal == "unknown" {
                    None
                } else {
                    event
                        .end_unix_nano
                        .or(event.time_unix_nano)
                        .filter(|stamp| *stamp > 0)
                };
                let later = edits.iter().filter_map(|edit| {
                    let stamp = edit["source"]["time_unix_nano"].as_u64()?;
                    completed.filter(|done| stamp > *done)?;
                    Some(&edit["source"])
                });
                let later_count = later.clone().count();
                let mut later_refs = Vec::new();
                for source in later.take(8) {
                    let bytes = source.to_string().len();
                    if bytes > remaining_reference_bytes {
                        break;
                    }
                    remaining_reference_bytes -= bytes;
                    later_refs.push(source);
                }
                let basis = if !facts.command_complete {
                    "command is a display label or captured input is incomplete"
                } else if analysis.compound && terminal == "unknown" {
                    "compound shell result; individual failure unavailable"
                } else {
                    facts.basis
                };
                if let Some((fingerprint, completed)) = facts.fingerprint.as_ref().zip(completed)
                    && !scope.agent_id.is_empty()
                {
                    comparable
                        .entry(fingerprint.clone())
                        .or_default()
                        .push((checks.len(), completed));
                }
                checks.push(json!({
                    "source": source(), "command": facts.command, "normalized_command": analysis.command,
                    "compound": analysis.compound, "kind": analysis.kind, "result": terminal,
                    "exit_code": facts.exit_code, "result_basis": basis, "completed_ns": completed,
                    "later_edits": later_refs, "later_edit_count": later_count,
                    "later_edits_clipped": later_refs.len() < later_count,
                    "freshness": if later_count > 0 { "later recorded edits" } else { "freshness unknown" },
                }));
            } else {
                unclassified.push(json!({
                    "source": source(), "command": facts.command, "result": safe_text(&event.result),
                }));
            }
        }
        if !facts.informational
            && (facts.result == "failed" || facts.conflict || commands::failure(&event.result))
        {
            let (detail, clipped) = clip(&facts.output, 1_500);
            errors.push(json!({
                "source": source(), "subject": facts.command, "exit_code": facts.exit_code,
                "detail": detail, "detail_clipped": clipped || facts.output_clipped,
                "output_kind": facts.output_kind, "output_origin": facts.output_origin,
                "result": facts.result, "result_basis": facts.basis,
                "operation_version": 1, "operation_fingerprint": facts.fingerprint, "error_fingerprint": null,
            }));
        }
        if matches!(
            event.action.as_str(),
            "compaction" | "compact" | "truncation" | "context_truncated"
        ) {
            context.push(json!({ "source": source(), "subject": safe_text(&event.subject) }));
        }
    }
    for attempts in comparable.values() {
        if let Some(latest) = attempts.iter().map(|(_, stamp)| stamp).max() {
            for (index, stamp) in attempts {
                checks[*index]["latest_comparable"] = json!(stamp == latest);
                checks[*index]["comparable_attempt_count"] = json!(attempts.len());
            }
        }
    }
    if !policy.denied && events.is_empty() {
        reasons
            .push("No scoped activity loaded; collection, indexing or capture may be unavailable");
    }
    if events.iter().any(|row| row.detail_clipped == Some(true)) {
        reasons.push("Some captured source text is clipped");
    }
    let stamps: Vec<_> = events.iter().filter_map(|row| row.time_unix_nano).collect();
    if stamps.is_empty() || stamps.len() != events.len() {
        reasons.push("Some timestamps unavailable");
    }
    let (usage, outcome, turn_reasons) = usage(scope, run.filter(|_| !policy.denied));
    let mut reasons: Vec<_> = reasons.into_iter().map(str::to_owned).collect();
    for reason in turn_reasons {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
    let coverage = if reasons.is_empty() {
        "loaded captured interval"
    } else {
        "partial"
    };
    json!({
        "format": "agenturm-review-v1",
        "title": if policy.denied { "Selected execution (metadata-only)" } else { TITLE },
        "scope": scope, "captured_at": captured_at, "reported_outcome": outcome,
        "edits": edits, "checks": checks, "unclassified_commands": unclassified, "usage": usage,
        "errors": errors, "context_events": context,
        "window": {
            "earliest_ns": stamps.iter().min(), "latest_ns": stamps.iter().max(), "count": events.len(),
            "next_cursor": cursor, "scope_revision": null, "capture_revision": policy.revision(),
            "coverage": coverage, "reasons": reasons, "note": CAPTURE_NOTE,
        },
        "request": latest_message("user", scope, events),
        "agent_update": latest_message("assistant", scope, events),
    })
}

fn reference(event: &Event, scope: &Scope) -> Value {
    let mut out = Map::new();
    out.insert("receiver_id".into(), json!(scope.receiver));
    if let Some(id) = event.id.as_ref().filter(|id| match id {
        Value::Number(number) => number.is_i64() || number.is_u64(),
        Value::String(id) => valid_identity(id),
        _ => false,
    }) {
        out.insert("id".into(), id.clone());
    }
    for (key, value) in [
        ("source_id", event.source_id.as_deref()),
        ("trace_id", event.trace_id.as_deref()),
        ("span_id", event.span_id.as_deref()),
        ("service", event.service.as_deref()),
        ("agent_id", event.agent_id.as_deref()),
        ("session_id", event.session_id.as_deref()),
        ("turn_id", event.turn_id.as_deref()),
    ] {
        if let Some(value) = value.filter(|id| valid_identity(id)) {
            out.insert(key.into(), json!(value));
        }
    }
    if let Some(stamp) = event.time_unix_nano {
        out.insert("time_unix_nano".into(), json!(stamp));
    }
    Value::Object(out)
}

fn usage(scope: &Scope, run: Option<&Run>) -> (Value, &'static str, Vec<String>) {
    let is_turn = !scope.turn_id.is_empty();
    let exact_owner = run.is_some_and(|run| {
        run.review_owner
            .as_ref()
            .is_some_and(|owner| owner.matches(scope))
    });
    let exact_usage = run.is_some_and(|run| {
        exact_owner
            && run.agent_count == Some(1)
            && run.agent_count_complete == Some(true)
            && (!is_turn || run.turn_usage_complete == Some(true))
    });
    let mut values = Map::new();
    if let Some(run) = run.filter(|_| exact_usage) {
        for key in [
            "input_tokens",
            "output_tokens",
            "cache_read_input_tokens",
            "cache_write_input_tokens",
            "reasoning_output_tokens",
            "unknown_cost_runs",
        ] {
            if let Some(value) = run
                .usage
                .get(key)
                .filter(|value| value.is_null() || value.is_u64())
            {
                values.insert(key.into(), value.clone());
            }
        }
        for key in ["cost_usd", "known_cost_usd"] {
            if let Some(value) = run.usage.get(key).filter(|value| {
                value.is_null() || value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)
            }) {
                values.insert(key.into(), value.clone());
            }
        }
        for key in [
            "input_tokens_reported",
            "output_tokens_reported",
            "cache_read_input_tokens_reported",
            "cache_write_input_tokens_reported",
            "reasoning_output_tokens_reported",
            "cost_reported",
            "cost_complete",
            "usage_complete",
        ] {
            if let Some(value) = run.usage.get(key).filter(|value| value.is_boolean()) {
                values.insert(key.into(), value.clone());
            }
        }
    }
    let reasons = run.filter(|_| is_turn).map_or_else(Vec::new, |run| {
        run.turn_coverage_reasons
            .iter()
            .take(16)
            .map(|text| clip(&safe_text(text), 200).0.to_owned())
            .collect()
    });
    let scope_label = if !exact_usage {
        "unavailable"
    } else if !is_turn {
        "exact run"
    } else if run.and_then(|run| run.turn_membership.as_deref())
        == Some("recorded idle-boundary interval")
    {
        "recorded turn interval"
    } else {
        "recorded user turn"
    };
    let outcome = if !is_turn && exact_owner {
        match run.and_then(|run| run.task_outcome.as_deref()) {
            Some("finished" | "completed" | "success") => "Completed",
            Some("failed" | "error") => "Failed",
            Some("waiting" | "blocked") => "Waiting",
            Some("active" | "running") => "Working",
            Some("interrupted" | "cancelled") => "Interrupted",
            _ => "Unknown",
        }
    } else {
        "Unknown"
    };
    let steps: Vec<_> = run
        .filter(|_| is_turn)
        .into_iter()
        .flat_map(|run| run.step_trace_ids.iter().filter(|id| valid_trace(id)))
        .take(1_000)
        .collect();
    (
        json!({
            "scope": scope_label, "trace_id": scope.run_id, "turn_coverage_reasons": reasons,
            "turn_closed": run.filter(|_| is_turn).and_then(|run| run.turn_closed),
            "step_trace_ids": steps, "agent_count": run.and_then(|run| run.agent_count),
            "allocation": if exact_usage { "single recorded agent" } else { "agent allocation unverified" },
            "values": values, "note": USAGE_NOTE,
        }),
        outcome,
        reasons,
    )
}

fn edit(event: &Event, scope: &Scope) -> Option<Value> {
    if event.domain != "file" {
        return None;
    }
    let action = match event.action.to_ascii_lowercase().as_str() {
        "write" => "write",
        "edit" | "update" => "edit",
        "add" | "create" => "create",
        "delete" | "remove" => "delete",
        "patch" => "patch",
        "rename" | "move" => "rename",
        _ => return None,
    };
    let mut paths = Vec::new();
    let mut from = BTreeSet::new();
    let mut to = BTreeSet::new();
    let mut valid = true;
    let mut add = |value: &str, role: &str| {
        if value.is_empty() {
            return;
        }
        if value.len() > 4_096 || value.chars().any(unsafe_display) {
            valid = false;
            return;
        }
        let path = json!({ "path": value, "role": role });
        if !paths.contains(&path) {
            paths.push(path);
        }
        match role {
            "from" => {
                from.insert(value.to_owned());
            }
            "to" => {
                to.insert(value.to_owned());
            }
            _ => {}
        }
    };
    add(&event.subject, "recorded");
    if action == "rename" {
        for value in [&event.from_path, &event.old_path].into_iter().flatten() {
            add(value, "from");
        }
        for value in [&event.to_path, &event.new_path].into_iter().flatten() {
            add(value, "to");
        }
        // Typed file input, never shell/MCP output, may provide explicit paths.
        let structured;
        let detail = match event.detail.as_ref() {
            Some(Value::String(text))
                if event.detail_clipped != Some(true) && text.len() <= 8_000 =>
            {
                structured = serde_json::from_str::<Value>(text).ok();
                structured.as_ref()
            }
            Some(value @ Value::Object(_)) if event.detail_clipped != Some(true) => Some(value),
            _ => None,
        };
        if let Some(detail) =
            detail.filter(|_| event.detail_role.as_deref() != Some("operation_v1"))
        {
            let mut envelopes = vec![detail];
            for field in ["rename", "move", "Rename", "Move"] {
                if let Some(value) = detail.get(field).or_else(|| detail.get("type")?.get(field)) {
                    envelopes.push(value);
                }
            }
            for value in envelopes {
                for field in ["from_path", "old_path", "from"] {
                    if let Some(path) = value.get(field).and_then(Value::as_str) {
                        add(path, "from");
                    }
                }
                for field in ["to_path", "new_path", "to"] {
                    if let Some(path) = value.get(field).and_then(Value::as_str) {
                        add(path, "to");
                    }
                }
            }
        }
    }
    let complete =
        valid && !paths.is_empty() && (action != "rename" || (from.len() == 1 && to.len() == 1));
    Some(json!({
        "source": reference(event, scope), "path": clip(&safe_text(&event.subject), 4_096).0, "action": action,
        "source_action": safe_text(&event.action), "paths": paths, "paths_complete": complete,
        "from_path": complete.then(|| from.first()).flatten(), "to_path": complete.then(|| to.first()).flatten(),
        "result": if event.result.is_empty() { "not reported".into() } else { safe_text(&event.result) },
        "effect": if matches!(event.result.as_str(), "success" | "ok" | "completed") { "recorded success" } else { "attempt; effect unknown" },
    }))
}

fn latest_message(action: &str, scope: &Scope, events: &[Event]) -> Option<Value> {
    for row in events
        .iter()
        .rev()
        .filter(|row| row.domain == "message" && row.action == action)
    {
        let mut text = String::new();
        if let Some(detail) = &row.detail {
            message_text(detail, 0, &mut text);
        }
        if text.trim().is_empty() {
            text = safe_text(&row.subject);
        }
        if text.trim().is_empty()
            || matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "assistant message" | "user message" | "message" | "user prompt"
            )
        {
            continue;
        }
        let (body, context) = if action == "user" {
            request_body(&text)
        } else {
            (text.as_str(), Vec::new())
        };
        if body.trim().is_empty() {
            continue;
        }
        let (body, clipped) = clip(body, 2_000);
        return Some(json!({
            "text": body, "source": reference(row, scope),
            "clipped": clipped || row.detail_clipped == Some(true), "context_omitted": context,
        }));
    }
    None
}

fn message_text(value: &Value, depth: usize, output: &mut String) {
    if depth > 12 || output.len() >= 32_768 {
        return;
    }
    match value {
        Value::String(text) => output.push_str(&safe_text(text)),
        Value::Object(record) => {
            for key in ["text", "content", "message", "prompt", "recorded_content"] {
                if let Some(value) = record.get(key) {
                    let prior = output.len();
                    message_text(value, depth + 1, output);
                    if output.len() > prior {
                        return;
                    }
                }
            }
        }
        Value::Array(entries) => {
            for entry in entries {
                let previous = output.len();
                if previous > 0 {
                    output.push('\n');
                }
                let before = output.len();
                message_text(entry, depth + 1, output);
                if output.len() == before {
                    output.truncate(previous);
                }
            }
        }
        _ => {}
    }
}

fn request_body(text: &str) -> (&str, Vec<String>) {
    const TAGS: &[(&str, &str)] = &[
        ("environment_context", "Environment context"),
        ("environment_details", "Environment details"),
        ("skills_instructions", "Skills instructions"),
        ("user_instructions", "User instructions"),
        ("developer_instructions", "Developer instructions"),
        ("system_instructions", "System instructions"),
        ("system-reminder", "System reminder"),
        ("INSTRUCTIONS", "Repository instructions"),
        ("skill", "Skill"),
        ("local-command-caveat", "Local command caveat"),
        ("local-command-stdout", "Local command stdout"),
        ("local-command-stderr", "Local command stderr"),
    ];
    let mut body = text;
    let mut context = Vec::new();
    for _ in 0..32 {
        let mut remaining = body.trim_start();
        if remaining.starts_with("# AGENTS.md instructions for ")
            && let Some((_, rest)) = remaining.split_once('\n')
            && rest.trim_start().starts_with("<INSTRUCTIONS>")
        {
            remaining = rest.trim_start();
        }
        let found = TAGS.iter().find_map(|(tag, label)| {
            let value = remaining.strip_prefix('<')?.strip_prefix(tag)?;
            if !value.starts_with(['>', ' ', '\t', '\n']) {
                return None;
            }
            let (_, value) = value.split_once('>')?;
            let (_, tail) = value.split_once(&format!("</{tag}>"))?;
            Some((tail, *label))
        });
        let Some((tail, label)) = found else { break };
        context.push(label.into());
        body = tail.trim_start();
    }
    (body, context)
}

pub(super) fn safe_text(value: &str) -> String {
    let mut result = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    while let Some(ch) = chars.peek().copied() {
                        if !(' '..='~').contains(&ch) {
                            break;
                        }
                        chars.next();
                        if ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(ch) = chars.next() {
                        if ch == '\u{7}' {
                            break;
                        }
                        if ch == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if ch == '\r' {
            if chars.peek() != Some(&'\n') {
                result.push('\n');
            }
        } else if ch == '\n' || ch == '\t' || !unsafe_display(ch) {
            result.push(ch);
        }
    }
    result
}

pub(super) fn clip(text: &str, limit: usize) -> (&str, bool) {
    match text.char_indices().nth(limit) {
        Some((index, _)) => (&text[..index], true),
        None => (text, false),
    }
}
