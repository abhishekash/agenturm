//! Pure, exact session query over a fenced, content-free reducer snapshot.
//!
//! The caller obtains the snapshot from Python Store and rechecks its lease,
//! policy and projection revision before using this offline result. This crate
//! has no database, filesystem, process, network or policy authority.

#![forbid(unsafe_code)]

use agenturm_domain_reducer::reduce_json;
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
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
    query: ExactSession,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExactSession {
    kind: String,
    source_id: String,
    native_session_id: String,
}

#[derive(Deserialize)]
struct SnapshotHeader {
    receiver_id: String,
    source_id: String,
    plugin_id: String,
    policy_epoch: u64,
    dirty_seq: u64,
}

#[derive(Serialize)]
struct Reply<'a> {
    schema_version: u32,
    receiver_id: &'a str,
    source_id: &'a str,
    plugin_id: &'a str,
    policy_epoch: String,
    dirty_seq: String,
    coverage: &'static str,
    query: &'a ExactSession,
    session: Option<&'a agenturm_domain_reducer::Session>,
}

fn native_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

/// Query only one exact native session from a version 3 admitted snapshot.
/// The returned `partial` coverage never treats an absent row as proof of a
/// complete source. The caller must recheck the snapshot's lease fence.
pub fn query_json(input: &str) -> Result<Value> {
    if input.len() > MAX_QUERY_BYTES {
        return Err(invalid("query exceeds byte limit"));
    }
    let request: Request<'_> = serde_json::from_str(input)?;
    if request.schema_version != 1
        || request.query.kind != "session"
        || !native_identity(&request.query.source_id)
        || !native_identity(&request.query.native_session_id)
    {
        return Err(invalid("invalid exact session query"));
    }
    let reduced = reduce_json(request.snapshot.get())?;
    if reduced.schema_version != 3 {
        return Err(invalid("query requires a version 3 snapshot"));
    }
    let header: SnapshotHeader = serde_json::from_str(request.snapshot.get())?;
    if request.query.source_id != header.source_id {
        return Err(invalid("query source differs from snapshot"));
    }
    let session = reduced
        .sessions
        .iter()
        .find(|row| row.native_session_id == request.query.native_session_id);
    Ok(serde_json::to_value(Reply {
        schema_version: 1,
        receiver_id: &header.receiver_id,
        source_id: &header.source_id,
        plugin_id: &header.plugin_id,
        policy_epoch: header.policy_epoch.to_string(),
        dirty_seq: header.dirty_seq.to_string(),
        coverage: "partial",
        query: &request.query,
        session,
    })?)
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
}
