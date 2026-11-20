//! Historical exact query over a caller-supplied, isolated Store backup.
//!
//! Storage shadow validates the bounded, already-published projection under
//! one SQLite read transaction. Domain query supplies the shared exact-row
//! selector. Neither component grants live freshness or writer authority.
//! The supplied backup must be trusted: its revision is not an authenticity
//! proof. For live publication, Python must compare the selected row (or exact
//! absence) with `rust_shadow_snapshot.capture(...).expected`, then call
//! `rust_shadow_snapshot.current()` after the Rust result.

#![forbid(unsafe_code)]

use agenturm_domain_query::{ExactQuery, select_projected};
use agenturm_storage_shadow::inspect_json;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::error::Error;

pub const MAX_QUERY_BYTES: usize = 8 * 1024;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    sqlite_path: String,
    source_id: String,
    query: ExactQuery,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub lease_epoch: String,
    pub policy_epoch: String,
    pub dirty_seq: String,
    pub capture_enabled: bool,
    pub projected_seq: Option<String>,
    pub projected_policy: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Unknown,
    Pending,
    Disabled,
    Ready,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Certificate {
    schema_version: u32,
    receiver_id: String,
    source_id: String,
    state: State,
    revision: Option<Revision>,
    sessions: Option<Vec<Value>>,
    activities: Option<Vec<Value>>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Coverage {
    Unavailable,
    Partial,
}

#[derive(Serialize)]
pub struct HistoricalResult {
    pub schema_version: u32,
    pub receiver_id: String,
    pub source_id: String,
    pub state: State,
    pub revision: Option<Revision>,
    pub coverage: Coverage,
    pub query: ExactQuery,
    // Some(None) is an exact miss (JSON null); None means the query was not
    // evaluated because the source projection was unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<Option<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<Option<Value>>,
}

/// The result is historical even when `state` is ready. The backup must be
/// trusted. A Python caller publishing live data must match receiver, full
/// revision, and selected row or exact absence with its captured expected
/// projection, then call `rust_shadow_snapshot.current()` before publication.
pub fn query(input: &str) -> Result<HistoricalResult> {
    if input.len() > MAX_QUERY_BYTES {
        return Err(invalid("backup query exceeds byte limit"));
    }
    let request: Request = serde_json::from_str(input)?;
    if request.schema_version != 1 || request.query.source_id()? != request.source_id {
        return Err(invalid("invalid backup query source"));
    }
    let session_query = request.query.kind() == "session";
    let shadow_request = json!({
        "schema_version": 1,
        "sqlite_path": request.sqlite_path,
        "source_id": request.source_id,
        "include_projection": session_query,
        "include_activity": !session_query,
    });
    let certificate: Certificate =
        serde_json::from_value(inspect_json(&shadow_request.to_string())?)?;
    if certificate.schema_version != 1 || certificate.source_id != request.source_id {
        return Err(invalid("invalid backup certificate"));
    }
    let ready = certificate.state == State::Ready;
    let (session, activity) = if ready {
        let revision = certificate
            .revision
            .as_ref()
            .ok_or_else(|| invalid("ready source without revision"))?;
        if !revision.capture_enabled
            || revision.projected_seq.as_ref() != Some(&revision.dirty_seq)
            || revision.projected_policy.as_ref() != Some(&revision.policy_epoch)
        {
            return Err(invalid("inconsistent ready certificate"));
        }
        if session_query {
            if certificate.activities.is_some() {
                return Err(invalid("unexpected Activity projection"));
            }
            let sessions = certificate
                .sessions
                .as_ref()
                .ok_or_else(|| invalid("missing session projection"))?;
            let selected = select_projected(&request.query, &certificate.source_id, sessions, &[])?;
            (Some(selected), None)
        } else {
            if certificate.sessions.is_some() {
                return Err(invalid("unexpected session projection"));
            }
            let activities = certificate
                .activities
                .as_ref()
                .ok_or_else(|| invalid("missing Activity projection"))?;
            let selected = select_projected(
                &request.query,
                &certificate.source_id,
                &[] as &[Value],
                activities,
            )?;
            (None, Some(selected))
        }
    } else {
        if certificate.sessions.is_some() || certificate.activities.is_some() {
            return Err(invalid("unavailable source exposed projection"));
        }
        (None, None)
    };
    Ok(HistoricalResult {
        schema_version: 1,
        receiver_id: certificate.receiver_id,
        source_id: certificate.source_id,
        state: certificate.state,
        revision: certificate.revision,
        coverage: if ready {
            Coverage::Partial
        } else {
            Coverage::Unavailable
        },
        query: request.query,
        session,
        activity,
    })
}

pub fn query_json(input: &str) -> Result<Value> {
    Ok(serde_json::to_value(query(input)?)?)
}
