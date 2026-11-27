//! Separately compiled, deterministic protocol fixture for the Python host.
//!
//! This is not a production harness or general plugin SDK. It emits one
//! synthetic lifecycle record, then proves that idle polls keep their cursor.

#![forbid(unsafe_code)]

use agenturm_protocol::{MAX_FRAME_BYTES, read_frame, write_frame};
use serde_json::{Value, json};
use std::error::Error;
use std::io::{self, BufReader, BufWriter};

const PLUGIN_ID: &str = "fixture.rust";
const CONTROL_LIMIT: usize = 64 * 1024;
const CURSOR: &str = "rust-fixture-next";

#[derive(Default)]
struct Session {
    source_id: String,
    hello_done: bool,
    pending: Option<String>,
    stopped: bool,
    host_limit: usize,
    data_limit: usize,
}

impl Session {
    fn handle(&mut self, message: Value) -> Result<Option<Value>, Box<dyn Error>> {
        let object = message
            .as_object()
            .ok_or("control message must be an object")?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or("missing control type")?;
        if object.get("protocol").and_then(Value::as_u64) != Some(1) {
            return Err("protocol mismatch".into());
        }
        if !self.hello_done {
            if kind != "hello" || object.get("plugin_id").and_then(Value::as_str) != Some(PLUGIN_ID)
            {
                return Err("hello identity mismatch".into());
            }
            let source = object
                .get("source_id")
                .and_then(Value::as_str)
                .ok_or("missing source ID")?;
            if source.is_empty()
                || source.chars().count() > 256
                || source
                    .chars()
                    .any(|char| (char as u32) < 32 || char == '\u{7f}')
            {
                return Err("invalid source ID".into());
            }
            self.source_id = source.to_owned();
            let limit = object
                .get("max_frame_bytes")
                .and_then(Value::as_u64)
                .ok_or("missing frame budget")?;
            self.host_limit = usize::try_from(limit)?.min(MAX_FRAME_BYTES);
            if self.host_limit == 0 {
                return Err("invalid frame budget".into());
            }
            self.data_limit = self.host_limit;
            self.hello_done = true;
            return Ok(Some(
                json!({"type":"hello_ack","protocol":1,"plugin_id":PLUGIN_ID,
                                  "features":["source-adapter/1"]}),
            ));
        }
        if kind == "stop" {
            self.stopped = true;
            return Ok(None);
        }
        let source = object
            .get("source_id")
            .and_then(Value::as_str)
            .ok_or("missing source ID")?;
        if source != self.source_id {
            return Err("source mismatch".into());
        }
        match kind {
            "poll" => {
                if self.pending.is_some() {
                    return Err("prior batch was not acknowledged".into());
                }
                let cursor = object
                    .get("cursor")
                    .and_then(Value::as_str)
                    .ok_or("missing cursor")?;
                let max_records = object
                    .get("max_records")
                    .and_then(Value::as_u64)
                    .ok_or("missing record budget")?;
                let max_bytes = object
                    .get("max_bytes")
                    .and_then(Value::as_u64)
                    .ok_or("missing byte budget")?;
                if max_records == 0 || max_records > 256 || (!cursor.is_empty() && cursor != CURSOR)
                {
                    return Err("invalid poll request".into());
                }
                let max_bytes = usize::try_from(max_bytes)?.min(self.host_limit);
                if max_bytes == 0 {
                    return Err("invalid poll byte budget".into());
                }
                self.data_limit = max_bytes;
                let records = if cursor.is_empty() {
                    vec![json!({"event_id":"rust-fixture-1","revision":0,
                                "kind":"lifecycle","time_ns":"9007199254740993",
                                "identity":{"session_id":"rust-native-session"},
                                "payload":{"state":"completed"}})]
                } else {
                    vec![]
                };
                let next = if cursor.is_empty() { CURSOR } else { cursor };
                self.pending = Some(next.to_owned());
                Ok(Some(
                    json!({"type":"batch","protocol":1,"source_id":self.source_id,
                               "cursor":next,"records":records}),
                ))
            }
            "ack" => {
                let cursor = object
                    .get("cursor")
                    .and_then(Value::as_str)
                    .ok_or("missing ack cursor")?;
                if self.pending.as_deref() != Some(cursor) {
                    return Err("ack cursor mismatch".into());
                }
                self.pending = None;
                Ok(None)
            }
            _ => Err("unknown control message".into()),
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = BufReader::new(stdin.lock());
    let mut output = BufWriter::new(stdout.lock());
    let mut session = Session::default();
    loop {
        let Some(raw) = read_frame(&mut input, CONTROL_LIMIT)? else {
            return Ok(());
        };
        let request: Value = serde_json::from_str(&raw)?;
        if let Some(response) = session.handle(request)? {
            let encoded = serde_json::to_string(&response)?;
            write_frame(&mut output, &encoded, session.data_limit)?;
        }
        if session.stopped {
            return Ok(());
        }
    }
}

fn main() {
    if run().is_err() {
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_progress_ack_and_idle() {
        let mut session = Session::default();
        let hello = session
            .handle(json!({"type":"hello","protocol":1,
            "plugin_id":PLUGIN_ID,"source_id":"test-source",
            "max_frame_bytes":MAX_FRAME_BYTES}))
            .unwrap()
            .unwrap();
        assert_eq!(hello["features"][0], "source-adapter/1");
        let poll = |cursor| {
            json!({"type":"poll","protocol":1,"source_id":"test-source",
                                   "cursor":cursor,"max_records":256,"max_bytes":MAX_FRAME_BYTES})
        };
        let first = session.handle(poll("")).unwrap().unwrap();
        assert_eq!(first["records"].as_array().unwrap().len(), 1);
        assert_eq!(first["cursor"], CURSOR);
        assert!(session.handle(poll(CURSOR)).is_err());
        assert!(
            session
                .handle(json!({"type":"ack","protocol":1,
            "source_id":"test-source","cursor":"wrong"}))
                .is_err()
        );
        session
            .handle(json!({"type":"ack","protocol":1,
            "source_id":"test-source","cursor":CURSOR}))
            .unwrap();
        let idle = session.handle(poll(CURSOR)).unwrap().unwrap();
        assert_eq!(idle["cursor"], CURSOR);
        assert!(idle["records"].as_array().unwrap().is_empty());
    }

    #[test]
    fn identity_and_unknown_cursor_are_rejected() {
        let mut session = Session::default();
        assert!(
            session
                .handle(json!({"type":"hello","protocol":1,
            "plugin_id":"other","source_id":"test-source",
            "max_frame_bytes":MAX_FRAME_BYTES}))
                .is_err()
        );
        session
            .handle(json!({"type":"hello","protocol":1,
            "plugin_id":PLUGIN_ID,"source_id":"test-source",
            "max_frame_bytes":MAX_FRAME_BYTES}))
            .unwrap();
        assert!(
            session
                .handle(json!({"type":"poll","protocol":1,
            "source_id":"wrong","cursor":"","max_records":256,
            "max_bytes":MAX_FRAME_BYTES}))
                .is_err()
        );
        assert!(
            session
                .handle(json!({"type":"poll","protocol":1,
            "source_id":"test-source","cursor":"unknown","max_records":256,
            "max_bytes":MAX_FRAME_BYTES}))
                .is_err()
        );
    }
}
