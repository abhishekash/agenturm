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
                let response = if P::DISCOVERY {
                    let members = discovery_members(object.get("members"))?;
                    P::poll_discovery(
                        path,
                        &self.source_id,
                        cursor,
                        &members,
                        count as usize,
                        bytes as usize,
                    )?
                } else {
                    P::poll(
                        path,
                        &self.source_id,
                        cursor,
                        count as usize,
                        bytes as usize,
                    )?
                };
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
}
