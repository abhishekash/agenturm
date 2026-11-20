//! Format independent framed control loop for an explicitly configured source.
//! The host owns manifest admission, capture policy, durable cursor and ack.

#![forbid(unsafe_code)]

use agenturm_protocol::{MAX_FRAME_BYTES, read_frame, write_frame};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::error::Error;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;
const CONTROL_LIMIT: usize = 64 * 1024;
const MAX_RECORDS: u64 = 256;
const MAX_STRUCTURAL_TOKENS: usize = 8192;

fn structural_tokens(raw: &[u8]) -> usize {
    let (mut quoted, mut escaped, mut count) = (false, false, 0);
    for byte in raw {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else if *byte == b'"' {
            quoted = true;
            count += 1;
        } else if matches!(byte, b'{' | b'}' | b'[' | b']' | b',' | b':') {
            count += 1;
        }
    }
    count
}

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

pub trait Poller {
    const PLUGIN_ID: &'static str;
    const DISCOVERY: bool = false;
    const ROOT_GAP: bool = false;
    const REFERENCE_FEATURE: Option<&'static str> = None;
    const REFERENCE_ENVELOPE_KEY: &'static str = "content_refs";
    const EXTRA_FEATURES: &'static [&'static str] = &[];
    fn poll(
        path: &Path,
        source_id: &str,
        cursor: &str,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<Value>;

    fn poll_discovery(
        _path: &Path,
        _source_id: &str,
        _cursor: &str,
        _members: &BTreeMap<String, String>,
        _max_records: usize,
        _max_bytes: usize,
    ) -> Result<Value> {
        Err(invalid("source discovery unsupported"))
    }
}

fn discovery_members(value: Option<&Value>) -> Result<BTreeMap<String, String>> {
    let members = value
        .and_then(Value::as_object)
        .ok_or("missing discovery members")?;
    if members.len() > 48 {
        return Err(invalid("too many discovery members"));
    }
    let mut result = BTreeMap::new();
    for (id, value) in members {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid("invalid discovery member ID"));
        }
        let cursor = value.as_str().ok_or("invalid discovery member cursor")?;
        if cursor.len() > 512 || cursor.bytes().any(|b| b < 32 || b == 127) {
            return Err(invalid("invalid discovery member cursor"));
        }
        result.insert(id.to_owned(), cursor.to_owned());
    }
    Ok(result)
}

#[derive(Default)]
struct Session {
    source_id: String,
    max_frame: usize,
    pending: Option<String>,
    stopped: bool,
}

impl Session {
    fn handle<P: Poller>(&mut self, request: Value, path: &Path) -> Result<Option<(Value, usize)>> {
        let object = request
            .as_object()
            .ok_or("control frame must be an object")?;
        if object.get("protocol").and_then(Value::as_u64) != Some(1) {
            return Err(invalid("protocol mismatch"));
        }
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or("missing control type")?;
        if self.source_id.is_empty() {
            if kind != "hello"
                || object.len() != 5
                || object.get("plugin_id").and_then(Value::as_str) != Some(P::PLUGIN_ID)
            {
                return Err(invalid("hello identity mismatch"));
            }
            let source = object
                .get("source_id")
                .and_then(Value::as_str)
                .ok_or("missing source")?;
            let limit = object
                .get("max_frame_bytes")
                .and_then(Value::as_u64)
                .ok_or("missing frame budget")?;
            if !identity(source) || limit == 0 || limit > MAX_FRAME_BYTES as u64 {
                return Err(invalid("invalid hello source or budget"));
            }
            self.source_id = source.to_owned();
            self.max_frame = limit as usize;
            let mut features = vec!["source-adapter/1", "configured-root/1"];
            if P::DISCOVERY {
                features.push("source-discovery/1");
            }
            if P::ROOT_GAP {
                features.push("source-discovery-root-gap/1");
            }
            if let Some(feature) = P::REFERENCE_FEATURE {
                features.push(feature);
            }
            features.extend_from_slice(P::EXTRA_FEATURES);
            return Ok(Some((
                json!({"type":"hello_ack","protocol":1,"plugin_id":P::PLUGIN_ID,
                "features":features}),
                self.max_frame,
            )));
        }
        if kind == "stop" && object.len() == 2 {
            self.stopped = true;
            return Ok(None);
        }
        if object.get("source_id").and_then(Value::as_str) != Some(self.source_id.as_str()) {
            return Err(invalid("source mismatch"));
        }
        match kind {
            "poll" if object.len() == if P::DISCOVERY { 7 } else { 6 } => {
                if self.pending.is_some() {
                    return Err(invalid("prior poll not acknowledged"));
                }
                let cursor = object
                    .get("cursor")
                    .and_then(Value::as_str)
                    .ok_or("missing cursor")?;
                let count = object
                    .get("max_records")
                    .and_then(Value::as_u64)
                    .ok_or("missing record budget")?;
                let bytes = object
                    .get("max_bytes")
                    .and_then(Value::as_u64)
                    .ok_or("missing byte budget")?;
                if count == 0 || count > MAX_RECORDS || bytes == 0 || bytes > self.max_frame as u64
                {
                    return Err(invalid("invalid poll budget"));
                }
                let poll_budget = if P::REFERENCE_FEATURE.is_some() {
                    (bytes as usize)
                        .checked_sub(128)
                        .ok_or("content envelope frame budget too small")?
                } else {
                    bytes as usize
                };
                let mut response = if P::DISCOVERY {
                    let members = discovery_members(object.get("members"))?;
                    P::poll_discovery(
                        path,
                        &self.source_id,
                        cursor,
                        &members,
                        count as usize,
                        poll_budget,
                    )?
                } else {
                    P::poll(path, &self.source_id, cursor, count as usize, poll_budget)?
                };
                if P::REFERENCE_FEATURE.is_some()
                    && response.get(P::REFERENCE_ENVELOPE_KEY).is_none()
                {
                    response[P::REFERENCE_ENVELOPE_KEY] = json!({"schema_version":1,"refs":[]});
                }
                let encoded = serde_json::to_vec(&response)?;
                if encoded.len() > bytes as usize
                    || structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS
                {
                    return Err(invalid("poll response exceeds requested budget"));
                }
                self.pending = Some(
                    response["cursor"]
                        .as_str()
                        .ok_or("missing response cursor")?
                        .to_owned(),
                );
                Ok(Some((response, bytes as usize)))
            }
            "ack" if object.len() == 4 => {
                let cursor = object
                    .get("cursor")
                    .and_then(Value::as_str)
                    .ok_or("missing ack cursor")?;
                if self.pending.as_deref() != Some(cursor) {
                    return Err(invalid("ack mismatch"));
                }
                self.pending = None;
                Ok(None)
            }
            _ => Err(invalid("invalid control message")),
        }
    }
}

pub fn run<P: Poller>() -> Result<()> {
    let configured =
        std::env::var_os("AGENTURM_PLUGIN_SOURCE_ROOT").ok_or("configured source root required")?;
    let path = PathBuf::from(configured);
    if !path.is_absolute() {
        return Err(invalid("configured source path must be absolute"));
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut session = Session::default();
    loop {
        let Some(raw) = read_frame(&mut reader, CONTROL_LIMIT)? else {
            return Ok(());
        };
        let request: Value = serde_json::from_str(&raw)?;
        if let Some((response, limit)) = session.handle::<P>(request, &path)? {
            write_frame(&mut writer, &serde_json::to_string(&response)?, limit)?;
        }
        if session.stopped {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Discovery;

    struct Content;

    struct Command;

    impl Poller for Command {
        const PLUGIN_ID: &'static str = "fixture.command";
        const REFERENCE_FEATURE: Option<&'static str> = Some("fixture-command-ref/1");
        const REFERENCE_ENVELOPE_KEY: &'static str = "command_refs";

        fn poll(
            path: &Path,
            source_id: &str,
            cursor: &str,
            max_records: usize,
            max_bytes: usize,
        ) -> Result<Value> {
            Content::poll(path, source_id, cursor, max_records, max_bytes)
        }
    }

    impl Poller for Content {
        const PLUGIN_ID: &'static str = "fixture.content";
        const REFERENCE_FEATURE: Option<&'static str> = Some("fixture-content/1");
        const EXTRA_FEATURES: &'static [&'static str] = &["harness.fixture/1"];

        fn poll(
            _path: &Path,
            source_id: &str,
            cursor: &str,
            _max_records: usize,
            max_bytes: usize,
        ) -> Result<Value> {
            if cursor == "large" {
                return Ok(json!({"type":"batch","protocol":1,"source_id":source_id,
                          "cursor":cursor,"records":[],"padding":"x".repeat(max_bytes)}));
            }
            Ok(json!({"type":"batch","protocol":1,"source_id":source_id,
                      "cursor":cursor,"records":[]}))
        }
    }

    impl Poller for Discovery {
        const PLUGIN_ID: &'static str = "fixture.discovery";
        const DISCOVERY: bool = true;

        fn poll(
            _path: &Path,
            _source_id: &str,
            _cursor: &str,
            _max_records: usize,
            _max_bytes: usize,
        ) -> Result<Value> {
            Err(invalid("single-file poll forbidden"))
        }

        fn poll_discovery(
            _path: &Path,
            source_id: &str,
            cursor: &str,
            _members: &BTreeMap<String, String>,
            _max_records: usize,
            _max_bytes: usize,
        ) -> Result<Value> {
            Ok(json!({"type":"batch","protocol":1,"source_id":source_id,
                      "cursor":cursor,"records":[]}))
        }
    }

    #[test]
    fn discovery_handshake_and_member_budget_are_explicit() {
        let mut session = Session::default();
        let hello = session
            .handle::<Discovery>(
                json!({"type":"hello","protocol":1,
            "plugin_id":"fixture.discovery","source_id":"source",
            "max_frame_bytes":65536}),
                Path::new("/tmp"),
            )
            .unwrap()
            .unwrap()
            .0;
        assert!(
            hello["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "source-discovery/1")
        );
        let members: BTreeMap<_, _> = (0..49)
            .map(|n| (format!("{n:064x}"), "cursor".to_owned()))
            .collect();
        assert!(
            session
                .handle::<Discovery>(
                    json!({"type":"poll","protocol":1,
            "source_id":"source","cursor":"","members":members,
            "max_records":256,"max_bytes":65536}),
                    Path::new("/tmp")
                )
                .is_err()
        );
        assert!(session.pending.is_none());
        let idle = session
            .handle::<Discovery>(
                json!({"type":"poll","protocol":1,
            "source_id":"source","cursor":"","members":{},
            "max_records":256,"max_bytes":65536}),
                Path::new("/tmp"),
            )
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(idle["records"], json!([]));
    }

    #[test]
    fn declared_content_feature_and_final_envelope_obey_exact_budget() {
        let mut session = Session::default();
        let hello = session
            .handle::<Content>(
                json!({"type":"hello","protocol":1,"plugin_id":"fixture.content",
                "source_id":"source","max_frame_bytes":65536}),
                Path::new("/tmp"),
            )
            .unwrap()
            .unwrap()
            .0;
        assert!(
            hello["features"]
                .as_array()
                .unwrap()
                .contains(&json!("fixture-content/1"))
        );
        assert!(
            hello["features"]
                .as_array()
                .unwrap()
                .contains(&json!("harness.fixture/1"))
        );
        let request = |max_bytes| {
            json!({"type":"poll","protocol":1,
            "source_id":"source","cursor":"","max_records":1,"max_bytes":max_bytes})
        };
        let oversized = json!({"type":"poll","protocol":1,
            "source_id":"source","cursor":"large","max_records":1,"max_bytes":256});
        assert!(
            session
                .handle::<Content>(oversized, Path::new("/tmp"))
                .is_err()
        );
        assert!(session.pending.is_none());
        let response = session
            .handle::<Content>(request(256), Path::new("/tmp"))
            .unwrap()
            .unwrap()
            .0;
        assert!(serde_json::to_vec(&response).unwrap().len() <= 256);
        assert_eq!(
            response["content_refs"],
            json!({"schema_version":1,"refs":[]})
        );
    }

    #[test]
    fn command_feature_fills_only_its_selected_envelope() {
        let mut session = Session::default();
        session
            .handle::<Command>(
                json!({"type":"hello","protocol":1,
            "plugin_id":"fixture.command","source_id":"source",
            "max_frame_bytes":65536}),
                Path::new("/tmp"),
            )
            .unwrap();
        let response = session
            .handle::<Command>(
                json!({"type":"poll","protocol":1,
            "source_id":"source","cursor":"","max_records":1,"max_bytes":256}),
                Path::new("/tmp"),
            )
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(
            response["command_refs"],
            json!({"schema_version":1,"refs":[]})
        );
        assert!(response.get("content_refs").is_none());
    }
}
