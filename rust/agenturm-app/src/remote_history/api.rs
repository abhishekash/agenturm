//! Source-aware, bounded metadata GET routes over the dedicated reader.

use agenturm_kernel::{
    ExtensionId, ObservationKind, ObservationPosition, ObservationQuery, ReadSnapshot,
};
use agenturm_storage_service::native_read::ReadFailure;
use agenturm_storage_service::{NativeSelection, ReaderHistoryReply, ReaderHistoryRequest};
use agenturm_store::{SessionPosition, SessionQuery};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum View {
    Activity,
    Traces,
    Mcp,
    Usage,
    Relationships,
}

pub(super) enum Request {
    Health,
    Sessions(SessionQuery),
    History {
        query: ObservationQuery,
        view: View,
        trace_id: Option<String>,
    },
}

impl Request {
    pub fn parse(target: &str) -> Result<Self, &'static str> {
        if target.len() > 8 * 1024 || target.contains('#') {
            return Err("400 Bad Request");
        }
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let mut fields = fields(query)?;
        if path == "/api/health" {
            return if fields.is_empty() {
                Ok(Self::Health)
            } else {
                Err("400 Bad Request")
            };
        }
        let view = match path {
            "/api/sessions" => None,
            "/api/activity" => Some(View::Activity),
            "/api/traces" => Some(View::Traces),
            "/api/mcp" => Some(View::Mcp),
            "/api/metrics" => Some(View::Usage),
            "/api/hierarchy" => Some(View::Relationships),
            _ if path.starts_with("/api/traces/") && path.ends_with("/spans") => Some(View::Traces),
            _ => return Err("404 Not Found"),
        };
        let limit = fields
            .remove("limit")
            .map(|text| text.parse::<usize>().map_err(|_| "400 Bad Request"))
            .transpose()?
            .unwrap_or(64);
        if !(1..=256).contains(&limit) {
            return Err("400 Bad Request");
        }
        let source = fields.remove("source_id");
        if source
            .as_ref()
            .is_some_and(|id| ExtensionId::parse(id.clone()).is_err())
        {
            return Err("400 Bad Request");
        }
        let snapshot = fields
            .remove("snapshot_id")
            .map(|text| {
                serde_json::from_str::<ReadSnapshot>(&text)
                    .or_else(|_| serde_json::from_value(Value::String(text)))
                    .map_err(|_| "400 Bad Request")
            })
            .transpose()?;
        let before = fields.remove("cursor");
        if view.is_none() {
            let before = before
                .map(|value| {
                    serde_json::from_str::<SessionPosition>(&value).map_err(|_| "400 Bad Request")
                })
                .transpose()?;
            let snapshot_id =
                snapshot.or_else(|| before.as_ref().map(|position| position.snapshot_id));
            if before
                .as_ref()
                .is_some_and(|position| Some(position.snapshot_id) != snapshot_id)
                || !fields.is_empty()
            {
                return Err("400 Bad Request");
            }
            return Ok(Self::Sessions(SessionQuery {
                harness_id: source,
                snapshot_id,
                before,
                limit,
            }));
        }
        let session_id = fields.remove("session_id");
        if session_id
            .as_deref()
            .is_some_and(|id| !crate::trace_import::valid_session_id(id))
        {
            return Err("400 Bad Request");
        }
        let before = before
            .map(|value| {
                serde_json::from_str::<ObservationPosition>(&value).map_err(|_| "400 Bad Request")
            })
            .transpose()?;
        let snapshot_id = snapshot.or_else(|| before.as_ref().map(|position| position.snapshot_id));
        if before
            .as_ref()
            .is_some_and(|position| Some(position.snapshot_id) != snapshot_id)
            || !fields.is_empty()
        {
            return Err("400 Bad Request");
        }
        let trace_id = if let Some(id) = path
            .strip_prefix("/api/traces/")
            .and_then(|path| path.strip_suffix("/spans"))
        {
            if id.len() != 32
                || !id.bytes().all(|ch| ch.is_ascii_hexdigit())
                || id.bytes().all(|ch| ch == b'0')
                || !source
                    .as_deref()
                    .is_some_and(|source| source.starts_with("otlp."))
            {
                return Err("400 Bad Request");
            }
            Some(id.to_ascii_lowercase())
        } else {
            None
        };
        Ok(Self::History {
            query: ObservationQuery {
                harness_id: source,
                session_id,
                before,
                snapshot_id,
                limit,
            },
            view: view.expect("history view"),
            trace_id,
        })
    }

    pub fn is_health(&self) -> bool {
        matches!(self, Self::Health)
    }

    pub fn execute(
        self,
        binding: &str,
        read: &impl Fn(ReaderHistoryRequest) -> Result<ReaderHistoryReply, ReadFailure>,
    ) -> Result<Value, ReadFailure> {
        let mut body = match self {
            Self::Health => {
                let ReaderHistoryReply::Identity(_) = read(ReaderHistoryRequest::Identity)? else {
                    return Err(ReadFailure::Unavailable);
                };
                json!({ "status":"ok", "read_only":true, "version":env!("CARGO_PKG_VERSION") })
            }
            Self::Sessions(query) => {
                let ReaderHistoryReply::Sessions(page) =
                    read(ReaderHistoryRequest::Sessions { query })?
                else {
                    return Err(ReadFailure::Unavailable);
                };
                let next = page
                    .next_before
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|_| ReadFailure::Unavailable)?;
                let mut value = serde_json::to_value(page).map_err(|_| ReadFailure::Unavailable)?;
                value
                    .as_object_mut()
                    .ok_or(ReadFailure::Unavailable)?
                    .remove("next_before");
                value["next_cursor"] = json!(next);
                value
            }
            Self::History {
                query,
                view,
                trace_id,
            } => {
                let ReaderHistoryReply::History(mut page) = read(ReaderHistoryRequest::History {
                    query,
                    selection: NativeSelection::Observations,
                })?
                else {
                    return Err(ReadFailure::Unavailable);
                };
                let inspected = page.observations.len();
                let next = page
                    .older_position()
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|_| ReadFailure::Unavailable)?;
                page.observations.retain(|row| match (&row.kind, view) {
                    (ObservationKind::HostSample(_), _) => false,
                    (_, View::Activity) => true,
                    (ObservationKind::TraceSpan(span), View::Traces) => {
                        trace_id.as_ref().is_none_or(|id| id == &span.trace_id)
                    }
                    (ObservationKind::McpCall { .. }, View::Mcp) => true,
                    (
                        ObservationKind::Usage { .. }
                        | ObservationKind::CreditUsage { .. }
                        | ObservationKind::ReportedCost(_)
                        | ObservationKind::SpanTokenUsage(_),
                        View::Usage,
                    ) => true,
                    (ObservationKind::Relationship { .. }, View::Relationships) => true,
                    _ => false,
                });
                json!({ "snapshot_id":page.snapshot_id, "observations":page.observations, "inspected":inspected,
                    "coverage":"bounded metadata page", "has_more":page.has_more, "next_cursor":next })
            }
        };
        exact_integers(&mut body);
        body["format"] = json!("agenturm.http-history/1");
        body["history_identity"] = json!(binding);
        Ok(body)
    }
}

fn exact_integers(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let exact = matches!(key.as_str(), "snapshot_id" | "nanounits" | "nanocredits")
                    || ["_unix_ns", "_unix_nano", "_tokens", "_bytes", "_ticks"]
                        .iter()
                        .any(|suffix| key.ends_with(suffix));
                if exact && value.is_number() {
                    *value = Value::String(value.to_string());
                } else {
                    exact_integers(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                exact_integers(value);
            }
        }
        _ => {}
    }
}

fn fields(query: &str) -> Result<BTreeMap<String, String>, &'static str> {
    let mut fields = BTreeMap::new();
    for field in query.split('&').filter(|value| !value.is_empty()) {
        let (name, value) = field.split_once('=').ok_or("400 Bad Request")?;
        let (name, value) = (decode(name)?, decode(value)?);
        if value.is_empty() || fields.insert(name, value).is_some() {
            return Err("400 Bad Request");
        }
    }
    Ok(fields)
}

fn decode(value: &str) -> Result<String, &'static str> {
    let mut bytes = value.bytes();
    let mut decoded = Vec::with_capacity(value.len());
    while let Some(byte) = bytes.next() {
        let byte = if byte == b'%' {
            let high = bytes
                .next()
                .and_then(|b| (b as char).to_digit(16))
                .ok_or("400 Bad Request")?;
            let low = bytes
                .next()
                .and_then(|b| (b as char).to_digit(16))
                .ok_or("400 Bad Request")?;
            ((high << 4) | low) as u8
        } else if byte == b'+' {
            b' '
        } else {
            byte
        };
        if byte.is_ascii_control() {
            return Err("400 Bad Request");
        }
        decoded.push(byte);
    }
    String::from_utf8(decoded).map_err(|_| "400 Bad Request")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{
        Observation, ObservationIdentity, ObservationPage, TraceSpan, TraceStatus,
    };

    #[test]
    fn rejected_shapes_never_reach_the_reader() {
        for target in [
            "/api/health?source_id=otlp.a",
            "/api/sessions?limit=0",
            "/api/activity?limit=257",
            "/api/activity?source_id=otlp.a&source_id=otlp.b",
            "/api/activity?sql=select",
            "/api/activity?cursor=%",
            "/api/activity?session_id=%00",
            "/api/activity?session_id=%ff",
            "/api/traces/11111111111111111111111111111111/spans",
            "/api/activity?source_id=",
            "/api/host",
            "/api/spans",
        ] {
            assert!(Request::parse(target).is_err(), "{target}");
        }
    }

    #[test]
    fn filtered_pages_keep_the_last_inspected_position() {
        let id = ExtensionId::parse("otlp.a").unwrap();
        let row = Observation {
            harness_id: id,
            event_id: "one".into(),
            revision: 0,
            identity: ObservationIdentity::session("same"),
            observed_at_unix_ns: 10,
            kind: ObservationKind::TraceSpan(TraceSpan {
                trace_id: "11".repeat(16),
                span_id: "22".repeat(8),
                parent_span_id: None,
                target_source_id: None,
                started_at_unix_ns: 9,
                status: TraceStatus::Ok,
            }),
        };
        let page = ObservationPage {
            observations: vec![row],
            snapshot_id: 1.into(),
            has_more: true,
        };
        let cursor = page.older_position();
        let output = Request::parse("/api/mcp?source_id=otlp.a")
            .unwrap()
            .execute("binding", &|request| {
                let ReaderHistoryRequest::History { query, .. } = request else {
                    panic!("metadata only");
                };
                assert_eq!(query.harness_id.as_deref(), Some("otlp.a"));
                Ok(ReaderHistoryReply::History(page.clone()))
            })
            .unwrap();
        assert_eq!(output["inspected"], 1);
        assert!(output["observations"].as_array().unwrap().is_empty());
        assert_eq!(
            output["next_cursor"],
            serde_json::to_value(
                cursor
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .unwrap()
            )
            .unwrap()
        );
        assert_eq!(output["has_more"], true);
    }
    #[test]
    fn integer_evidence_and_opaque_cursor_survive_json_number_precision_limits() {
        let position = ObservationPosition {
            observed_at_unix_ns: (1_u64 << 53) + 1,
            harness_id: "otlp.a".into(),
            event_id: "precise".into(),
            revision: 0,
            snapshot_id: 1.into(),
        };
        let cursor = serde_json::to_string(&position).unwrap();
        let mut response = json!({"snapshot_id":1,"input_tokens":0,"nanounits":i64::MAX,"observed_at_unix_ns":position.observed_at_unix_ns,"next_cursor":cursor});
        exact_integers(&mut response);
        assert_eq!(response["input_tokens"], "0");
        assert_eq!(response["nanounits"], i64::MAX.to_string());
        assert_eq!(
            response["observed_at_unix_ns"],
            position.observed_at_unix_ns.to_string()
        );
        assert_eq!(
            serde_json::from_str::<ObservationPosition>(response["next_cursor"].as_str().unwrap())
                .unwrap(),
            position
        );
    }
}
