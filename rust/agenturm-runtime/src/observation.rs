//! Host validation and projection for the shared observation format.

use crate::{
    DATA_FRAME_LIMIT, MAX_POLL_RECORDS, PROTOCOL_VERSION, Result, RuntimeError, SourceBatchDecoder,
    validate_cursor,
};
use agenturm_kernel::{
    ObservationIdentity, ObservationKind, ObservationScope, PreparedSourceBatch, RawObservation,
    is_anchored_pull_request, is_anchored_worker_launch, is_anchored_worker_spawn,
    is_file_edit_operation, is_lifecycle_state, is_mcp_operation, is_safe_diagnostic_code,
    is_session_relationship, is_valid_mcp_metadata,
};
use agenturm_protocol::observation::{ObservationBatch, ObservationFact, ObservationRecord};

/// Host decoder for `agenturm-observations/1`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ObservationV1Decoder;

impl SourceBatchDecoder for ObservationV1Decoder {
    fn decode_batch(&self, encoded: &str, expected_source_id: &str) -> Result<PreparedSourceBatch> {
        parse_source_batch(encoded, expected_source_id)
    }
}

pub(super) fn parse_source_batch(
    encoded: &str,
    expected_source_id: &str,
) -> Result<PreparedSourceBatch> {
    if encoded.len() > DATA_FRAME_LIMIT {
        return Err(RuntimeError::new("source batch exceeds byte limit"));
    }
    let batch: ObservationBatch = serde_json::from_str(encoded)
        .map_err(|_| RuntimeError::new("source batch rejected: invalid observation schema"))?;
    if batch.protocol() != PROTOCOL_VERSION
        || batch.source_id != expected_source_id
        || batch.records.len() > usize::from(MAX_POLL_RECORDS)
    {
        return Err(RuntimeError::new("source batch envelope mismatch"));
    }
    validate_cursor(&batch.cursor)?;
    let mut observations = Vec::with_capacity(batch.records.len());
    let mut skipped_unprojected_facts = 0;
    for record in batch.records {
        match project_record(record, expected_source_id)? {
            Some(observation) => observations.push(observation),
            None => skipped_unprojected_facts += 1,
        }
    }
    Ok(PreparedSourceBatch {
        next_cursor: batch.cursor,
        observations,
        skipped_unprojected_facts,
    })
}

fn project_record(record: ObservationRecord, source_id: &str) -> Result<Option<RawObservation>> {
    if !valid_source_identifier(&record.event_id, 256) {
        return Err(RuntimeError::new("source event identity is invalid"));
    }
    let observed_at_unix_ns = parse_decimal_timestamp(&record.time_ns)?;
    let identity = match record.identity {
        Some(identity) => {
            let parts = [
                Some(identity.session_id.as_str()),
                identity.worker_id.as_deref(),
                identity.turn_id.as_deref(),
                identity.operation_id.as_deref(),
            ];
            if parts
                .into_iter()
                .flatten()
                .any(|value| !valid_source_identifier(value, 512))
            {
                return Err(RuntimeError::new("source native identity is invalid"));
            }
            ObservationIdentity {
                scope: ObservationScope::Session,
                session_id: Some(identity.session_id),
                worker_id: identity.worker_id,
                turn_id: identity.turn_id,
                operation_id: identity.operation_id,
            }
        }
        None if matches!(
            record.fact,
            ObservationFact::Diagnostic(_) | ObservationFact::InternalReview(_)
        ) =>
        {
            ObservationIdentity::source()
        }
        None => return Err(RuntimeError::new("source fact identity is missing")),
    };
    let kind = match record.fact {
        ObservationFact::Usage(counters) => {
            let fields = [
                counters.input_tokens,
                counters.output_tokens,
                counters.cache_read_tokens,
                counters.cache_write_tokens,
                counters.total_tokens,
            ];
            if fields.iter().all(Option::is_none) {
                return Err(RuntimeError::new("usage record has no counters"));
            }
            ObservationKind::Usage {
                input_tokens: counters.input_tokens,
                output_tokens: counters.output_tokens,
                cache_read_tokens: counters.cache_read_tokens,
                cache_write_tokens: counters.cache_write_tokens,
                total_tokens: counters.total_tokens,
            }
        }
        ObservationFact::SpanTokenUsage(usage) => {
            if !source_id.starts_with("otlp.")
                || !agenturm_kernel::is_span_usage_anchor_valid(
                    &usage,
                    &record.event_id,
                    identity.turn_id.as_deref(),
                    identity.worker_id.as_deref(),
                    identity.operation_id.as_deref(),
                )
            {
                return Err(RuntimeError::new(
                    "source span usage metadata or event identity is invalid",
                ));
            }
            ObservationKind::SpanTokenUsage(usage)
        }
        ObservationFact::CreditUsage(usage) => {
            if usage.nanocredits > i64::MAX as u64 {
                return Err(RuntimeError::new(
                    "source credit usage exceeds supported range",
                ));
            }
            ObservationKind::CreditUsage {
                nanocredits: usage.nanocredits,
            }
        }
        ObservationFact::ReportedCost(cost) => {
            if !cost.is_valid()
                || !cost.matches_evidence(&record.event_id, identity.operation_id.as_deref())
                || (cost.span.is_some() && !source_id.starts_with("otlp."))
            {
                return Err(RuntimeError::new(
                    "source reported cost metadata or event identity is invalid",
                ));
            }
            ObservationKind::ReportedCost(cost)
        }
        ObservationFact::Tool(tool) => {
            if !matches!(tool.category.as_str(), "shell" | "file" | "mcp" | "other")
                || !matches!(
                    tool.result.as_str(),
                    "started" | "succeeded" | "failed" | "denied"
                )
            {
                return Err(RuntimeError::new("source tool payload is invalid"));
            }
            ObservationKind::Tool {
                category: tool.category,
                result: tool.result,
            }
        }
        ObservationFact::FileEdit(edit) => {
            if !edit.is_valid() || !is_file_edit_operation(&identity) {
                return Err(RuntimeError::new(
                    "source file edit or operation identity is invalid",
                ));
            }
            ObservationKind::FileEdit(edit)
        }
        ObservationFact::McpCall(call) => {
            if !is_mcp_operation(&identity)
                || !is_valid_mcp_metadata(
                    call.server.as_deref(),
                    call.tool.as_deref(),
                    call.duration_ns,
                    &call.result,
                )
            {
                return Err(RuntimeError::new(
                    "source MCP call metadata or operation identity is invalid",
                ));
            }
            ObservationKind::McpCall {
                server: call.server,
                tool: call.tool,
                method: call.method,
                duration_ns: call.duration_ns,
                result: call.result,
            }
        }
        ObservationFact::TraceSpan(span) => {
            if !span.is_valid(observed_at_unix_ns)
                || !agenturm_kernel::is_trace_source_link_valid(source_id, &span)
                || !agenturm_kernel::is_trace_turn_link_valid(
                    &span,
                    &record.event_id,
                    identity.turn_id.as_deref(),
                    identity.worker_id.as_deref(),
                    identity.operation_id.as_deref(),
                )
            {
                return Err(RuntimeError::new(
                    "source trace identity, timing, or source link is invalid",
                ));
            }
            ObservationKind::TraceSpan(span)
        }
        ObservationFact::InternalReview(review) => {
            if identity.scope != ObservationScope::Source
                || !agenturm_protocol::native_review::is_codex_review_source(source_id)
                || !review.is_valid()
            {
                return Err(RuntimeError::new(
                    "source internal review metadata is invalid",
                ));
            }
            ObservationKind::InternalReview(review)
        }
        ObservationFact::NativeRequest(request) => {
            if identity.scope != ObservationScope::Session
                || identity.turn_id.is_none()
                || !request.is_valid()
            {
                return Err(RuntimeError::new(
                    "source native request metadata is invalid",
                ));
            }
            ObservationKind::NativeRequest(request)
        }
        ObservationFact::SessionDiscovered(_) => {
            if record.revision != 0
                || identity.scope != ObservationScope::Session
                || identity.turn_id.is_some()
                || identity.operation_id.is_some()
            {
                return Err(RuntimeError::new(
                    "source session discovery identity is invalid",
                ));
            }
            ObservationKind::Activity {
                category: "session_discovered".into(),
            }
        }
        ObservationFact::Lifecycle(lifecycle) => {
            if !is_lifecycle_state(&lifecycle.state)
                || lifecycle
                    .reason_code
                    .as_deref()
                    .is_some_and(|reason| !valid_reason_code(reason))
            {
                return Err(RuntimeError::new("source lifecycle payload is invalid"));
            }
            ObservationKind::Lifecycle {
                state: lifecycle.state,
                reason_code: lifecycle.reason_code,
            }
        }
        ObservationFact::Check(check) => {
            if !matches!(check.category.as_str(), "test" | "lint" | "build")
                || !matches!(check.result.as_str(), "passed" | "failed" | "unknown")
            {
                return Err(RuntimeError::new("source check payload is invalid"));
            }
            ObservationKind::Check {
                category: check.category,
                result: check.result,
            }
        }
        ObservationFact::PullRequestRaised(result) => {
            if !is_anchored_pull_request(&result.repository, result.number, &identity) {
                return Err(RuntimeError::new(
                    "source pull request reference or identity is invalid",
                ));
            }
            ObservationKind::PullRequestRaised {
                repository: result.repository,
                number: result.number,
            }
        }
        ObservationFact::Relationship(relationship) => {
            if !is_session_relationship(&relationship.parent_session_id, &identity) {
                return Err(RuntimeError::new(
                    "source relationship parent or identity is invalid",
                ));
            }
            ObservationKind::Relationship {
                relationship_kind: relationship.relationship_kind,
                parent_session_id: relationship.parent_session_id,
            }
        }
        ObservationFact::WorkerSpawn(spawn) => {
            if !is_anchored_worker_spawn(&spawn, &identity) {
                return Err(RuntimeError::new("source worker spawn identity is invalid"));
            }
            ObservationKind::WorkerSpawn(spawn)
        }
        ObservationFact::WorkerLaunch(launch) => {
            if !is_anchored_worker_launch(&launch, &identity) {
                return Err(RuntimeError::new(
                    "source worker launch identity is invalid",
                ));
            }
            ObservationKind::WorkerLaunch(launch)
        }
        ObservationFact::Message(message) => {
            if !matches!(message.role.as_str(), "user" | "assistant") {
                return Err(RuntimeError::new("source message payload is invalid"));
            }
            ObservationKind::Message { role: message.role }
        }
        ObservationFact::Diagnostic(diagnostic) => {
            if diagnostic.code.is_empty()
                || diagnostic.code.len() > 128
                || !matches!(diagnostic.severity.as_str(), "info" | "warning" | "error")
            {
                return Err(RuntimeError::new("source diagnostic payload is invalid"));
            }
            if !is_safe_diagnostic_code(&diagnostic.code) {
                return Ok(None);
            }
            ObservationKind::Diagnostic {
                code: diagnostic.code,
                severity: diagnostic.severity,
            }
        }
    };
    Ok(Some(RawObservation {
        event_id: record.event_id,
        revision: record.revision,
        identity,
        observed_at_unix_ns,
        kind,
    }))
}

fn valid_source_identifier(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

fn valid_reason_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

pub(super) fn parse_decimal_timestamp(value: &str) -> Result<u64> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(RuntimeError::new(
            "source timestamp is not canonical decimal",
        ));
    }
    let timestamp = value
        .parse::<u64>()
        .map_err(|_| RuntimeError::new("source timestamp is outside range"))?;
    if timestamp > i64::MAX as u64 {
        return Err(RuntimeError::new(
            "source timestamp exceeds supported range",
        ));
    }
    Ok(timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{
        ExtensionId, ExtensionStage, Kernel, Observation, ObservationStore, SourceAdapter,
    };
    use agenturm_store::{SessionQuery, Store};
    use serde_json::json;

    #[test]
    fn internal_review_requires_reserved_source_and_source_scope() {
        let source = format!("review.codex.{}", "a".repeat(64));
        let mut batch = json!({"type":"batch","protocol":1,"source_id":source,"cursor":"review",
            "records":[{"event_id":"review","revision":0,"kind":"internal_review","time_ns":"1",
                "payload":{"session_id":"review-thread","parent_session_id":"parent-thread",
                    "turn_id":"review-turn","state":"interrupted"}}]});
        let decoded = parse_source_batch(&batch.to_string(), &source).unwrap();
        assert_eq!(
            decoded.observations[0].identity,
            ObservationIdentity::source()
        );
        assert!(matches!(
            decoded.observations[0].kind,
            ObservationKind::InternalReview(_)
        ));
        batch["source_id"] = json!("harness.codex");
        assert!(parse_source_batch(&batch.to_string(), "harness.codex").is_err());
        batch["source_id"] = json!("review.codex.short");
        assert!(parse_source_batch(&batch.to_string(), "review.codex.short").is_err());
        batch["source_id"] = json!(source);
        batch["records"][0]["identity"] = json!({"session_id":"review-thread"});
        assert!(parse_source_batch(&batch.to_string(), &source).is_err());
    }

    #[test]
    fn native_request_rejects_missing_exact_scope_and_untyped_payload() {
        let mut batch = json!({"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"request",
            "records":[{"event_id":"approval-request","revision":0,"kind":"native_request","time_ns":"1",
                "identity":{"session_id":"session","turn_id":"turn","worker_id":"worker"},
                "payload":{"request_id":"b".repeat(64),"request_kind":"approval","state":"requested"}}]});
        let decoded = parse_source_batch(&batch.to_string(), "harness.codex").unwrap();
        let event = &decoded.observations[0];
        assert!(matches!(event.kind, ObservationKind::NativeRequest(_)));
        assert_eq!(event.identity.turn_id.as_deref(), Some("turn"));
        assert_eq!(event.identity.worker_id.as_deref(), Some("worker"));
        let valid = batch["records"][0].clone();
        for invalid in [
            ("identity", json!(null)),
            (
                "identity",
                json!({"session_id":"session","worker_id":"worker"}),
            ),
            (
                "payload",
                json!({"request_id":"raw identity","request_kind":"approval","state":"requested"}),
            ),
            (
                "payload",
                json!({"request_id":"b".repeat(64),"request_kind":"approval","state":"approved"}),
            ),
            (
                "payload",
                json!({"request_id":"b".repeat(64),"request_kind":"approval","state":"requested","body":"private sentinel"}),
            ),
        ] {
            batch["records"][0] = valid.clone();
            batch["records"][0][invalid.0] = invalid.1;
            let error = parse_source_batch(&batch.to_string(), "harness.codex").unwrap_err();
            assert!(!error.to_string().contains("private sentinel"));
        }
    }

    #[test]
    fn worker_spawn_requires_exact_parent_scope_and_closed_child_id() {
        let source = "harness.claude.fixture";
        let mut batch = json!({"type":"batch","protocol":1,"source_id":source,"cursor":"verified-operation",
            "records":[{"event_id":"spawn-event","revision":7,"kind":"worker_spawn","time_ns":"100",
                "identity":{"session_id":"session","turn_id":"turn","worker_id":"parent","operation_id":"launch"},
                "payload":{"child_worker_id":"child"}}]});
        let prepared = parse_source_batch(&batch.to_string(), source).unwrap();
        let row = &prepared.observations[0];
        assert_eq!(row.identity.operation_id.as_deref(), Some("launch"));
        assert!(matches!(&row.kind,
            ObservationKind::WorkerSpawn(spawn) if spawn.child_worker_id == "child"));
        let valid = batch["records"][0].clone();
        for (field, invalid) in [
            ("identity", json!(null)),
            (
                "identity",
                json!({"session_id":"session","operation_id":"launch"}),
            ),
            ("identity", json!({"session_id":"session","turn_id":"turn"})),
            ("payload", json!({"child_worker_id":"parent"})),
            ("payload", json!({"child_worker_id":"invalid ID"})),
            (
                "payload",
                json!({"child_worker_id":"child","body":"private fixture"}),
            ),
            ("payload", json!(["child"])),
        ] {
            batch["records"][0] = valid.clone();
            batch["records"][0][field] = invalid;
            let error = parse_source_batch(&batch.to_string(), source).unwrap_err();
            assert!(!error.to_string().contains("private fixture"));
        }
    }

    #[test]
    fn worker_launch_requires_exact_parent_scope_and_closed_child_id() {
        let source = "harness.claude.fixture";
        let mut batch = json!({"type":"batch","protocol":1,"source_id":source,"cursor":"verified-launch",
            "records":[{"event_id":"launch-event","revision":7,"kind":"worker_launch","time_ns":"100",
                "identity":{"session_id":"session","turn_id":"turn","worker_id":"parent","operation_id":"launch"},
                "payload":{"child_worker_id":"child"}}]});
        let prepared = parse_source_batch(&batch.to_string(), source).unwrap();
        let row = &prepared.observations[0];
        assert_eq!(row.identity.operation_id.as_deref(), Some("launch"));
        assert!(matches!(&row.kind,
            ObservationKind::WorkerLaunch(launch) if launch.child_worker_id == "child"));
        let valid = batch["records"][0].clone();
        for (field, invalid) in [
            ("identity", json!(null)),
            (
                "identity",
                json!({"session_id":"session","operation_id":"launch"}),
            ),
            ("identity", json!({"session_id":"session","turn_id":"turn"})),
            ("payload", json!({"child_worker_id":"parent"})),
            ("payload", json!({"child_worker_id":"invalid ID"})),
            (
                "payload",
                json!({"child_worker_id":"child","body":"private fixture"}),
            ),
            ("payload", json!(["child"])),
        ] {
            batch["records"][0] = valid.clone();
            batch["records"][0][field] = invalid;
            let error = parse_source_batch(&batch.to_string(), source).unwrap_err();
            assert!(!error.to_string().contains("private fixture"));
        }
    }

    #[test]
    fn header_discovery_creates_unknown_roster_evidence_without_overriding_lifecycle() {
        let source = "harness.codex";
        let mut batch = json!({"type":"batch","protocol":1,"source_id":source,"cursor":"verified-header",
            "records":[{"event_id":"header","revision":0,"kind":"session_discovered","time_ns":"100",
                "identity":{"session_id":"thread-1","worker_id":"thread-1"}, "payload":{}}]});
        let prepared = parse_source_batch(&batch.to_string(), source).unwrap();
        let original = batch["records"][0].clone();
        for (field, invalid) in [
            ("revision", json!(1)),
            ("identity", json!(null)),
            (
                "identity",
                json!({"session_id":"thread-1","turn_id":"unproved"}),
            ),
            (
                "identity",
                json!({"session_id":"thread-1","operation_id":"unproved"}),
            ),
            (
                "payload",
                json!({"state":"running","body":"private fixture"}),
            ),
            ("payload", json!([])),
        ] {
            batch["records"][0] = original.clone();
            batch["records"][0][field] = invalid;
            let message = parse_source_batch(&batch.to_string(), source)
                .unwrap_err()
                .to_string();
            assert!(!message.contains("private fixture"));
        }
        let raw = prepared.observations.into_iter().next().unwrap();
        let row = Observation {
            harness_id: ExtensionId::parse(source).unwrap(),
            event_id: raw.event_id,
            revision: raw.revision,
            identity: raw.identity,
            observed_at_unix_ns: raw.observed_at_unix_ns,
            kind: raw.kind,
        };
        assert_eq!(
            row.kind,
            ObservationKind::Activity {
                category: "session_discovered".into()
            }
        );
        let mut store = Store::open(":memory:").unwrap();
        store
            .commit(
                source,
                None,
                &prepared.next_cursor,
                std::slice::from_ref(&row),
            )
            .unwrap();
        let query = SessionQuery::latest(1);
        let page = store.query_sessions(&query).unwrap();
        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].session_id, "thread-1");
        assert_eq!(page.sessions[0].observed_at_unix_ns, 100);
        assert!(page.sessions[0].lifecycle.is_none());

        let mut started = row.clone();
        started.event_id = "explicit-turn-start".into();
        started.identity.turn_id = Some("turn-1".into());
        // Source clocks can record the real turn before the metadata header.
        started.observed_at_unix_ns = 99;
        started.kind = ObservationKind::Lifecycle {
            state: "running".into(),
            reason_code: None,
        };
        store
            .commit(
                source,
                Some(&prepared.next_cursor),
                "verified-turn",
                &[started],
            )
            .unwrap();
        let page = store.query_sessions(&query).unwrap();
        let lifecycle = page.sessions[0].lifecycle.as_ref().unwrap();
        assert_eq!(lifecycle.state, "running");
        assert_eq!(lifecycle.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(lifecycle.observed_at_unix_ns, 99);
    }

    #[test]
    fn reserved_review_cannot_admit_session_work_at_either_boundary() {
        struct FixedReviewSource(String, RawObservation);
        impl SourceAdapter for FixedReviewSource {
            fn source_id(&self) -> &str {
                &self.0
            }
            fn poll(&mut self, _: &str) -> std::result::Result<PreparedSourceBatch, String> {
                Ok(PreparedSourceBatch {
                    next_cursor: "next".into(),
                    observations: vec![self.1.clone()],
                    skipped_unprojected_facts: 0,
                })
            }
            fn ack(&mut self, _: &str) -> std::result::Result<(), String> {
                Ok(())
            }
            fn reset(&mut self) -> std::result::Result<(), String> {
                Ok(())
            }
        }
        let source = format!("review.codex.{}", "a".repeat(64));
        let id = ExtensionId::parse(&source).unwrap();
        let mut store = Store::open(":memory:").unwrap();
        let invalid = RawObservation {
            event_id: "spoofed-session".into(),
            revision: 0,
            identity: ObservationIdentity::session("review-thread"),
            observed_at_unix_ns: 10,
            kind: ObservationKind::SessionStarted,
        };
        let mut kernel = Kernel::new();
        kernel
            .register_source_adapter(
                id.clone(),
                FixedReviewSource(source.clone(), invalid.clone()),
            )
            .unwrap();
        let outcome = kernel.refresh_with_store(&mut store);
        assert!(outcome.admissions.is_empty());
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(
            outcome.failures[0].stage,
            ExtensionStage::AdmissionValidation
        );
        let forged = Observation {
            harness_id: id.clone(),
            event_id: invalid.event_id,
            revision: invalid.revision,
            identity: invalid.identity,
            observed_at_unix_ns: invalid.observed_at_unix_ns,
            kind: invalid.kind,
        };
        assert!(ObservationStore::commit(&mut store, &source, None, "next", &[forged]).is_err());
        assert!(store.source_cursor(&source).unwrap().is_none());

        let batch = json!({"type":"batch","protocol":1,"source_id":source,"cursor":"next",
            "records":[{"event_id":"review","revision":0,"kind":"internal_review","time_ns":"11",
                "payload":{"session_id":"review-thread","turn_id":"turn","state":"started"}}]});
        let allowed = parse_source_batch(&batch.to_string(), &source)
            .unwrap()
            .observations
            .pop()
            .unwrap();
        kernel.unregister_source_adapter(&id);
        kernel
            .register_source_adapter(id.clone(), FixedReviewSource(source, allowed))
            .unwrap();
        let retained = kernel.refresh_with_store(&mut store);
        assert!(retained.failures.is_empty());
        assert_eq!(retained.admissions[&id].inserted, 1);
        assert!(matches!(
            retained.observations[0].kind,
            ObservationKind::InternalReview(_)
        ));
        assert!(
            store
                .query_sessions(&SessionQuery::latest(10))
                .unwrap()
                .sessions
                .is_empty()
        );
    }

    #[test]
    fn native_mcp_details_require_an_operation_and_bounded_metadata() {
        let mut batch = json!({
            "type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next",
            "records":[{
                "event_id":"mcp","revision":0,"kind":"mcp_call","time_ns":"1",
                "identity":{"session_id":"session","operation_id":"operation"},
                "payload":{
                    "server":"github","tool":"search_code","method":"tools/call",
                    "duration_ns":0,"result":"succeeded"
                }
            }]
        });
        let good = batch["records"][0].clone();
        let decoded = parse_source_batch(&batch.to_string(), "harness.codex").unwrap();
        assert_eq!(
            decoded.observations[0].kind,
            ObservationKind::McpCall {
                server: Some("github".into()),
                tool: Some("search_code".into()),
                method: agenturm_kernel::McpMethod::ToolsCall,
                duration_ns: Some(0),
                result: "succeeded".into(),
            },
        );

        for (pointer, invalid) in [
            ("/identity/operation_id", json!(null)),
            ("/payload/server", json!("../private-source")),
            ("/payload/tool", json!("raw call argument")),
            ("/payload/duration_ns", json!(i64::MAX as u64 + 1)),
            ("/payload/method", json!("other")),
        ] {
            batch["records"][0] = good.clone();
            *batch["records"][0].pointer_mut(pointer).unwrap() = invalid;
            assert!(parse_source_batch(&batch.to_string(), "harness.codex").is_err());
        }
        batch["records"][0] = good;
        batch["records"][0]["payload"]["arguments"] = json!({"secret": "private"});
        let error = parse_source_batch(&batch.to_string(), "harness.codex").unwrap_err();
        assert!(!error.to_string().contains("private"));
    }

    #[test]
    fn credit_usage_keeps_zero_and_rejects_invalid_units_or_amounts() {
        let mut batch = json!({
            "type":"batch","protocol":1,"source_id":"harness.kiro","cursor":"next",
            "records":[{
                "event_id":"credit","revision":0,"kind":"credit_usage","time_ns":"1",
                "identity":{"session_id":"session"},"payload":{"nanocredits":0},
            }]
        });
        assert_eq!(
            parse_source_batch(&batch.to_string(), "harness.kiro")
                .unwrap()
                .observations[0]
                .kind,
            ObservationKind::CreditUsage { nanocredits: 0 },
        );
        for payload in [
            json!({}),
            json!({"nanocredits":-1}),
            json!({"nanocredits":i64::MAX as u64 + 1}),
            json!({"nanocredits":0.5}),
            json!({"nanocredits":0,"input_tokens":2}),
        ] {
            batch["records"][0]["payload"] = payload;
            assert!(parse_source_batch(&batch.to_string(), "harness.kiro").is_err());
        }
    }

    #[test]
    fn relationship_requires_a_distinct_parent_and_a_native_session_anchor() {
        let mut batch = json!({
            "type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next",
            "records":[{
                "event_id":"e1","revision":0,"kind":"relationship","time_ns":"1",
                "identity":{"session_id":"child"},
                "payload":{"relationship_kind":"delegated","parent_session_id":"parent"}
            }]
        });
        let decoded = parse_source_batch(&batch.to_string(), "harness.codex").unwrap();
        assert_eq!(
            decoded.observations[0].kind,
            ObservationKind::Relationship {
                relationship_kind: agenturm_kernel::RelationshipKind::Delegated,
                parent_session_id: "parent".into(),
            },
        );

        for (identity, payload) in [
            (json!(null), batch["records"][0]["payload"].clone()),
            (
                json!({"session_id":"child","turn_id":"turn"}),
                batch["records"][0]["payload"].clone(),
            ),
            (
                json!({"session_id":"parent"}),
                batch["records"][0]["payload"].clone(),
            ),
            (
                json!({"session_id":"child"}),
                json!({"relationship_kind":"other","parent_session_id":"parent"}),
            ),
            (
                json!({"session_id":"child"}),
                json!({"relationship_kind":"delegated","parent_session_id":"../private"}),
            ),
        ] {
            batch["records"][0]["identity"] = identity;
            batch["records"][0]["payload"] = payload;
            assert!(parse_source_batch(&batch.to_string(), "harness.codex").is_err());
        }
    }
}
