//! A frozen Review selects a retained occurrence. It never supplies prompt text.

use super::{ExactSource, ReviewPin, TaskResult, read_input};
use agenturm_kernel::{ExtensionId, ObservationIdentity, ObservationScope, ReadSnapshot};
use agenturm_store::WorkspaceAnchor;
use serde::Deserialize;
use serde_json::value::RawValue;
use std::path::PathBuf;

const MAX_REVIEW_BYTES: u64 = 3 * 1024 * 1024;
const FORMAT: &str = "agenturm-review-receipt/1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewFile {
    pub file: PathBuf,
    pub event_id: Option<String>,
    pub revision: Option<u32>,
}

#[derive(Deserialize)]
struct Receipt<'a> {
    format: String,
    history_identity: Option<String>,
    snapshot_id: ReadSnapshot,
    scope: Scope,
    #[serde(borrow)]
    observations: Vec<Occurrence<'a>>,
}

#[derive(Deserialize)]
struct Scope {
    source_id: ExtensionId,
    harness_id: ExtensionId,
    session_id: String,
    turn_id: Option<String>,
    trace_id: Option<String>,
    worker_id: Option<String>,
    worker_scope: String,
}

#[derive(Deserialize)]
struct Occurrence<'a> {
    harness_id: ExtensionId,
    event_id: String,
    revision: u32,
    observed_at_unix_ns: String,
    identity: ObservationIdentity,
    #[serde(borrow)]
    observation: &'a RawValue,
}

#[derive(Deserialize)]
struct ObservationKind {
    kind: String,
    role: Option<String>,
}

impl ReviewFile {
    pub(super) fn select(&self) -> TaskResult<ExactSource> {
        let input = read_input(&self.file, MAX_REVIEW_BYTES)?;
        self.select_receipt(&input)
    }

    fn select_receipt(&self, input: &str) -> TaskResult<ExactSource> {
        let invalid = "Choose a Rust Review JSON receipt exported with --format json";
        let receipt: Receipt<'_> = serde_json::from_str(input).map_err(|_| invalid)?;
        if receipt.format != FORMAT {
            return Err(invalid.into());
        }
        let history_identity = receipt.history_identity.filter(|value| {
            value.len() == 32 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        }).ok_or("The Review receipt has no history identity. Export a new exact Rust Review receipt.")?;
        let scope = receipt.scope;
        let turn = scope.turn_id.filter(|turn| !turn.is_empty()).ok_or(
            "A source-backed draft requires a native turn Review with a recorded user prompt",
        )?;
        if scope.source_id != scope.harness_id
            || scope.source_id.as_str().starts_with("otlp.")
            || scope.session_id.is_empty()
            || scope.trace_id.is_some()
            || scope.worker_scope
                != if scope.worker_id.is_some() {
                    "exact"
                } else {
                    "workerless"
                }
        {
            return Err(
                "The Review receipt does not have one exact native source, turn, and worker".into(),
            );
        }
        let mut selected = None;
        for row in receipt.observations {
            if row.harness_id != scope.source_id
                || row.identity.scope != ObservationScope::Session
                || row.identity.session_id.as_deref() != Some(scope.session_id.as_str())
                || row.identity.turn_id.as_deref() != Some(turn.as_str())
                || row.identity.worker_id != scope.worker_id
            {
                return Err("A Review observation is outside its declared native scope".into());
            }
            if self
                .event_id
                .as_ref()
                .is_some_and(|expected| expected != &row.event_id)
                || self
                    .revision
                    .is_some_and(|expected| expected != row.revision)
            {
                continue;
            }
            let observation: ObservationKind =
                serde_json::from_str(row.observation.get()).map_err(|_| invalid)?;
            if observation.kind != "message" || observation.role.as_deref() != Some("user") {
                continue;
            }
            if selected.is_some() {
                return Err("The Review has multiple matching user prompts. Choose one exact --event-id and --revision.".into());
            }
            selected = Some(row);
        }
        let selected = selected.ok_or("The Review has no matching retained user prompt. Export a native turn page containing the exact prompt.")?;
        let timestamp = selected
            .observed_at_unix_ns
            .parse::<u64>()
            .ok()
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or("The Review prompt timestamp is invalid")?;
        if selected.event_id.is_empty()
            || selected.event_id.len() > 256
            || selected.identity.operation_id.is_some()
        {
            return Err("The selected Review prompt identity is invalid".into());
        }
        Ok(ExactSource {
            anchor: WorkspaceAnchor {
                harness_id: scope.source_id,
                session_id: scope.session_id,
                turn_id: Some(turn),
                worker_id: scope.worker_id,
            },
            event_id: selected.event_id,
            revision: selected.revision,
            review: Some(ReviewPin {
                history_identity,
                snapshot_id: receipt.snapshot_id,
                observed_at_unix_ns: timestamp,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn review_requires_an_exact_selected_prompt_and_refuses_cross_scope_rows() {
        let receipt = json!({
            "format":FORMAT,"history_identity":"1".repeat(32),"snapshot_id":"20",
            "scope":{"source_id":"harness.pi","harness_id":"harness.pi","session_id":"session",
                "turn_id":"turn","worker_id":null,"worker_scope":"workerless"},
            "observations":[{"harness_id":"harness.pi","event_id":"event","revision":2,
                "observed_at_unix_ns":"50","identity":{"scope":"session","session_id":"session","turn_id":"turn"},
                "observation":{"kind":"message","role":"user","spurious_text":"never comes from this file"}}],
        });
        let latest = ReviewFile {
            file: PathBuf::new(),
            event_id: None,
            revision: None,
        };
        let selected = latest.select_receipt(&receipt.to_string()).unwrap();
        assert_eq!(selected.event_id, "event");
        assert_eq!(selected.review.as_ref().unwrap().observed_at_unix_ns, 50);
        assert!(!format!("{selected:?}").contains("spurious_text"));
        let mut duplicate = receipt.clone();
        let mut later = duplicate["observations"][0].clone();
        later["revision"] = json!(3);
        duplicate["observations"]
            .as_array_mut()
            .unwrap()
            .push(later);
        assert!(latest.select_receipt(&duplicate.to_string()).is_err());
        let explicit = ReviewFile {
            file: PathBuf::new(),
            event_id: Some("event".into()),
            revision: Some(2),
        };
        assert_eq!(
            explicit
                .select_receipt(&duplicate.to_string())
                .unwrap()
                .revision,
            2
        );
        duplicate["observations"][1]["identity"]["worker_id"] = json!("sibling");
        assert!(explicit.select_receipt(&duplicate.to_string()).is_err());
        let mut standalone = receipt;
        standalone["scope"]["turn_id"] = serde_json::Value::Null;
        standalone["scope"]["trace_id"] = json!("2".repeat(32));
        assert!(latest.select_receipt(&standalone.to_string()).is_err());
    }
}
