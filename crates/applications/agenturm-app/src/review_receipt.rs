//! Formats a bounded Review page from the metadata-only history projection.
//!
//! The receipt mirrors the reviewed-work sections the Python 0.2.8 Review
//! screen exposes: the latest captured request, the agent-reported update,
//! recorded checks, modifying operations, errors, usage, context events and an
//! excerpt index for saving one recorded item to a task. Provider-specific
//! payloads are never reconstructed here; a missing section is labelled as
//! unavailable rather than inferred from the task title or a completion state.

use crate::handoff_export::{export_documents, html_sections, markdown_sections};
use agenturm_kernel::ReadSnapshot;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

const TITLE: &str = "Review receipt";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReviewFormat {
    Json,
    Text,
    Markdown,
    Html,
}

impl ReviewFormat {
    pub(super) fn parse(format: &str) -> Result<Self, String> {
        match format {
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            "markdown" => Ok(Self::Markdown),
            "html" => Ok(Self::Html),
            _ => Err("Choose a review format from text, json, markdown, or html".into()),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReviewOutput {
    pub(super) format: Option<ReviewFormat>,
    pub(super) directory: Option<PathBuf>,
}

pub(super) struct ReviewScope<'a> {
    pub harness_id: &'a str,
    pub session_id: &'a str,
    pub turn_id: Option<&'a str>,
    pub trace_id: Option<&'a str>,
    pub worker_id: Option<&'a str>,
}

#[derive(Default)]
struct ReviewParts {
    request: Option<String>,
    update: Option<String>,
    checks: Vec<ReviewItem>,
    edits: Vec<ReviewItem>,
    errors: Vec<ReviewItem>,
    usage: Vec<String>,
    context: Vec<String>,
}

struct ReviewItem {
    label: String,
    excerpt: String,
}

impl ReviewItem {
    fn new(label: impl Into<String>, excerpt: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            excerpt: excerpt.into(),
        }
    }
}

pub(super) fn render(
    mut page: Value,
    scope: ReviewScope<'_>,
    snapshot_id: ReadSnapshot,
    output: &ReviewOutput,
) -> Result<String, String> {
    let encode = |value: &Value| {
        serde_json::to_string_pretty(value).map_err(|_| "Cannot encode review receipt".to_owned())
    };
    let legacy = if output.format.is_none() {
        let existing = encode(&page)?;
        if output.directory.is_none() {
            return Ok(existing);
        }
        Some(existing)
    } else {
        None
    };
    page["format"] = json!("agenturm-review-receipt/1");
    page["snapshot_id"] = json!(snapshot_id.to_string());
    page["scope"] = json!({
        "source_id": scope.harness_id,
        "harness_id": scope.harness_id,
        "session_id": scope.session_id,
        "turn_id": scope.turn_id,
        "trace_id": scope.trace_id,
        "worker_id": scope.worker_id,
    });
    if scope.turn_id.is_none() {
        page["scope"].as_object_mut().unwrap().remove("turn_id");
    }
    if scope.trace_id.is_none() {
        page["scope"].as_object_mut().unwrap().remove("trace_id");
    }
    page["scope"]["worker_scope"] = json!(if scope.worker_id.is_some() {
        "exact"
    } else if scope.turn_id.is_none() {
        "all_recorded"
    } else {
        "workerless"
    });
    let parts = classify(&page["observations"]);
    let has_more = page["has_more"].as_bool().unwrap_or(false);
    page["summary"] = json!({
        "records": parts.checks.len() + parts.edits.len() + parts.errors.len(),
        "checks": check_counts(&parts.checks),
        "has_more": has_more,
        "next_step": guidance(&parts.checks, has_more),
        "request_available": parts.request.is_some(),
        "usage_reported": !parts.usage.is_empty(),
    });
    page["excerpts"] =
        json!(
            parts
                .checks
                .iter()
                .map(|item| json!({"kind": "check", "label": &item.label, "source": &item.excerpt}))
                .chain(parts.edits.iter().map(
                    |item| json!({"kind": "edit", "label": &item.label, "source": &item.excerpt})
                ))
                .chain(parts.errors.iter().map(
                    |item| json!({"kind": "error", "label": &item.label, "source": &item.excerpt})
                ))
                .collect::<Vec<_>>()
        );
    if output.format == Some(ReviewFormat::Json) && output.directory.is_none() {
        return encode(&page);
    }
    let worker = scope.worker_id.map_or_else(
        || {
            if scope.turn_id.is_none() {
                "All recorded workers".into()
            } else {
                "Workerless".into()
            }
        },
        |id| json!(id).to_string(),
    );
    let mut scope_summary = format!(
        "Source   {}\nSession  {}",
        json!(scope.harness_id),
        json!(scope.session_id)
    );
    if let Some(turn) = scope.turn_id {
        scope_summary.push_str(&format!("\nTurn     {}", json!(turn)));
    }
    if let Some(trace) = scope.trace_id {
        scope_summary.push_str(&format!("\nTrace    {}", json!(trace)));
    }
    scope_summary.push_str(&format!("\nWorker   {worker}\nSnapshot {snapshot_id}"));
    let mut sections = vec![("Scope", scope_summary)];
    sections.push(("Summary", summary(&parts, has_more)));
    sections.push((
        "Latest captured request",
        parts.request.clone().unwrap_or_else(|| {
            "Not available in loaded evidence; the task label is not a reconstructed request."
                .into()
        }),
    ));
    if let Some(update) = &parts.update {
        sections.push(("Agent-reported update", update.clone()));
    }
    sections.push(("Recorded checks", items_or(
        &parts.checks,
        "No recognized validation records in the loaded evidence. This does not establish that no validation ran.",
    )));
    sections.push((
        "Recorded modifying operations",
        items_or(
            &parts.edits,
            "No file operations captured in this evidence page.",
        ),
    ));
    sections.push((
        "Recorded errors",
        items_or(
            &parts.errors,
            "None in the loaded evidence. Later recovery is not inferred from completion.",
        ),
    ));
    sections.push((
        "Recorded usage",
        if parts.usage.is_empty() {
            "Not reported for this exact scope.".into()
        } else {
            parts.usage.join("\n")
        },
    ));
    if !parts.context.is_empty() {
        sections.push(("Context events", parts.context.join("\n")));
    }
    let excerpts = parts
        .checks
        .iter()
        .chain(&parts.edits)
        .chain(&parts.errors)
        .enumerate()
        .map(|(index, item)| format!("{}. {} · {}", index + 1, item.label, item.excerpt))
        .collect::<Vec<_>>();
    sections.push((
        "Excerpt index",
        if excerpts.is_empty() {
            "No checks, edits or errors are available to excerpt from this page.".into()
        } else {
            excerpts.join("\n")
        },
    ));
    if !page["next_before"].is_null() {
        sections.push(("Older evidence cursor", encode(&page["next_before"])?));
    }
    sections.push(("Recorded observations", encode(&page["observations"])?));

    let receipt = |format| match format {
        ReviewFormat::Json => encode(&page),
        ReviewFormat::Text => {
            let mut text = format!("{TITLE}\n");
            for (label, content) in &sections {
                text.push_str(&format!("\n{label}\n{content}\n"));
            }
            Ok(text)
        }
        ReviewFormat::Markdown => Ok(markdown_sections(TITLE, &sections)),
        ReviewFormat::Html => Ok(html_sections(TITLE, &sections)),
    };
    if let Some(path) = &output.directory {
        let markdown = receipt(ReviewFormat::Markdown)?;
        let html = receipt(ReviewFormat::Html)?;
        let json = receipt(ReviewFormat::Json)? + "\n";
        export_documents(
            path,
            &[
                ("review.md", &markdown),
                ("review.html", &html),
                ("review.json", &json),
            ],
        )
        .map_err(|error| format!("Cannot export review receipt: {error}"))?;
    }
    if let Some(legacy) = legacy {
        Ok(legacy)
    } else {
        receipt(output.format.unwrap_or(ReviewFormat::Json))
    }
}

fn classify(observations: &Value) -> ReviewParts {
    let mut parts = ReviewParts::default();
    let Some(rows) = observations.as_array() else {
        return parts;
    };
    let mut tokens = TokenTotal::default();
    let mut costs = BTreeMap::<String, (u64, usize)>::new();
    let mut credits = 0_u64;
    for row in rows {
        let Some(kind) = row["observation"]["kind"].as_str() else {
            continue;
        };
        let event = row["event_id"].as_str().unwrap_or("unrecorded event");
        let revision = row["revision"].as_u64().unwrap_or(0);
        let source = format!("{event} revision {revision}");
        match kind {
            "message" => match row["observation"]["role"].as_str() {
                Some("user") if parts.request.is_none() => {
                    parts.request = Some(format!(
                        "Request record {source} · captured text is not retained in this receipt."
                    ));
                }
                Some("assistant") if parts.update.is_none() => {
                    parts.update = Some(format!(
                        "Update record {source} · captured text is not retained in this receipt."
                    ));
                }
                Some(role) => parts.context.push(format!("Message · {role} · {source}")),
                None => parts.context.push(format!("Message · {source}")),
            },
            "native_request" if parts.request.is_none() => {
                parts.request = Some(format!("Recorded native request · {source}"));
            }
            "check" => {
                let result = row["observation"]["result"]
                    .as_str()
                    .unwrap_or("unreported");
                let category = row["observation"]["category"].as_str().unwrap_or("check");
                parts.checks.push(ReviewItem::new(
                    format!("{} · {category}", result.to_uppercase()),
                    format!("check · {category} · {source}"),
                ));
            }
            "file_edit" => {
                let field = |name: &str| row["observation"][name].as_u64().unwrap_or(0);
                let label = format!(
                    "File edit · +{} added · {} updated · {} deleted · {} moved",
                    field("added"),
                    field("updated"),
                    field("deleted"),
                    field("moved"),
                );
                parts.edits.push(ReviewItem::new(
                    label.clone(),
                    format!(
                        "edit · +{}/{}/{}/{} · {source}",
                        field("added"),
                        field("updated"),
                        field("deleted"),
                        field("moved")
                    ),
                ));
            }
            "diagnostic" if row["observation"]["severity"].as_str() == Some("error") => {
                let code = row["observation"]["code"].as_str().unwrap_or("error");
                parts.errors.push(ReviewItem::new(
                    format!("Diagnostic error · {code}"),
                    format!("error · {code} · {source}"),
                ));
            }
            "diagnostic" => parts.context.push(format!(
                "Diagnostic · {} · {} · {source}",
                row["observation"]["severity"]
                    .as_str()
                    .unwrap_or("unreported"),
                row["observation"]["code"].as_str().unwrap_or("code"),
            )),
            "lifecycle" => {
                let state = row["observation"]["state"].as_str().unwrap_or("unreported");
                if state == "failed" {
                    let reason = row["observation"]["reason_code"]
                        .as_str()
                        .filter(|reason| !reason.is_empty())
                        .unwrap_or("reason not recorded");
                    parts.errors.push(ReviewItem::new(
                        format!("Run failed · {reason}"),
                        format!("error · failed · {source}"),
                    ));
                } else {
                    parts
                        .context
                        .push(format!("Lifecycle · {state} · {source}"));
                }
            }
            "usage" => {
                tokens.add(
                    row["observation"]["input_tokens"].as_u64(),
                    row["observation"]["output_tokens"].as_u64(),
                );
            }
            "reported_cost" => {
                let currency = row["observation"]["currency"]
                    .as_str()
                    .unwrap_or("reported");
                let amount = row["observation"]["nanounits"]
                    .as_str()
                    .and_then(|value| value.parse::<u64>().ok())
                    .or_else(|| row["observation"]["nanounits"].as_u64())
                    .unwrap_or(0);
                let total = costs.entry(currency.to_owned()).or_insert((0, 0));
                total.0 = total.0.saturating_add(amount);
                total.1 += 1;
            }
            "credit_usage" => {
                credits = credits.saturating_add(
                    row["observation"]["nanocredits"]
                        .as_u64()
                        .unwrap_or_default(),
                );
            }
            "tool" => parts.context.push(format!(
                "Tool · {} · {} · {source}",
                row["observation"]["category"].as_str().unwrap_or("tool"),
                row["observation"]["result"]
                    .as_str()
                    .unwrap_or("unreported"),
            )),
            "activity" => parts.context.push(format!(
                "Activity · {} · {source}",
                row["observation"]["category"]
                    .as_str()
                    .unwrap_or("activity")
            )),
            "mcp_call" => parts.context.push(format!(
                "MCP · {}/{} · {} · {source}",
                row["observation"]["server"].as_str().unwrap_or("server"),
                row["observation"]["tool"].as_str().unwrap_or("call"),
                row["observation"]["result"]
                    .as_str()
                    .unwrap_or("unreported"),
            )),
            "trace_span" => parts.context.push(format!(
                "Trace span · {} · {} · {source}",
                row["observation"]["span_id"].as_str().unwrap_or("span"),
                row["observation"]["status"].as_str().unwrap_or("status"),
            )),
            "pull_request_raised" => parts.context.push(format!(
                "Work result · PR {}#{} · {source}",
                row["observation"]["repository"]
                    .as_str()
                    .unwrap_or("repository"),
                row["observation"]["number"].as_u64().unwrap_or_default(),
            )),
            "internal_review" => parts.context.push(format!(
                "Internal review · {} · {source}",
                row["observation"]["state"].as_str().unwrap_or("recorded")
            )),
            "session_started" | "session_ended" => {
                parts.context.push(format!("{kind} · {source}"));
            }
            _ => parts.context.push(format!("{kind} · {source}")),
        }
    }
    if let Some(line) = tokens.render() {
        parts.usage.push(line);
    }
    for (currency, (amount, records)) in costs {
        parts.usage.push(format!(
            "Recorded cost · {} {currency} · {records} reported record(s); completeness not claimed",
            crate::review_receipt::decimal(amount, 1_000_000_000)
        ));
    }
    if credits != 0 {
        parts.usage.push(format!("Credits · {credits} nanocredits"));
    }
    parts
}

#[derive(Default)]
struct TokenTotal {
    input: Option<u64>,
    output: Option<u64>,
    records: usize,
}

impl TokenTotal {
    fn add(&mut self, input: Option<u64>, output: Option<u64>) {
        if input.is_none() && output.is_none() {
            return;
        }
        self.records += 1;
        if let Some(input) = input {
            self.input = Some(self.input.unwrap_or_default().saturating_add(input));
        }
        if let Some(output) = output {
            self.output = Some(self.output.unwrap_or_default().saturating_add(output));
        }
    }

    fn render(&self) -> Option<String> {
        if self.records == 0 {
            return None;
        }
        let mut line = String::from("Tokens");
        match (self.input, self.output) {
            (Some(input), Some(output)) => {
                line.push_str(&format!(
                    " · input {input} · output {output} · total {}",
                    input.saturating_add(output)
                ));
            }
            (Some(input), None) => line.push_str(&format!(" · input {input}")),
            (None, Some(output)) => line.push_str(&format!(" · output {output}")),
            (None, None) => {}
        }
        if self.input.is_none() || self.output.is_none() {
            line.push_str(" · partial: one or more records omitted a field");
        }
        Some(line)
    }
}

fn check_counts(checks: &[ReviewItem]) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for check in checks {
        let state = if check.label.starts_with("PASSED") {
            "passed"
        } else if check.label.starts_with("FAILED") {
            "failed"
        } else if check.label.starts_with("UNKNOWN") {
            "unknown"
        } else {
            "unclassified"
        };
        *counts.entry(state).or_insert(0) += 1;
    }
    counts
}

fn summary(parts: &ReviewParts, has_more: bool) -> String {
    let counts = check_counts(&parts.checks);
    let checks = if counts.is_empty() {
        "Checks: none recorded".into()
    } else {
        format!(
            "Checks: {}",
            counts
                .iter()
                .map(|(state, count)| format!("{count} {state}"))
                .collect::<Vec<_>>()
                .join(" · ")
        )
    };
    format!(
        "{checks}\nRecords on page: {} checks · {} edits · {} errors\nEvidence: {} · snapshot coverage\nReview next step: {}",
        parts.checks.len(),
        parts.edits.len(),
        parts.errors.len(),
        if has_more {
            "Partial page; older evidence is available"
        } else {
            "Page reached the retained boundary"
        },
        guidance(&parts.checks, has_more)
    )
}

fn guidance(checks: &[ReviewItem], has_more: bool) -> &'static str {
    if checks.iter().any(|check| check.label.starts_with("FAILED")) {
        return "Inspect the latest comparable failed validation records and their source evidence.";
    }
    if checks
        .iter()
        .any(|check| check.label.starts_with("UNKNOWN"))
    {
        return "Find the missing command results before relying on these checks.";
    }
    if checks.is_empty() {
        return "No validation was captured here; choose and run the checks this change needs.";
    }
    if has_more {
        return "Inspect older evidence before drawing conclusions from this page.";
    }
    "Review the changes and decide which checks the current worktree still needs."
}

fn items_or(items: &[ReviewItem], empty: &str) -> String {
    if items.is_empty() {
        empty.into()
    } else {
        items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Exact fixed-point rendering for reported currency amounts.
pub(super) fn decimal(value: u64, scale: u64) -> String {
    let mut remainder = value % scale;
    let mut text = (value / scale).to_string();
    if remainder != 0 {
        text.push('.');
        for _ in 0..9 {
            remainder *= 10;
            let digit = u8::try_from(remainder / scale).expect("decimal digit");
            text.push(char::from(b'0' + digit));
            remainder %= scale;
            if remainder == 0 {
                break;
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(event: &str, kind: Value) -> Value {
        json!({
            "harness_id": "harness.codex",
            "event_id": event,
            "revision": 2,
            "observed_at_unix_ns": "12",
            "identity": {
                "scope": "session",
                "session_id": "session",
                "worker_id": null,
                "turn_id": "turn",
                "operation_id": null,
            },
            "observation": kind,
        })
    }

    fn page(observations: Vec<Value>, has_more: bool) -> Value {
        json!({
            "observations": observations,
            "has_more": has_more,
            "next_before": has_more.then(|| json!({
                "observed_at_unix_ns": "1",
                "harness_id": "harness.codex",
                "event_id": "older",
                "revision": 0,
                "snapshot_id": "5",
            })),
        })
    }

    fn scope() -> ReviewScope<'static> {
        ReviewScope {
            harness_id: "harness.codex",
            session_id: "session",
            turn_id: Some("turn"),
            trace_id: None,
            worker_id: None,
        }
    }

    #[test]
    fn receipt_names_every_reviewed_work_section_and_its_absence() {
        let page = page(
            vec![
                row("request", json!({"kind": "message", "role": "user"})),
                row("update", json!({"kind": "message", "role": "assistant"})),
                row(
                    "check-pass",
                    json!({"kind": "check", "category": "cargo test", "result": "passed"}),
                ),
                row(
                    "check-fail",
                    json!({"kind": "check", "category": "cargo fmt --check", "result": "failed"}),
                ),
                row(
                    "edit",
                    json!({"kind": "file_edit", "added": 2, "updated": 1, "deleted": 0, "moved": 0}),
                ),
                row(
                    "error",
                    json!({"kind": "diagnostic", "code": "compile", "severity": "error"}),
                ),
                row(
                    "usage",
                    json!({"kind": "usage", "input_tokens": 10, "output_tokens": 5,
                        "cache_read_tokens": null, "cache_write_tokens": null, "total_tokens": 15}),
                ),
                row(
                    "cost",
                    json!({"kind": "reported_cost", "currency": "USD", "nanounits": "2500000000", "span": null}),
                ),
                row(
                    "mcp",
                    json!({"kind": "mcp_call", "server": "server", "tool": "read",
                        "method": "call", "duration_ns": null, "result": "succeeded"}),
                ),
            ],
            true,
        );
        let text = render(
            page,
            scope(),
            9.into(),
            &ReviewOutput {
                format: Some(ReviewFormat::Text),
                directory: None,
            },
        )
        .unwrap();
        for marker in [
            "Latest captured request",
            "Agent-reported update",
            "Recorded checks",
            "PASSED · cargo test",
            "FAILED · cargo fmt --check",
            "Recorded modifying operations",
            "File edit · +2 added · 1 updated",
            "Recorded errors",
            "Diagnostic error · compile",
            "Recorded usage",
            "Tokens · input 10 · output 5 · total 15",
            "Recorded cost · 2.5 USD",
            "Context events",
            "MCP · server/read · succeeded",
            "Excerpt index",
            "check · cargo test · check-pass revision 2",
            "Older evidence cursor",
            "Inspect the latest comparable failed validation records",
        ] {
            assert!(text.contains(marker), "missing {marker}");
        }
        assert!(!text.contains("2 passed"));
    }

    #[test]
    fn receipt_labels_absent_sections_and_marks_partial_token_coverage() {
        let page = page(
            vec![row(
                "usage",
                json!({"kind": "usage", "input_tokens": 7, "output_tokens": null,
                    "cache_read_tokens": null, "cache_write_tokens": null, "total_tokens": null}),
            )],
            false,
        );
        let markdown = render(
            page,
            scope(),
            9.into(),
            &ReviewOutput {
                format: Some(ReviewFormat::Markdown),
                directory: None,
            },
        )
        .unwrap();
        for marker in [
            "# Review receipt",
            "Not available in loaded evidence; the task label is not a reconstructed request.",
            "No recognized validation records in the loaded evidence.",
            "No file operations captured in this evidence page.",
            "None in the loaded evidence. Later recovery is not inferred from completion.",
            "Tokens · input 7 · partial: one or more records omitted a field",
            "No checks, edits or errors are available to excerpt from this page.",
        ] {
            assert!(markdown.contains(marker), "missing {marker}");
        }
        assert!(!markdown.contains("## Older evidence cursor"));
    }

    #[test]
    fn receipt_json_carries_exact_scope_summary_and_excerpts() {
        let page = page(
            vec![row(
                "check",
                json!({"kind": "check", "category": "cargo test", "result": "unknown"}),
            )],
            false,
        );
        let text = render(
            page,
            scope(),
            11.into(),
            &ReviewOutput {
                format: Some(ReviewFormat::Json),
                directory: None,
            },
        )
        .unwrap();
        let receipt: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(receipt["scope"]["session_id"], "session");
        assert_eq!(receipt["scope"]["turn_id"], "turn");
        assert_eq!(receipt["scope"]["worker_scope"], "workerless");
        assert_eq!(receipt["snapshot_id"], "11");
        assert_eq!(receipt["summary"]["checks"]["unknown"], 1);
        assert_eq!(receipt["summary"]["has_more"], false);
        assert_eq!(receipt["excerpts"][0]["kind"], "check");
        assert_eq!(
            receipt["excerpts"][0]["source"],
            "check · cargo test · check revision 2"
        );
        assert_eq!(
            receipt["summary"]["next_step"],
            "Find the missing command results before relying on these checks."
        );
    }

    #[test]
    fn currency_rendering_is_exact_and_does_not_invent_precision() {
        assert_eq!(decimal(0, 1_000_000_000), "0");
        assert_eq!(decimal(2_500_000_000, 1_000_000_000), "2.5");
        assert_eq!(decimal(1, 1_000_000_000), "0.000000001");
    }
}
