//! Read bounded metadata pages and identify recorded failure candidates.

use super::{InvestigationOptions, MAX_PAGE, USAGE, digest, id, required, token};
use agenturm_kernel::{ExtensionId, ObservationPosition, ReadSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_SESSION_ID_BYTES: usize = 512;

fn scope_id(value: &str) -> bool {
    value.starts_with(|ch: char| ch.is_ascii_alphanumeric())
        && value.len() <= MAX_SESSION_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FailureSearch {
    filters: Filters,
    limit: usize,
    cursor: Option<Cursor>,
    snapshot_id: Option<ReadSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct Filters {
    source_id: Option<ExtensionId>,
    session_id: Option<String>,
    kind: Option<Kind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Lifecycle,
    Tool,
    Mcp,
    Check,
    Trace,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u16,
    binding_sha256: String,
    before: ObservationPosition,
}

pub(super) fn parse(args: &[String], broker: bool) -> Result<InvestigationOptions, String> {
    let (pairs, remainder) = args.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(USAGE.into());
    }
    let mut options = BTreeMap::new();
    for [flag, value] in pairs {
        if !flag.starts_with("--")
            || value.is_empty()
            || value.starts_with("--")
            || options.insert(flag.as_str(), value.as_str()).is_some()
        {
            return Err(USAGE.into());
        }
    }
    let db = if broker {
        None
    } else {
        Some(required(&mut options, "--db")?.into())
    };
    let source_id = options
        .remove("--source-id")
        .map(ExtensionId::parse)
        .transpose()
        .map_err(|_| "Invalid investigation source")?;
    let session_id = options.remove("--session-id").map(str::to_owned);
    if session_id
        .as_deref()
        .is_some_and(|session| !scope_id(session))
    {
        return Err("Invalid investigation session".into());
    }
    let kind = match options.remove("--kind") {
        None => None,
        Some("lifecycle") => Some(Kind::Lifecycle),
        Some("tool") => Some(Kind::Tool),
        Some("mcp") => Some(Kind::Mcp),
        Some("check") => Some(Kind::Check),
        Some("trace") => Some(Kind::Trace),
        _ => return Err("Invalid failure candidate kind".into()),
    };
    let limit = options
        .remove("--limit")
        .map(str::parse::<usize>)
        .transpose()
        .map_err(|_| "Investigation limit must be 1..100")?
        .unwrap_or(30);
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err("Investigation limit must be 1..100".into());
    }
    let cursor: Option<Cursor> = options.remove("--cursor").map(token).transpose()?;
    if cursor.as_ref().is_some_and(|cursor| {
        cursor.version != 1
            || !digest(&cursor.binding_sha256)
            || !id(&cursor.before.event_id)
            || ExtensionId::parse(&cursor.before.harness_id).is_err()
            || source_id
                .as_ref()
                .is_some_and(|source| source.as_str() != cursor.before.harness_id)
    }) {
        return Err("Invalid failure investigation cursor".into());
    }
    if !options.is_empty() {
        return Err(USAGE.into());
    }
    Ok(InvestigationOptions {
        db,
        config: None,
        action: super::Action::Failures(FailureSearch {
            filters: Filters {
                source_id,
                session_id,
                kind,
                turn_id: None,
                worker_id: None,
            },
            limit,
            cursor,
            snapshot_id: None,
        }),
    })
}

#[cfg(target_os = "linux")]
impl FailureSearch {
    pub(super) fn for_cockpit(request: &agenturm_ui::InvestigationRead) -> Result<Self, String> {
        let owner = request.owner.as_ref();
        let source_id = owner
            .map(|owner| ExtensionId::parse(&owner.source))
            .transpose()
            .map_err(|_| "Invalid investigation source")?;
        if owner.is_some_and(|owner| {
            !scope_id(&owner.session)
                || owner.turn.as_deref().is_some_and(|turn| !scope_id(turn))
                || owner
                    .worker
                    .as_deref()
                    .is_some_and(|worker| !scope_id(worker) || owner.turn.is_none())
        }) {
            return Err("Invalid investigation session, turn, or worker scope".into());
        }
        let cursor: Option<Cursor> = request.cursor.as_deref().map(token).transpose()?;
        if cursor.as_ref().is_some_and(|cursor| cursor.version != 1) {
            return Err("Invalid failure investigation cursor".into());
        }
        Ok(Self {
            filters: Filters {
                source_id,
                session_id: owner.map(|owner| owner.session.clone()),
                kind: None,
                turn_id: owner.and_then(|owner| owner.turn.clone()),
                worker_id: owner.and_then(|owner| owner.worker.clone()),
            },
            limit: super::COCKPIT_PAGE_LIMIT,
            cursor,
            snapshot_id: request.snapshot_id,
        })
    }
}

#[cfg(target_os = "linux")]
pub(super) struct FailureCandidate<'a> {
    pub row: &'a agenturm_kernel::Observation,
    kind: Kind,
    result: &'a str,
    category: Option<&'a str>,
    pub snapshot_id: ReadSnapshot,
}

#[cfg(target_os = "linux")]
impl FailureCandidate<'_> {
    pub(super) fn label(&self) -> &'static str {
        match (self.kind, self.result) {
            (Kind::Lifecycle, _) => "Lifecycle failed",
            (Kind::Tool, "denied") => "Tool denied",
            (Kind::Tool, _) => "Tool failed",
            (Kind::Mcp, "denied") => "MCP denied",
            (Kind::Mcp, _) => "MCP failed",
            (Kind::Check, _) => "Check failed",
            (Kind::Trace, _) => "Trace error",
        }
    }

    fn reference(&self, history_binding: &str) -> Option<super::SourceReference> {
        if self.kind != Kind::Tool
            || self.category != Some("shell")
            || !self.row.event_id.starts_with("codex:")
            || !self.row.event_id.ends_with(":tool")
        {
            return None;
        }
        let mut reference = super::linux::candidate(
            self.row,
            super::EvidenceKind::Command,
            history_binding,
            self.snapshot_id,
        )?;
        reference.version = 2;
        reference.valid().then_some(reference)
    }

    pub(super) fn encoded_reference(
        &self,
        history_binding: &str,
    ) -> Result<Option<String>, String> {
        self.reference(history_binding)
            .as_ref()
            .map(super::linux::encoded)
            .transpose()
    }
}

#[cfg(target_os = "linux")]
pub(super) struct Coverage {
    pub snapshot_id: ReadSnapshot,
    pub inspected: usize,
    pub matched: usize,
    pub next_cursor: Option<String>,
}

#[cfg(target_os = "linux")]
pub(super) fn scan(
    search: &FailureSearch,
    history: &dyn agenturm_kernel::ObservationStore,
    history_binding: &str,
    mut visit: impl FnMut(FailureCandidate<'_>) -> Result<(), String>,
) -> Result<Coverage, String> {
    use super::linux::{encoded, hash, position};
    use agenturm_kernel::{ObservationKind, ObservationQuery, TraceStatus};

    let binding = hash((
        "agenturm-failure-search/1",
        history_binding,
        &search.filters,
    ))?;
    if let Some(cursor) = &search.cursor
        && cursor.binding_sha256 != hash((&binding, &cursor.before))?
    {
        return Err("Failure cursor belongs to another query, history, or snapshot".into());
    }
    let before = search.cursor.as_ref().map(|cursor| cursor.before.clone());
    let snapshot_id = super::pinned_snapshot(search.snapshot_id, before.as_ref())?;
    let query = ObservationQuery {
        harness_id: search
            .filters
            .source_id
            .as_ref()
            .map(|id| id.as_str().to_owned()),
        session_id: search.filters.session_id.clone(),
        snapshot_id,
        before,
        limit: search.limit,
    };
    let page = if let Some(turn) = &search.filters.turn_id {
        history.query_worker_turn_observations(&query, turn, search.filters.worker_id.as_deref())?
    } else {
        history.query_observations(&query)?
    };
    let mut matched = 0;
    for row in page.observations.iter().rev() {
        let (kind, result, category) = match &row.kind {
            ObservationKind::Lifecycle { state, .. } if state == "failed" => {
                (Kind::Lifecycle, "failed", None)
            }
            ObservationKind::Tool { category, result } if reported_tool_failure(row) => {
                (Kind::Tool, result.as_str(), Some(category.as_str()))
            }
            ObservationKind::McpCall { result, .. }
                if matches!(result.as_str(), "failed" | "denied") =>
            {
                (Kind::Mcp, result.as_str(), None)
            }
            ObservationKind::Check { category, result } if result == "failed" => {
                (Kind::Check, "failed", Some(category.as_str()))
            }
            ObservationKind::TraceSpan(span) if span.status == TraceStatus::Error => {
                (Kind::Trace, "error", None)
            }
            _ => continue,
        };
        if search
            .filters
            .kind
            .is_some_and(|requested| requested != kind)
        {
            continue;
        }
        visit(FailureCandidate {
            row,
            kind,
            result,
            category,
            snapshot_id: page.snapshot_id,
        })?;
        matched += 1;
    }
    let next_cursor = if page.has_more {
        let row = page
            .observations
            .first()
            .ok_or("Failure investigation pagination did not advance")?;
        let before = position(row, page.snapshot_id);
        Some(encoded(&Cursor {
            version: 1,
            binding_sha256: hash((&binding, &before))?,
            before,
        })?)
    } else {
        None
    };
    Ok(Coverage {
        snapshot_id: page.snapshot_id,
        inspected: page.observations.len(),
        matched,
        next_cursor,
    })
}

#[cfg(target_os = "linux")]
pub(super) fn emit_page(
    search: &FailureSearch,
    history: &dyn agenturm_kernel::ObservationStore,
    history_binding: &str,
    output: &mut dyn std::io::Write,
) -> Result<(), String> {
    use super::linux::{emit, position};
    use agenturm_kernel::ObservationKind;
    use serde_json::json;

    let coverage = scan(search, history, history_binding, |item| {
        let row = item.row;
        let mut record = json!({
            "type":"failure_candidate", "kind":item.kind, "reported_result":item.result,
            "category":item.category, "source_id":row.harness_id, "identity":row.identity,
            "position":position(row, item.snapshot_id),
        });
        if let ObservationKind::TraceSpan(span) = &row.kind {
            record["trace_id"] = json!(span.trace_id);
            record["span_id"] = json!(span.span_id);
        }
        if let Some(reference) = item.encoded_reference(history_binding)? {
            record["source_reference"] = json!(reference);
        }
        emit(output, record)
    })?;
    emit(
        output,
        json!({
            "type":"coverage", "format":"agenturm-investigation-failures/1",
            "snapshot_id":coverage.snapshot_id, "observations_examined":coverage.inspected,
            "matched":coverage.matched, "next_cursor":coverage.next_cursor,
            "scope_exhausted":coverage.next_cursor.is_none(),
            "retained_reported_evidence_only":true,
        }),
    )
}

#[cfg(target_os = "linux")]
pub(super) fn reported_tool_failure(row: &agenturm_kernel::Observation) -> bool {
    matches!(&row.kind, agenturm_kernel::ObservationKind::Tool { result, .. } if matches!(result.as_str(), "failed" | "denied"))
}
