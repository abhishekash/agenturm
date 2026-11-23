//! Source-aware, bounded metadata GET routes over the dedicated reader.

use agenturm_kernel::retained_history::HistoryProjection;
use agenturm_kernel::{
    ExtensionId, ObservationPosition, ObservationQuery, ReadSnapshot, is_host_source,
};
use agenturm_storage_service::native_read::ReadFailure;
use agenturm_storage_service::{
    NativeSelection, ProjectedHistoryScope, ReaderHistoryReply, ReaderHistoryRequest,
};
use agenturm_store::{
    EvalInventoryQuery, MAX_EVAL_INVENTORY_SOURCES, MAX_HOST_HISTORY_PAGE, SessionPosition,
    SessionQuery,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) enum Request {
    Health,
    EvalInventory(EvalInventoryQuery),
    Sessions(SessionQuery),
    HostHistory(ObservationQuery),
    History {
        query: ObservationQuery,
        projection: HistoryProjection,
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
        if path == "/api/outcomes" {
            let raw = fields.remove("source_ids").ok_or("400 Bad Request")?;
            if !fields.is_empty() {
                return Err("400 Bad Request");
            }
            let source_ids = raw
                .split(',')
                .take(MAX_EVAL_INVENTORY_SOURCES + 1)
                .map(|source| ExtensionId::parse(source).map_err(|_| "400 Bad Request"))
                .collect::<Result<Vec<_>, _>>()?;
            let query = EvalInventoryQuery { source_ids };
            query.validate().map_err(|_| "400 Bad Request")?;
            return Ok(Self::EvalInventory(query));
        }
        let host_history = path == "/api/host";
        let projection = match path {
            "/api/sessions" | "/api/host" => None,
            "/api/activity" => Some(HistoryProjection::Activity),
            "/api/traces" => Some(HistoryProjection::Traces { trace_id: None }),
            "/api/mcp" => {
                let query = fields.remove("q");
                if query
                    .as_deref()
                    .is_some_and(|query| !HistoryProjection::valid_mcp_query(query))
                {
                    return Err("400 Bad Request");
                }
                Some(HistoryProjection::Mcp { query })
            }
            "/api/metrics" => Some(HistoryProjection::Usage),
            "/api/hierarchy" => Some(HistoryProjection::Relationships),
            _ => Some(trace_projection(path)?),
        };
        let limit = fields
            .remove("limit")
            .map(|text| text.parse::<usize>().map_err(|_| "400 Bad Request"))
            .transpose()?
            .unwrap_or(64);
        let maximum = if host_history {
            MAX_HOST_HISTORY_PAGE
        } else {
            256
        };
        if !(1..=maximum).contains(&limit) {
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
        if projection.is_none() && !host_history {
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
        let query = ObservationQuery {
            harness_id: source,
            session_id,
            before,
            snapshot_id,
            limit,
        };
        if host_history {
            if query.session_id.is_some()
                || query
                    .harness_id
                    .as_deref()
                    .is_some_and(|id| !is_host_source(id))
                || query
                    .before
                    .as_ref()
                    .is_some_and(|cursor| !is_host_source(&cursor.harness_id))
            {
                return Err("400 Bad Request");
            }
            return Ok(Self::HostHistory(query));
        }
        let projection = projection.expect("history projection");
        if matches!(
            &projection,
            HistoryProjection::Traces { trace_id: Some(_) }
                | HistoryProjection::Progress { trace_id: Some(_) }
        ) && !query
            .harness_id
            .as_deref()
            .is_some_and(|source| source.starts_with("otlp."))
        {
            return Err("400 Bad Request");
        }
        Ok(Self::History { query, projection })
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
            Self::EvalInventory(query) => {
                let ReaderHistoryReply::EvalInventory(inventory) =
                    read(ReaderHistoryRequest::EvalInventory { query })?
                else {
                    return Err(ReadFailure::Unavailable);
                };
                let mut value =
                    serde_json::to_value(&inventory).map_err(|_| ReadFailure::Unavailable)?;
                for (row, outcome) in value["outcomes"]
                    .as_array_mut()
                    .ok_or(ReadFailure::Unavailable)?
                    .iter_mut()
                    .zip(&inventory.outcomes)
                {
                    row["revision"] = json!(outcome.revision.to_string());
                }
                value
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
            Self::HostHistory(query) => {
                let ReaderHistoryReply::HostHistory(page) =
                    read(ReaderHistoryRequest::HostHistory { query })?
                else {
                    return Err(ReadFailure::Unavailable);
                };
                let next = page
                    .older_position()
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|_| ReadFailure::Unavailable)?;
                json!({ "snapshot_id":page.snapshot_id, "samples":page.observations,
                    "coverage":"bounded recorded Host page", "has_more":page.has_more, "next_cursor":next })
            }
            Self::History { query, projection } => {
                let ReaderHistoryReply::History(page) = read(ReaderHistoryRequest::History {
                    query,
                    selection: NativeSelection::Projected {
                        projection,
                        scope: ProjectedHistoryScope::All,
                    },
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

fn trace_projection(path: &str) -> Result<HistoryProjection, &'static str> {
    let path = path.strip_prefix("/api/traces/").ok_or("404 Not Found")?;
    let (id, spans) = if let Some(id) = path.strip_suffix("/spans") {
        (id, true)
    } else if let Some(id) = path.strip_suffix("/progress") {
        (id, false)
    } else {
        return Err("404 Not Found");
    };
    if id.len() != 32
        || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || id.bytes().all(|byte| byte == b'0')
    {
        return Err("400 Bad Request");
    }
    let trace_id = Some(id.to_ascii_lowercase());
    Ok(if spans {
        HistoryProjection::Traces { trace_id }
    } else {
        HistoryProjection::Progress { trace_id }
    })
}

fn exact_integers(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let exact = matches!(
                    key.as_str(),
                    "snapshot_id"
                        | "trace_evidence_snapshot"
                        | "nanounits"
                        | "nanocredits"
                        | "clock_ticks_per_second"
                ) || [
                    "_unix_ns",
                    "_unix_nano",
                    "_tokens",
                    "_bytes",
                    "_ticks",
                    "_milli",
                ]
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

    #[test]
    fn outcome_inventory_requires_only_an_explicit_unique_source_set() {
        let Request::EvalInventory(query) =
            Request::parse("/api/outcomes?source_ids=otlp.a%2Cotlp.b")
                .ok()
                .unwrap()
        else {
            panic!("expected an outcome inventory request");
        };
        assert_eq!(
            query
                .source_ids
                .iter()
                .map(ExtensionId::as_str)
                .collect::<Vec<_>>(),
            ["otlp.a", "otlp.b"]
        );
        for target in [
            "/api/outcomes",
            "/api/outcomes?source_ids=otlp.a,otlp.a",
            "/api/outcomes?source_ids=otlp.a,harness.pi",
            "/api/outcomes?source_ids=otlp.a,",
            "/api/outcomes?source_ids=otlp.a&source_ids=otlp.b",
            "/api/outcomes?source_id=otlp.a",
            "/api/outcomes?source_ids=otlp.a&limit=1",
            "/api/outcomes?source_ids=otlp.a&cursor=x",
            "/api/outcomes?source_ids=otlp.a&session_id=x",
            "/api/outcomes?source_ids=otlp.a&snapshot_id=1",
        ] {
            assert!(
                matches!(Request::parse(target), Err("400 Bad Request")),
                "{target}"
            );
        }
        let too_many = (0..=16)
            .map(|index| format!("otlp.s{index}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(matches!(
            Request::parse(&format!("/api/outcomes?source_ids={too_many}")),
            Err("400 Bad Request")
        ));
    }

    #[test]
    fn outcome_inventory_http_preserves_precision_and_unknown_score_values() {
        use agenturm_store::{
            EvalInventory, EvalInventoryRow, EvalOutcome, EvalScore, EvaluationMetadata,
        };

        let exact = (1_u64 << 53) + 1;
        let output = Request::parse("/api/outcomes?source_ids=otlp.a")
            .ok()
            .unwrap()
            .execute("read-binding", &|request| {
                let ReaderHistoryRequest::EvalInventory { query } = request else {
                    panic!("expected an authorized reader request");
                };
                let trace_source_id = query.source_ids[0].clone();
                Ok(ReaderHistoryReply::EvalInventory(EvalInventory {
                    source_ids: query.source_ids,
                    trace_evidence_snapshot: ReadSnapshot::new(exact, None),
                    evaluation_metadata: EvaluationMetadata::Current,
                    outcomes: vec![EvalInventoryRow {
                        trace_source_id,
                        revision: exact,
                        outcome: EvalOutcome {
                            trace_id: "11".repeat(16),
                            passed: false,
                            task_id: None,
                            provider_model: None,
                            scores: vec![
                                EvalScore {
                                    scorer: "zero".into(),
                                    passed: Some(false),
                                    score: Some(0.into()),
                                },
                                EvalScore {
                                    scorer: "unknown".into(),
                                    passed: Some(true),
                                    score: None,
                                },
                            ],
                        },
                    }],
                }))
            })
            .unwrap();
        assert_eq!(output["history_identity"], "read-binding");
        assert_eq!(output["evaluation_metadata"], "current");
        assert_eq!(output["trace_evidence_snapshot"], exact.to_string());
        assert_eq!(output["outcomes"][0]["revision"], exact.to_string());
        assert_eq!(output["outcomes"][0]["outcome"]["scores"][0]["score"], 0);
        assert_eq!(
            output["outcomes"][0]["outcome"]["scores"][0]["passed"],
            false
        );
        assert!(output["outcomes"][0]["outcome"]["scores"][1]["score"].is_null());
        assert_eq!(
            output["outcomes"][0]["outcome"]["scores"][1]["passed"],
            true
        );
        assert!(output["outcomes"][0].get("detail_selection").is_none());
    }
    use agenturm_kernel::{
        McpMethod, Observation, ObservationIdentity, ObservationKind, ObservationPage,
    };

    #[test]
    fn rejected_shapes_never_reach_the_reader() {
        for target in [
            "/api/health?source_id=otlp.a",
            "/api/sessions?limit=0",
            "/api/activity?limit=257",
            "/api/activity?source_id=otlp.a&source_id=otlp.b",
            "/api/activity?sql=select",
            "/api/activity?q=not-mcp",
            "/api/mcp?q=%20",
            "/api/mcp?q=%C2%85",
            "/api/activity?cursor=%",
            "/api/activity?session_id=%00",
            "/api/activity?session_id=%ff",
            "/api/traces/11111111111111111111111111111111/spans",
            "/api/activity?source_id=",
            "/api/host?session_id=not-host-scoped",
            "/api/host?source_id=otlp.other",
            "/api/host?limit=65",
            "/api/spans",
        ] {
            assert!(Request::parse(target).is_err(), "{target}");
        }
    }

    #[test]
    fn host_history_get_uses_the_separate_recorded_host_request() {
        let value = Request::parse("/api/host?source_id=host.receiver&limit=2&snapshot_id=5")
            .unwrap()
            .execute("binding", &|request| {
                let ReaderHistoryRequest::HostHistory { query } = request else {
                    panic!("Host history must use a dedicated request");
                };
                assert_eq!(query.harness_id.as_deref(), Some("host.receiver"));
                assert!(query.session_id.is_none());
                assert_eq!(query.limit, 2);
                assert_eq!(query.snapshot_id, Some(5.into()));
                Ok(ReaderHistoryReply::HostHistory(ObservationPage {
                    snapshot_id: 5.into(),
                    ..Default::default()
                }))
            })
            .unwrap();
        assert_eq!(value["snapshot_id"], "5");
        assert_eq!(value["samples"], json!([]));
        assert!(value.get("observations").is_none());
    }

    #[test]
    fn mcp_search_sends_literal_projection_and_uses_the_last_matching_position() {
        let id = ExtensionId::parse("otlp.a").unwrap();
        let row = Observation {
            harness_id: id,
            event_id: "one".into(),
            revision: 0,
            identity: ObservationIdentity::session("same"),
            observed_at_unix_ns: 10,
            kind: ObservationKind::McpCall {
                server: Some("test_server".into()),
                tool: None,
                method: McpMethod::ToolsCall,
                duration_ns: None,
                result: "succeeded".into(),
            },
        };
        let page = ObservationPage {
            observations: vec![row],
            snapshot_id: 1.into(),
            has_more: true,
        };
        let cursor = page.older_position();
        let output = Request::parse("/api/mcp?source_id=otlp.a&q=TEST%5F&limit=1")
            .unwrap()
            .execute("binding", &|request| {
                let ReaderHistoryRequest::History {
                    query,
                    selection:
                        NativeSelection::Projected {
                            projection,
                            scope: ProjectedHistoryScope::All,
                        },
                } = request
                else {
                    panic!("expected a source-scoped projection");
                };
                assert_eq!(query.harness_id.as_deref(), Some("otlp.a"));
                assert_eq!(query.limit, 1);
                assert_eq!(
                    projection,
                    HistoryProjection::Mcp {
                        query: Some("TEST_".into())
                    }
                );
                Ok(ReaderHistoryReply::History(page.clone()))
            })
            .unwrap();
        assert_eq!(output["inspected"], 1);
        assert_eq!(output["observations"][0]["event_id"], "one");
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
    fn trace_spans_and_progress_use_distinct_exact_source_scoped_projections() {
        let trace = "AB".repeat(16);
        for (route, expected) in [
            (
                "spans",
                HistoryProjection::Traces {
                    trace_id: Some(trace.to_ascii_lowercase()),
                },
            ),
            (
                "progress",
                HistoryProjection::Progress {
                    trace_id: Some(trace.to_ascii_lowercase()),
                },
            ),
        ] {
            let request = Request::parse(&format!(
                "/api/traces/{trace}/{route}?source_id=otlp.fixture&limit=1&session_id=one"
            ))
            .unwrap();
            request
                .execute("binding", &|request| {
                    let ReaderHistoryRequest::History {
                        query,
                        selection:
                            NativeSelection::Projected {
                                projection,
                                scope: ProjectedHistoryScope::All,
                            },
                    } = request
                    else {
                        panic!("expected exact projection")
                    };
                    assert_eq!(projection, expected);
                    assert_eq!(query.harness_id.as_deref(), Some("otlp.fixture"));
                    assert_eq!(query.session_id.as_deref(), Some("one"));
                    assert_eq!(query.limit, 1);
                    Ok(ReaderHistoryReply::History(ObservationPage::default()))
                })
                .unwrap();
        }
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
