//! Pure, exact session or Activity query over a fenced reducer snapshot.
//!
//! The caller obtains the snapshot from Python Store and rechecks its lease,
//! policy and projection revision before using this offline result. This crate
//! has no database, filesystem, process, network or policy authority.

#![forbid(unsafe_code)]

use agenturm_domain_reducer::{Session, reduce_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json, value::RawValue};
use std::error::Error;

pub const MAX_QUERY_BYTES: usize = agenturm_domain_reducer::MAX_FIXTURE_BYTES + 1024;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request<'a> {
    schema_version: u32,
    #[serde(borrow)]
    snapshot: &'a RawValue,
    query: ExactQuery,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactSession {
    kind: String,
    source_id: String,
    native_session_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactActivity {
    kind: String,
    source_id: String,
    event_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub enum ExactQuery {
    Session(ExactSession),
    Activity(ExactActivity),
}

impl ExactQuery {
    /// Validate the exact, source-scoped identity before selecting rows.
    pub fn source_id(&self) -> Result<&str> {
        match self {
            Self::Session(query)
                if query.kind == "session"
                    && native_identity(&query.source_id)
                    && native_identity(&query.native_session_id) =>
            {
                Ok(&query.source_id)
            }
            Self::Activity(query)
                if query.kind == "activity"
                    && native_identity(&query.source_id)
                    && event_identity(&query.event_id) =>
            {
                Ok(&query.source_id)
            }
            _ => Err(invalid("invalid exact domain query")),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Session(_) => "session",
            Self::Activity(_) => "activity",
        }
    }
}

/// A projected session identity used by the shared exact selector.
pub trait ProjectedSession: Serialize {
    fn native_session_id(&self) -> Option<&str>;
}

impl ProjectedSession for Session {
    fn native_session_id(&self) -> Option<&str> {
        Some(&self.native_session_id)
    }
}

impl ProjectedSession for Value {
    fn native_session_id(&self) -> Option<&str> {
        self["native_session_id"].as_str()
    }
}

/// Select from already validated, source-scoped projection rows. The caller
/// must validate row provenance and source revision before calling this pure
/// selector; absence means only that the exact row is absent in that snapshot.
pub fn select_projected<S: ProjectedSession>(
    query: &ExactQuery,
    source_id: &str,
    sessions: &[S],
    activities: &[Value],
) -> Result<Option<Value>> {
    if query.source_id()? != source_id {
        return Err(invalid("query source differs from snapshot"));
    }
    match query {
        ExactQuery::Session(query) => sessions
            .iter()
            .find(|row| row.native_session_id() == Some(&query.native_session_id))
            .map(serde_json::to_value)
            .transpose()
            .map_err(Into::into),
        ExactQuery::Activity(query) => Ok(activities
            .iter()
            .find(|row| row["id"].as_str() == Some(&query.event_id))
            .cloned()),
    }
}

#[derive(Deserialize)]
struct SnapshotHeader {
    receiver_id: String,
    source_id: String,
    plugin_id: String,
    policy_epoch: u64,
    dirty_seq: u64,
}

fn native_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn event_identity(value: &str) -> bool {
    value.strip_prefix("plugin-event:").is_some_and(|opaque| {
        opaque.len() == 64
            && opaque
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Query one exact native session or opaque Activity ID from a v3 snapshot.
/// The returned `partial` coverage never treats an absent row as proof of a
/// complete source. The caller must recheck the snapshot's lease fence.
pub fn query_json(input: &str) -> Result<Value> {
    if input.len() > MAX_QUERY_BYTES {
        return Err(invalid("query exceeds byte limit"));
    }
    let request: Request<'_> = serde_json::from_str(input)?;
    let source_id = request.query.source_id()?;
    if request.schema_version != 1 {
        return Err(invalid("invalid exact session query"));
    }
    let reduced = reduce_json(request.snapshot.get())?;
    if reduced.schema_version != 3 {
        return Err(invalid("query requires a version 3 snapshot"));
    }
    let header: SnapshotHeader = serde_json::from_str(request.snapshot.get())?;
    if source_id != header.source_id {
        return Err(invalid("query source differs from snapshot"));
    }
    let mut reply = json!({
        "schema_version": 1,
        "receiver_id": header.receiver_id,
        "source_id": header.source_id,
        "plugin_id": header.plugin_id,
        "policy_epoch": header.policy_epoch.to_string(),
        "dirty_seq": header.dirty_seq.to_string(),
        "coverage": "partial",
        "query": &request.query,
    });
    let activities = if request.query.kind() == "activity" {
        reduced
            .activities
            .as_deref()
            .ok_or_else(|| invalid("missing v3 Activity"))?
    } else {
        &[]
    };
    reply[request.query.kind()] = serde_json::to_value(select_projected(
        &request.query,
        &header.source_id,
        &reduced.sessions,
        activities,
    )?)?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_query_envelopes_fail_closed() {
        let snapshot = serde_json::json!({"schema_version":3,"receiver_id":"receiver",
            "source_id":"source","plugin_id":"fixture.plugin","policy_epoch":1,
            "dirty_seq":1,"rows":[],"invalidated":[],"source_gaps":[]});
        let request = |id: &str| {
            serde_json::json!({"schema_version":1,"snapshot":snapshot,
            "query":{"kind":"session","source_id":"source","native_session_id":id}})
            .to_string()
        };
        assert!(query_json(&request("valid-id")).is_ok());
        assert!(query_json(&request("invalid id")).is_err());
        assert!(query_json(&request(&"a".repeat(257))).is_err());
        assert!(query_json(&format!("{{\"schema_version\":1,\"schema_version\":1,\"snapshot\":{snapshot},\"query\":{{\"kind\":\"session\",\"source_id\":\"source\",\"native_session_id\":\"valid\"}}}}"))
            .is_err());
        assert!(query_json(&" ".repeat(MAX_QUERY_BYTES + 1)).is_err());
    }

    #[test]
    fn exact_activity_is_scoped_and_always_partial() {
        let snapshot = serde_json::json!({"schema_version":3,"receiver_id":"receiver",
            "source_id":"source","plugin_id":"fixture.plugin","policy_epoch":1,
            "dirty_seq":1,"rows":[{"event_id_hash":"a".repeat(64),"revision":0,
                "kind":"message","time_ns":"10","admitted_ns":"11",
                "native_session_id":"native-one","native_turn_id":"turn-one",
                "payload":{"role":"user"}}],"invalidated":[],"source_gaps":[]});
        let reduced = reduce_json(&snapshot.to_string()).unwrap();
        let event_id = reduced.activities.unwrap()[0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let request = |source: &str, id: &str| {
            serde_json::json!({"schema_version":1,"snapshot":snapshot,
                "query":{"kind":"activity","source_id":source,"event_id":id}})
            .to_string()
        };
        let reply = query_json(&request("source", &event_id)).unwrap();
        assert_eq!(reply["activity"]["id"], event_id);
        assert_eq!(reply["coverage"], "partial");
        assert_eq!(
            query_json(&request(
                "source",
                &format!("plugin-event:{}", "0".repeat(64))
            ))
            .unwrap()["activity"],
            Value::Null
        );
        assert!(query_json(&request("wrong", &event_id)).is_err());
        assert!(query_json(&request("source", "bad-event")).is_err());
        let unknown = serde_json::json!({"schema_version":1,"snapshot":snapshot,
            "query":{"kind":"activity","source_id":"source","event_id":event_id,
                     "native_session_id":"native-one"}});
        assert!(query_json(&unknown.to_string()).is_err());
    }

    #[test]
    fn shared_selector_needs_only_requested_validated_domain() {
        let session: ExactQuery = serde_json::from_value(json!({
            "kind":"session","source_id":"source","native_session_id":"native-one"
        }))
        .unwrap();
        let sessions = vec![json!({"native_session_id":"native-one","reported_state":"started"})];
        assert_eq!(
            select_projected(&session, "source", &sessions, &[]).unwrap(),
            Some(sessions[0].clone())
        );
        assert!(select_projected(&session, "other", &sessions, &[]).is_err());
        let activity: ExactQuery = serde_json::from_value(json!({
            "kind":"activity","source_id":"source",
            "event_id":format!("plugin-event:{}", "a".repeat(64))
        }))
        .unwrap();
        let activities = vec![json!({"id":format!("plugin-event:{}", "a".repeat(64))})];
        assert_eq!(
            select_projected(&activity, "source", &[] as &[Value], &activities).unwrap(),
            Some(activities[0].clone())
        );
    }
}
