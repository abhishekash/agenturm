//! Metadata-only progress published before, between, or after span exports.

use crate::trace_import::{valid_execution_id, valid_session_id};
use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, TraceProgress,
    is_trace_progress_scope_valid,
};
use serde::Deserialize;

pub(super) const PATH: &str = "/native/v1/progress";
pub(super) const MAX_BYTES: usize = 4_096;
const INVALID: &str = "Invalid trace progress metadata";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    event_id: String,
    trace_id: String,
    observed_at_unix_ns: String,
    stage: String,
    percent_basis_points: Option<u16>,
    session_id: Option<String>,
    target_source_id: Option<String>,
    turn_id: Option<String>,
    worker_id: Option<String>,
}

pub(super) fn decode(
    body: &[u8],
    source_id: &ExtensionId,
    launch_session: Option<&str>,
) -> Result<Observation, &'static str> {
    if body.len() > MAX_BYTES || !source_id.as_str().starts_with("otlp.") {
        return Err(INVALID);
    }
    let record: Record = serde_json::from_slice(body).map_err(|_| INVALID)?;
    let session = match record.session_id.as_deref() {
        Some(session) => session,
        None if record.target_source_id.is_none() => launch_session.ok_or(INVALID)?,
        None => return Err(INVALID),
    };
    if !valid_session_id(session)
        || !record.turn_id.as_deref().is_none_or(valid_execution_id)
        || !record.worker_id.as_deref().is_none_or(valid_execution_id)
        || !record
            .observed_at_unix_ns
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    {
        return Err(INVALID);
    }
    let observed_at_unix_ns = record.observed_at_unix_ns.parse().map_err(|_| INVALID)?;
    let progress = TraceProgress {
        trace_id: record.trace_id.to_ascii_lowercase(),
        target_source_id: record.target_source_id,
        stage: record.stage,
        percent_basis_points: record.percent_basis_points,
    };
    let event_id = format!(
        "{}{}",
        TraceProgress::EVENT_PREFIX,
        record.event_id.to_ascii_lowercase()
    );
    if !progress.is_valid(&event_id, observed_at_unix_ns)
        || !is_trace_progress_scope_valid(
            &progress,
            record.turn_id.as_deref(),
            record.worker_id.as_deref(),
            None,
        )
    {
        return Err(INVALID);
    }
    let mut identity = ObservationIdentity::session(session);
    identity.turn_id = record.turn_id;
    identity.worker_id = record.worker_id;
    Ok(Observation {
        harness_id: source_id.clone(),
        event_id,
        revision: 0,
        identity,
        observed_at_unix_ns,
        kind: ObservationKind::TraceProgress(progress),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_import::commit_with_store;
    use agenturm_kernel::{ObservationQuery, ObservationStore};
    use agenturm_store::Store;
    use serde_json::{Value, json};
    use std::{collections::BTreeSet, path::PathBuf};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("agenturm-progress-{}-{nonce}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn wire() -> Value {
        json!({
            "event_id":"22".repeat(16), "trace_id":"11".repeat(16),
            "observed_at_unix_ns":"10", "stage":"planning",
            "session_id":"session", "target_source_id":"harness.pi",
            "turn_id":"turn", "worker_id":"worker/one",
            "percent_basis_points":0,
        })
    }

    fn read(
        value: &Value,
        source: &ExtensionId,
        launch: Option<&str>,
    ) -> Result<Observation, &'static str> {
        decode(&serde_json::to_vec(value).unwrap(), source, launch)
    }

    #[test]
    fn validates_exact_scope_and_public_metadata() {
        let source = ExtensionId::parse("otlp.fixture").unwrap();
        let selected = read(&wire(), &source, Some("launch-only")).unwrap();
        assert_eq!(selected.harness_id, source);
        assert_eq!(selected.identity.session_id.as_deref(), Some("session"));
        assert_eq!(selected.identity.worker_id.as_deref(), Some("worker/one"));
        assert!(matches!(
            selected.kind,
            ObservationKind::TraceProgress(TraceProgress {
                percent_basis_points: Some(0),
                ..
            })
        ));
        for (field, invalid) in [
            ("session_id", Value::Null),
            ("target_source_id", json!("otlp.other")),
            ("turn_id", Value::Null),
            ("stage", json!("private message")),
            ("stage", json!("planning\n")),
            ("stage", json!("step-ghp_fixture")),
            ("event_id", json!("00".repeat(16))),
            ("observed_at_unix_ns", json!("0")),
            ("percent_basis_points", json!(10001)),
            ("percent_basis_points", json!(0.2)),
            ("message", json!("fixture private body")),
        ] {
            let mut candidate = wire();
            candidate[field] = invalid;
            assert!(
                read(&candidate, &source, Some("launch-only")).is_err(),
                "{field}"
            );
        }
        let mut standalone = wire();
        for key in [
            "session_id",
            "target_source_id",
            "turn_id",
            "worker_id",
            "percent_basis_points",
        ] {
            standalone.as_object_mut().unwrap().remove(key);
        }
        assert!(read(&standalone, &source, None).is_err());
        let fallback = read(&standalone, &source, Some("launch-only")).unwrap();
        assert_eq!(fallback.identity.session_id.as_deref(), Some("launch-only"));
        assert!(matches!(
            fallback.kind,
            ObservationKind::TraceProgress(TraceProgress {
                target_source_id: None,
                percent_basis_points: None,
                ..
            })
        ));
    }

    #[test]
    fn progress_replay_is_immutable_across_other_events_and_retention() {
        let source = ExtensionId::parse("otlp.fixture").unwrap();
        let scratch = Scratch::new();
        let path = scratch.0.join("history.sqlite");
        let mut history = Store::open_collector(&path).unwrap();
        let first = read(&wire(), &source, None).unwrap();
        assert_eq!(
            commit_with_store(&mut history, &source, std::slice::from_ref(&first)),
            Ok(1)
        );
        let mut later = wire();
        later["event_id"] = json!("33".repeat(16));
        later["observed_at_unix_ns"] = json!("20");
        let later = read(&later, &source, None).unwrap();
        assert_eq!(commit_with_store(&mut history, &source, &[later]), Ok(1));
        assert_eq!(
            commit_with_store(&mut history, &source, std::slice::from_ref(&first)),
            Ok(0)
        );
        let mut conflicting = wire();
        conflicting["trace_id"] = json!("44".repeat(16));
        let conflicting = read(&conflicting, &source, None).unwrap();
        assert!(
            commit_with_store(&mut history, &source, std::slice::from_ref(&conflicting)).is_err()
        );

        history.prune_observations_before(11).unwrap();
        assert_eq!(commit_with_store(&mut history, &source, &[first]), Ok(0));
        assert!(commit_with_store(&mut history, &source, &[conflicting]).is_err());
        drop(history);
        let history = Store::open_read_only(&path).unwrap();
        let page =
            ObservationStore::query_observations(&history, &ObservationQuery::latest(10)).unwrap();
        assert_eq!(page.observations.len(), 1);
        assert_eq!(page.observations[0].observed_at_unix_ns, 20);
        assert!(matches!(
            page.observations[0].kind,
            ObservationKind::TraceProgress(_)
        ));
    }

    #[test]
    fn native_linked_progress_requires_retained_anchor_and_authorized_source() {
        let source = ExtensionId::parse("otlp.fixture").unwrap();
        let native = ExtensionId::parse("harness.pi").unwrap();
        let mut history = Store::open(":memory:").unwrap();
        let progress = read(&wire(), &source, None).unwrap();
        commit_with_store(&mut history, &source, std::slice::from_ref(&progress)).unwrap();
        let query = ObservationQuery {
            harness_id: Some(native.as_str().into()),
            session_id: Some("session".into()),
            ..ObservationQuery::latest(10)
        };
        let query_turn = |history: &Store, turn, worker, allowed| {
            history
                .query_linked_turn_traces_from_sources(&query, turn, worker, allowed)
                .unwrap()
                .observations
        };
        let allowed = BTreeSet::from([native.clone(), source]);
        assert!(query_turn(&history, "turn", Some("worker/one"), &allowed).is_empty());
        let anchor = Observation {
            harness_id: native.clone(),
            event_id: "native-message".into(),
            revision: 0,
            identity: progress.identity.clone(),
            observed_at_unix_ns: 9,
            kind: ObservationKind::Message {
                role: "user".into(),
            },
        };
        ObservationStore::commit(
            &mut history,
            native.as_str(),
            None,
            "native-cursor",
            &[anchor],
        )
        .unwrap();
        assert_eq!(
            query_turn(&history, "turn", Some("worker/one"), &allowed),
            [progress]
        );
        assert!(query_turn(&history, "turn", Some("worker/two"), &allowed).is_empty());
        let native_only = BTreeSet::from([native]);
        assert!(query_turn(&history, "turn", Some("worker/one"), &native_only).is_empty());
    }
}
