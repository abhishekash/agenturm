//! An opt-in, data-only `capability-adapter/1` CountFacts example.
//!
//! This process has no Store, database, filesystem, or harness dependencies. It
//! receives only host-derived schema-v3 CountFacts and returns a pane model.

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};

const FRAME_LIMIT: usize = 64 * 1024;
const PANE_ID: &str = "count-observations";
const FEATURES: [&str; 1] = ["capability-adapter/1"];
const COUNT_KEYS: [&str; 3] = ["checks", "shell", "users"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    #[serde(rename = "type")]
    kind: String,
    protocol: u8,
    plugin_id: String,
    pane_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FactsRequest {
    #[serde(rename = "type")]
    kind: String,
    protocol: u8,
    seq: u64,
    facts: CountFacts,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CountFacts {
    state: String,
    observed_events: i64,
    counts: ExactCounts,
    window_complete: bool,
}

#[derive(Debug, Default)]
struct ExactCounts(BTreeMap<String, i64>);

impl<'de> Deserialize<'de> for ExactCounts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CountsVisitor;
        impl<'de> Visitor<'de> for CountsVisitor {
            type Value = ExactCounts;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object with exactly checks, shell, and users integer counts")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, i64>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate count key"));
                    }
                }
                if values.keys().map(String::as_str).collect::<Vec<_>>() != COUNT_KEYS.to_vec() {
                    return Err(serde::de::Error::custom("count keys differ from schema"));
                }
                Ok(ExactCounts(values))
            }
        }
        deserializer.deserialize_map(CountsVisitor)
    }
}

impl CountFacts {
    fn validate(&self) -> Result<(), &'static str> {
        if self.state != "ready" && self.state != "degraded" {
            return Err("invalid facts state");
        }
        if !(0..=200).contains(&self.observed_events) {
            return Err("observed_events outside schema bounds");
        }
        if self
            .counts
            .0
            .values()
            .any(|value| !(0..=200).contains(value))
        {
            return Err("count outside schema bounds");
        }
        Ok(())
    }
}

fn hello_ack(plugin_id: &str) -> Value {
    json!({"type":"hello_ack", "protocol":1, "plugin_id":plugin_id,
           "features":FEATURES})
}

fn count_model(seq: u64, facts: &CountFacts) -> Result<Value, &'static str> {
    facts.validate()?;
    let counts = &facts.counts.0;
    let users = counts["users"];
    let shell = counts["shell"];
    let checks = counts["checks"];
    let ratio = if users == 0 {
        "unavailable".to_owned()
    } else {
        // Integer arithmetic implements Python's nearest-hundredth rule,
        // including ties rounded upward, without floating-point drift.
        let scaled = (shell * 100 + users / 2) / users;
        format!("{}.{:02}", scaled / 100, scaled % 100)
    };
    let coverage = if facts.window_complete {
        "page complete; source may be partial"
    } else {
        "recent page partial"
    };
    let summary = format!(
        "{} scoped events · {} · observations only",
        facts.observed_events, coverage
    );
    Ok(json!({
        "type":"model", "protocol":1, "seq":seq, "state":facts.state,
        "summary":summary,
        "rows":[
            {"key":"users", "cells":["User messages", users.to_string()], "detail":""},
            {"key":"checks", "cells":["Failed checks", checks.to_string()], "detail":""},
            {"key":"shell-per-user", "cells":["Shell actions / User messages", ratio], "detail":""}
        ]
    }))
}

fn read_payload(input: &mut impl Read) -> Result<Option<String>, Box<dyn std::error::Error>> {
    Ok(agenturm_protocol::read_frame(input, FRAME_LIMIT)?)
}

fn write_json(output: &mut impl Write, value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    agenturm_protocol::write_frame(output, &serde_json::to_string(value)?, FRAME_LIMIT)?;
    Ok(())
}

fn run(input: &mut impl Read, output: &mut impl Write) -> Result<(), Box<dyn std::error::Error>> {
    let hello_raw = read_payload(input)?.ok_or("missing hello")?;
    let hello: Hello = serde_json::from_str(&hello_raw)?;
    if hello.kind != "hello"
        || hello.protocol != 1
        || hello.pane_id != PANE_ID
        || hello.plugin_id.is_empty()
    {
        return Err("invalid capability hello".into());
    }
    write_json(output, &hello_ack(&hello.plugin_id))?;

    let mut last_seq = 0;
    while let Some(raw) = read_payload(input)? {
        let request: FactsRequest = serde_json::from_str(&raw)?;
        if request.kind != "facts"
            || request.protocol != 1
            || request.seq == 0
            || request.seq <= last_seq
        {
            return Err("invalid facts request".into());
        }
        let model = count_model(request.seq, &request.facts)?;
        write_json(output, &model)?;
        last_seq = request.seq;
    }
    Ok(())
}

fn main() {
    if let Err(error) = run(&mut io::stdin().lock(), &mut io::stdout().lock()) {
        eprintln!("count capability protocol error: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_facts(
        state: &str,
        observed: i64,
        users: i64,
        shell: i64,
        checks: i64,
        complete: bool,
    ) -> CountFacts {
        CountFacts {
            state: state.to_owned(),
            observed_events: observed,
            counts: ExactCounts(BTreeMap::from([
                ("users".to_owned(), users),
                ("shell".to_owned(), shell),
                ("checks".to_owned(), checks),
            ])),
            window_complete: complete,
        }
    }

    fn facts_json(text: &str) -> Result<CountFacts, Box<dyn std::error::Error>> {
        let facts: CountFacts = serde_json::from_str(text)?;
        facts.validate()?;
        Ok(facts)
    }

    #[test]
    fn schema_v3_accepts_boundary_counts_and_matches_rounding() {
        let cases = [
            ("ready", 0, 0, 0, 0, true, "unavailable"),
            ("ready", 1, 1, 1, 0, true, "1.00"),
            ("ready", 200, 200, 1, 0, true, "0.01"),
            ("degraded", 200, 200, 101, 200, false, "0.51"),
        ];
        for (state, observed, users, shell, checks, complete, ratio) in cases {
            let facts = valid_facts(state, observed, users, shell, checks, complete);
            let model = count_model(7, &facts).unwrap();
            assert_eq!(model["seq"], 7);
            assert_eq!(model["state"], state);
            assert_eq!(model["rows"][2]["cells"][1], ratio);
            assert!(model["summary"].as_str().unwrap().contains(if complete {
                "page complete; source may be partial"
            } else {
                "recent page partial"
            }));
        }
    }

    #[test]
    fn rejects_unknown_duplicate_missing_wrong_type_and_out_of_range_facts() {
        for raw in [
            r#"{"state":"ready","observed_events":0,"counts":{"users":0,"shell":0,"checks":0},"window_complete":true,"extra":1}"#,
            r#"{"state":"ready","observed_events":0,"counts":{"users":0,"shell":0,"checks":0,"other":0},"window_complete":true}"#,
            r#"{"state":"ready","observed_events":0,"counts":{"users":0,"shell":0},"window_complete":true}"#,
            r#"{"state":"ready","observed_events":0,"counts":{"users":0,"users":1,"shell":0,"checks":0},"window_complete":true}"#,
            r#"{"state":"ready","state":"degraded","observed_events":0,"counts":{"users":0,"shell":0,"checks":0},"window_complete":true}"#,
            r#"{"state":"ready","observed_events":true,"counts":{"users":0,"shell":0,"checks":0},"window_complete":true}"#,
            r#"{"state":"ready","observed_events":0,"counts":{"users":false,"shell":0,"checks":0},"window_complete":true}"#,
            r#"{"state":"ready","observed_events":201,"counts":{"users":0,"shell":0,"checks":0},"window_complete":true}"#,
            r#"{"state":"ready","observed_events":0,"counts":{"users":0,"shell":-1,"checks":0},"window_complete":true}"#,
            r#"{"state":"ready","observed_events":0,"counts":{"users":0,"shell":0,"checks":201},"window_complete":true}"#,
            r#"{"state":"ready","observed_events":0,"counts":{"users":0,"shell":0,"checks":0},"window_complete":1}"#,
        ] {
            assert!(facts_json(raw).is_err(), "accepted malformed facts: {raw}");
        }
    }

    #[test]
    fn rejects_invalid_state_and_nonmonotonic_or_zero_sequence() {
        let bad_state = valid_facts("unknown", 0, 0, 0, 0, false);
        assert!(count_model(1, &bad_state).is_err());
        assert!(count_model(0, &valid_facts("ready", 0, 0, 0, 0, false)).is_ok());
        let hello = json!({"type":"hello","protocol":1,"plugin_id":"example.test",
                           "pane_id":PANE_ID});
        let request = json!({"type":"facts","protocol":1,"seq":1,
            "facts":{"state":"ready","observed_events":0,
                     "counts":{"users":0,"shell":0,"checks":0},"window_complete":true}});
        let mut stream = Vec::new();
        for item in [&hello, &request, &request] {
            agenturm_protocol::write_frame(&mut stream, &item.to_string(), FRAME_LIMIT).unwrap();
        }
        let mut response = Vec::new();
        assert!(run(&mut stream.as_slice(), &mut response).is_err());
        let mut zero = request.clone();
        zero["seq"] = json!(0);
        let mut stream = Vec::new();
        agenturm_protocol::write_frame(&mut stream, &hello.to_string(), FRAME_LIMIT).unwrap();
        agenturm_protocol::write_frame(&mut stream, &zero.to_string(), FRAME_LIMIT).unwrap();
        assert!(run(&mut stream.as_slice(), &mut Vec::new()).is_err());
    }

    #[test]
    fn rejects_invalid_handshake_and_oversized_frames() {
        let bad = json!({"type":"hello","protocol":1,"plugin_id":"x","pane_id":"wrong"});
        let mut stream = Vec::new();
        agenturm_protocol::write_frame(&mut stream, &bad.to_string(), FRAME_LIMIT).unwrap();
        assert!(run(&mut stream.as_slice(), &mut Vec::new()).is_err());
        let oversized = (FRAME_LIMIT as u32 + 1).to_be_bytes().to_vec();
        assert!(agenturm_protocol::read_frame(&mut oversized.as_slice(), FRAME_LIMIT).is_err());
    }

    fn framed_raw(payload: &[u8]) -> Vec<u8> {
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn rejects_duplicate_handshake_and_facts_envelope_fields() {
        let duplicate_hello = framed_raw(
            br#"{"type":"hello","protocol":1,"plugin_id":"x","pane_id":"count-observations","pane_id":"count-observations"}"#);
        assert!(run(&mut duplicate_hello.as_slice(), &mut Vec::new()).is_err());

        let hello = json!({"type":"hello","protocol":1,"plugin_id":"x","pane_id":PANE_ID});
        let hello_frame = {
            let mut frame = Vec::new();
            agenturm_protocol::write_frame(&mut frame, &hello.to_string(), FRAME_LIMIT).unwrap();
            frame
        };
        let duplicate_facts = framed_raw(
            br#"{"type":"facts","protocol":1,"seq":1,"facts":{"state":"ready","observed_events":0,"counts":{"users":0,"shell":0,"checks":0},"window_complete":true,"window_complete":false}}"#);
        let mut stream = hello_frame;
        stream.extend_from_slice(&duplicate_facts);
        assert!(run(&mut stream.as_slice(), &mut Vec::new()).is_err());
    }

    #[test]
    fn successful_process_uses_exact_ack_and_model_envelopes() {
        let hello = json!({"type":"hello","protocol":1,"plugin_id":"example.test",
                           "pane_id":PANE_ID});
        let request = json!({"type":"facts","protocol":1,"seq":1,
            "facts":{"state":"degraded","observed_events":2,
                     "counts":{"users":1,"shell":2,"checks":0},"window_complete":false}});
        let mut stream = Vec::new();
        agenturm_protocol::write_frame(&mut stream, &hello.to_string(), FRAME_LIMIT).unwrap();
        agenturm_protocol::write_frame(&mut stream, &request.to_string(), FRAME_LIMIT).unwrap();
        let mut output = Vec::new();
        run(&mut stream.as_slice(), &mut output).unwrap();
        let mut output = output.as_slice();
        let ack: Value = serde_json::from_str(
            &agenturm_protocol::read_frame(&mut output, FRAME_LIMIT)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let model: Value = serde_json::from_str(
            &agenturm_protocol::read_frame(&mut output, FRAME_LIMIT)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(ack, hello_ack("example.test"));
        assert_eq!(model["type"], "model");
        assert_eq!(model["seq"], 1);
        assert_eq!(model["rows"][2]["cells"][1], "2.00");
        assert_eq!(model["rows"][0]["cells"][0], "User messages");
        assert_eq!(model.as_object().unwrap().keys().count(), 6);
    }
}
