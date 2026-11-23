//! Read bounded metadata pages and identify recorded failure candidates.

use super::{InvestigationOptions, MAX_PAGE, USAGE, digest, id, required, token};
use agenturm_kernel::{ExtensionId, ObservationPosition};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_SESSION_ID_BYTES: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FailureSearch {
    filters: Filters,
    limit: usize,
    cursor: Option<Cursor>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct Filters {
    source_id: Option<ExtensionId>,
    session_id: Option<String>,
    kind: Option<Kind>,
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
    if session_id.as_deref().is_some_and(|session| {
        !session.starts_with(|ch: char| ch.is_ascii_alphanumeric())
            || session.len() > MAX_SESSION_ID_BYTES
            || !session
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
    }) {
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
            },
            limit,
            cursor,
        }),
    })
}

#[cfg(target_os = "linux")]
pub(super) fn emit_page(
    search: &FailureSearch,
    history: &dyn agenturm_kernel::ObservationStore,
    history_binding: &str,
    output: &mut dyn std::io::Write,
) -> Result<(), String> {
    use super::{
        EvidenceKind,
        linux::{candidate, emit, encoded, hash, position},
    };
    use agenturm_kernel::{ObservationKind, ObservationQuery, TraceStatus};
    use serde_json::json;

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
    let query = ObservationQuery {
        harness_id: search
            .filters
            .source_id
            .as_ref()
            .map(|id| id.as_str().to_owned()),
        session_id: search.filters.session_id.clone(),
        snapshot_id: before.as_ref().map(|position| position.snapshot_id),
        before,
        limit: search.limit,
    };
    let page = history.query_observations(&query)?;
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
        let mut record = json!({
            "type":"failure_candidate", "kind":kind, "reported_result":result,
            "category":category, "source_id":row.harness_id, "identity":row.identity,
            "position":position(row, page.snapshot_id),
        });
        if let ObservationKind::TraceSpan(span) = &row.kind {
            record["trace_id"] = json!(span.trace_id);
            record["span_id"] = json!(span.span_id);
        }
        if kind == Kind::Tool
            && category == Some("shell")
            && row.event_id.starts_with("codex:")
            && row.event_id.ends_with(":tool")
            && let Some(mut reference) = candidate(
                row,
                EvidenceKind::Command,
                history_binding,
                page.snapshot_id,
            )
        {
            reference.version = 2;
            if reference.valid() {
                record["source_reference"] = json!(encoded(&reference)?);
            }
        }
        emit(output, record)?;
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
    emit(
        output,
        json!({
            "type":"coverage", "format":"agenturm-investigation-failures/1",
            "snapshot_id":page.snapshot_id, "observations_examined":page.observations.len(),
            "matched":matched, "next_cursor":next_cursor, "scope_exhausted":!page.has_more,
            "retained_reported_evidence_only":true,
        }),
    )
}

#[cfg(target_os = "linux")]
pub(super) fn reported_tool_failure(row: &agenturm_kernel::Observation) -> bool {
    matches!(&row.kind, agenturm_kernel::ObservationKind::Tool { result, .. } if matches!(result.as_str(), "failed" | "denied"))
}
