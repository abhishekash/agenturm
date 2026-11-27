//! Opt-in, metadata-only loopback connector over the shared source process contract.

#![forbid(unsafe_code)]

use agenturm_source_process::{Poller, Result, run};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const PLUGIN_ID: &str = "example.rust-loopback-inbound";
const MAX_HTTP_BODY: usize = 1024 * 1024;
const MAX_HTTP_HEADERS: usize = 16 * 1024;
const MAX_RECORD_BYTES: usize = 16 * 1024;
const MAX_RECORDS: usize = 256;
const MAX_STRUCTURAL_TOKENS: usize = 8192;
const MAX_CURSOR: usize = 512;
const MAX_CONFIG: usize = 4096;
const MAX_REVISION: u32 = i32::MAX as u32;
const MAX_TIME: u64 = i64::MAX as u64;
const USAGE_KEYS: [&str; 5] = [
    "cache_read_tokens",
    "cache_write_tokens",
    "input_tokens",
    "output_tokens",
    "total_tokens",
];
const STATES: [&str; 7] = [
    "cancelled",
    "completed",
    "failed",
    "interrupted",
    "paused",
    "running",
    "started",
];

static RETRY_UNTIL: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectorConfig {
    endpoint: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemotePage {
    records: Vec<RemoteRecord>,
    next: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteRecord {
    id: String,
    session: String,
    at_ns: String,
    kind: String,
    #[serde(default)]
    revision: Field<u32>,
    #[serde(default)]
    state: Field<String>,
    #[serde(default)]
    reason_code: Field<String>,
    #[serde(default)]
    usage: Field<ExactUsage>,
}

#[derive(Default)]
enum Field<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FieldVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for FieldVisitor<T> {
            type Value = Field<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a non-null value")
            }

            fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Field::Null)
            }

            fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Field::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                T::deserialize(deserializer).map(Field::Value)
            }
        }
        deserializer.deserialize_option(FieldVisitor(PhantomData))
    }
}

#[derive(Debug, Default)]
struct ExactUsage(BTreeMap<String, u64>);

impl<'de> Deserialize<'de> for ExactUsage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UsageVisitor;
        impl<'de> Visitor<'de> for UsageVisitor {
            type Value = ExactUsage;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded token usage object")
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, u64>()? {
                    if !USAGE_KEYS.contains(&key.as_str()) || values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("unknown or duplicate usage field"));
                    }
                }
                Ok(ExactUsage(values))
            }
        }
        deserializer.deserialize_map(UsageVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedCursor {
    token: String,
    last_ns: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HttpResult {
    Page,
    RateLimited(u64),
    Expired,
}

fn valid_token(token: &str) -> bool {
    token.len() <= 200
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn endpoint_port(value: &str) -> Result<u16> {
    if value.len() > 300 || !value.starts_with("http://127.0.0.1:") {
        return Err(invalid("connector endpoint must be exact loopback HTTP"));
    }
    let authority = value
        .strip_prefix("http://")
        .and_then(|rest| rest.strip_suffix("/events"))
        .ok_or_else(|| invalid("connector endpoint path must be /events"))?;
    if authority.contains(['/', '?', '#', '@']) {
        return Err(invalid(
            "connector endpoint contains unsupported URL components",
        ));
    }
    let (host, port) = authority
        .split_once(':')
        .ok_or_else(|| invalid("connector endpoint port required"))?;
    if host != "127.0.0.1"
        || port.is_empty()
        || (port.len() > 1 && port.starts_with('0'))
        || !port.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid("connector endpoint must be exact loopback HTTP"));
    }
    let parsed: u16 = port.parse().map_err(|_| "connector port out of range")?;
    if parsed == 0 {
        return Err(invalid("connector port out of range"));
    }
    Ok(parsed)
}

fn read_config(path: &Path) -> Result<ConnectorConfig> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_CONFIG as u64
    {
        return Err(invalid("invalid connector configuration file"));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let file_meta = file.metadata()?;
    if !file_meta.is_file() || file_meta.len() > MAX_CONFIG as u64 {
        return Err(invalid("invalid connector configuration file"));
    }
    let mut raw = Vec::with_capacity(file_meta.len() as usize);
    file.take((MAX_CONFIG + 1) as u64).read_to_end(&mut raw)?;
    if raw.len() > MAX_CONFIG {
        return Err(invalid("connector configuration exceeds byte limit"));
    }
    Ok(serde_json::from_slice(&raw)?)
}

fn canonical_decimal(value: &str, max: u64) -> Result<u64> {
    if value.is_empty()
        || value.len() > 19
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(invalid("invalid decimal source value"));
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| "invalid decimal source value")?;
    if parsed > max {
        return Err(invalid("decimal source value out of range"));
    }
    Ok(parsed)
}

fn parse_cursor(cursor: &str) -> Result<ParsedCursor> {
    if cursor.len() > MAX_CURSOR {
        return Err(invalid("resume cursor too large"));
    }
    if cursor.is_empty() || is_gap(cursor) {
        return Ok(ParsedCursor {
            token: String::new(),
            last_ns: "0".to_owned(),
        });
    }
    let value = cursor
        .strip_prefix("v1:")
        .ok_or_else(|| invalid("invalid resume cursor"))?;
    let (token, last_ns) = value
        .rsplit_once(':')
        .ok_or_else(|| invalid("invalid resume cursor"))?;
    if token != "~" && (token.is_empty() || !valid_token(token)) {
        return Err(invalid("invalid resume cursor"));
    }
    let timestamp = canonical_decimal(last_ns, MAX_TIME)?;
    Ok(ParsedCursor {
        token: if token == "~" {
            String::new()
        } else {
            token.to_owned()
        },
        last_ns: timestamp.to_string(),
    })
}

fn is_gap(cursor: &str) -> bool {
    cursor.strip_prefix("gap:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

fn source_record(row: RemoteRecord) -> Result<Value> {
    if !valid_identity(&row.id) || !valid_identity(&row.session) {
        return Err(invalid("invalid source identity"));
    }
    let timestamp = canonical_decimal(&row.at_ns, MAX_TIME)?;
    let revision = match row.revision {
        Field::Missing => 0,
        Field::Value(value) => value,
        Field::Null => return Err(invalid("invalid source revision")),
    };
    if revision > MAX_REVISION {
        return Err(invalid("source revision out of range"));
    }
    let payload = match row.kind.as_str() {
        "lifecycle" if matches!(&row.usage, Field::Missing) => {
            let state = match &row.state {
                Field::Value(state) => state.as_str(),
                _ => return Err(invalid("missing lifecycle state")),
            };
            let reason = match &row.reason_code {
                Field::Missing => None,
                Field::Value(code) => Some(code.as_str()),
                Field::Null => return Err(invalid("invalid source reason code")),
            };
            if !STATES.contains(&state) || reason.is_some_and(|code| !valid_code(code)) {
                return Err(invalid("invalid lifecycle metadata"));
            }
            let mut payload = serde_json::Map::new();
            payload.insert("state".to_owned(), json!(state));
            if let Some(reason) = reason {
                payload.insert("reason_code".to_owned(), json!(reason));
            }
            Value::Object(payload)
        }
        "usage"
            if matches!(&row.state, Field::Missing)
                && matches!(&row.reason_code, Field::Missing) =>
        {
            let usage = match row.usage {
                Field::Value(usage) => usage.0,
                _ => return Err(invalid("missing usage fields")),
            };
            if usage.is_empty() || usage.values().any(|value| *value > MAX_TIME) {
                return Err(invalid("invalid usage metadata"));
            }
            json!(usage)
        }
        _ => return Err(invalid("unsupported source record kind or fields")),
    };
    let record = json!({
        "event_id": format!("inbound:{}", row.id),
        "revision": revision,
        "kind": row.kind,
        "time_ns": timestamp.to_string(),
        "identity": {"session_id": row.session},
        "payload": payload
    });
    if serde_json::to_vec(&record)?.len() > MAX_RECORD_BYTES {
        return Err(invalid("source record exceeds byte limit"));
    }
    Ok(record)
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn valid_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'.' | b'/' | b'-')
        })
}

fn parse_retry_after(raw: Option<&str>) -> u64 {
    raw.and_then(|value| {
        (!value.is_empty() && value.len() <= 3 && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.parse::<u64>().ok())
            .flatten()
    })
    .unwrap_or(1)
    .clamp(1, 60)
}

fn get_http(endpoint: &str, token: &str) -> Result<(HttpResult, Option<RemotePage>)> {
    let port = endpoint_port(endpoint)?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let request = format!(
        "GET /events?after={token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;

    let limit = MAX_HTTP_HEADERS + MAX_HTTP_BODY;
    let mut response = Vec::with_capacity(4096);
    let mut buffer = [0u8; 8192];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if response.len() + count > limit {
            return Err(invalid("connector response exceeds byte limit"));
        }
        response.extend_from_slice(&buffer[..count]);
    }
    parse_http_response(&response)
}

fn parse_http_response(response: &[u8]) -> Result<(HttpResult, Option<RemotePage>)> {
    if response.len() > MAX_HTTP_HEADERS + MAX_HTTP_BODY {
        return Err(invalid("connector response exceeds byte limit"));
    }
    let (header, body) = split_http(response)?;
    let header_text = std::str::from_utf8(header)?;
    if header.len() > MAX_HTTP_HEADERS {
        return Err(invalid("connector HTTP headers exceed byte limit"));
    }
    let mut lines = header_text.split("\r\n");
    let status_line = lines.next().ok_or("missing connector HTTP status")?;
    let mut status_parts = status_line.split_ascii_whitespace();
    let version = status_parts.next().ok_or("invalid connector HTTP status")?;
    let code = status_parts
        .next()
        .ok_or("invalid connector HTTP status")?
        .parse::<u16>()
        .map_err(|_| "invalid connector HTTP status")?;
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || status_parts.next().is_none() {
        return Err(invalid("invalid connector HTTP status"));
    }
    let mut content_length = None;
    let mut retry_after = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or("malformed connector HTTP header")?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(invalid("invalid connector Content-Length"));
            }
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "invalid Content-Length")?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(invalid("chunked connector responses are unsupported"));
        } else if name.eq_ignore_ascii_case("retry-after") {
            if retry_after.is_some() {
                return Err(invalid("duplicate Retry-After"));
            }
            retry_after = Some(value.to_owned());
        }
    }
    if body.len() > MAX_HTTP_BODY {
        return Err(invalid("connector response body exceeds byte limit"));
    }
    if let Some(expected) = content_length {
        if expected != body.len() {
            return Err(invalid("connector Content-Length mismatch"));
        }
    } else if !body.is_empty() {
        return Err(invalid("connector response lacks Content-Length"));
    }
    match code {
        200 => {
            if content_length.is_none() || body.is_empty() {
                return Err(invalid("connector page lacks a bounded response body"));
            }
            if structural_tokens(body) > MAX_STRUCTURAL_TOKENS {
                return Err(invalid("connector page exceeds structural token limit"));
            }
            let page = serde_json::from_slice(body)?;
            Ok((HttpResult::Page, Some(page)))
        }
        410 => Ok((HttpResult::Expired, None)),
        429 => Ok((
            HttpResult::RateLimited(parse_retry_after(retry_after.as_deref())),
            None,
        )),
        _ => Err(invalid("connector HTTP status rejected")),
    }
}

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
        } else if matches!(*byte, b'{' | b'}' | b'[' | b']' | b',' | b':') {
            count += 1;
        }
    }
    count
}

fn split_http(response: &[u8]) -> Result<(&[u8], &[u8])> {
    response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (&response[..index], &response[index + 4..]))
        .ok_or_else(|| invalid("malformed connector HTTP response"))
}

fn empty_batch(source_id: &str, cursor: &str) -> Value {
    json!({"type":"batch","protocol":1,"source_id":source_id,
          "cursor":cursor,"records":[]})
}

fn expired_batch(source_id: &str, cursor: &str, last_ns: &str) -> Result<Value> {
    if cursor.is_empty() || is_gap(cursor) {
        return Err(invalid("connector cannot recover a fresh or gap cursor"));
    }
    let digest = format!("{:x}", Sha256::digest(cursor.as_bytes()));
    Ok(json!({"type":"batch","protocol":1,"source_id":source_id,
        "cursor":format!("gap:{digest}"),
        "records":[{"event_id":format!("inbound-gap:{digest}"),"revision":0,
            "kind":"diagnostic","time_ns":last_ns,
            "payload":{"code":"cursor_expired","severity":"warning"}}]}))
}

fn set_retry(source_id: &str, seconds: u64) {
    let retry = RETRY_UNTIL.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut retries) = retry.lock() {
        retries.insert(
            source_id.to_owned(),
            Instant::now() + Duration::from_secs(seconds),
        );
    }
}

fn retry_active(source_id: &str) -> bool {
    let Some(retry) = RETRY_UNTIL.get() else {
        return false;
    };
    let Ok(mut retries) = retry.lock() else {
        return true;
    };
    if let Some(deadline) = retries.get(source_id).copied() {
        if Instant::now() < deadline {
            return true;
        }
        retries.remove(source_id);
    }
    false
}

struct LoopbackConnector;

impl Poller for LoopbackConnector {
    const PLUGIN_ID: &'static str = PLUGIN_ID;
    const EXTRA_FEATURES: &'static [&'static str] = &["connector.inbound/1", "connector-adapter/1"];

    fn poll(
        path: &Path,
        source_id: &str,
        cursor: &str,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<Value> {
        if max_records == 0 || max_records > MAX_RECORDS || max_bytes == 0 {
            return Err(invalid("invalid host poll budget"));
        }
        let parsed_cursor = parse_cursor(cursor)?;
        let config = read_config(path)?;
        endpoint_port(&config.endpoint)?;
        if retry_active(source_id) {
            return Ok(empty_batch(source_id, cursor));
        }
        let (status, page) = get_http(&config.endpoint, &parsed_cursor.token)?;
        match status {
            HttpResult::RateLimited(seconds) => {
                set_retry(source_id, seconds);
                Ok(empty_batch(source_id, cursor))
            }
            HttpResult::Expired => expired_batch(source_id, cursor, &parsed_cursor.last_ns),
            HttpResult::Page => {
                let page = page.ok_or("missing connector page")?;
                if page.records.len() > max_records || !valid_token(&page.next) {
                    return Err(invalid("invalid connector page budget or cursor"));
                }
                if !page.records.is_empty() && page.next == parsed_cursor.token {
                    return Err(invalid("connector failed to advance its page token"));
                }
                let records = page
                    .records
                    .into_iter()
                    .map(source_record)
                    .collect::<Result<Vec<_>>>()?;
                let mut newest = parsed_cursor.last_ns.parse::<u64>()?;
                for record in &records {
                    newest = newest.max(
                        record["time_ns"]
                            .as_str()
                            .ok_or("bad projected time")?
                            .parse()?,
                    );
                }
                let next_cursor = if records.is_empty() && page.next == parsed_cursor.token {
                    cursor.to_owned()
                } else {
                    format!(
                        "v1:{}:{newest}",
                        if page.next.is_empty() {
                            "~"
                        } else {
                            &page.next
                        }
                    )
                };
                let batch = json!({"type":"batch","protocol":1,"source_id":source_id,
                    "cursor":next_cursor,"records":records});
                if serde_json::to_vec(&batch)?.len() > max_bytes {
                    return Err(invalid("connector page exceeds host byte budget"));
                }
                Ok(batch)
            }
        }
    }
}

fn main() {
    if run::<LoopbackConnector>().is_err() {
        // Keep endpoint, cursor and source payload out of diagnostics.
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
        let mut raw = format!("HTTP/1.1 {status}\r\n").into_bytes();
        for (name, value) in headers {
            raw.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        raw.extend_from_slice(b"\r\n");
        raw.extend_from_slice(body);
        raw
    }

    #[test]
    fn endpoint_accepts_only_exact_literal_loopback_path() {
        assert_eq!(endpoint_port("http://127.0.0.1:8123/events").unwrap(), 8123);
        for value in [
            "https://127.0.0.1:8123/events",
            "http://localhost:8123/events",
            "http://127.0.0.2:8123/events",
            "http://127.0.0.1:0/events",
            "http://127.0.0.1:08123/events",
            "http://127.0.0.1:8123/events?after=x",
            "http://127.0.0.1:8123/events#frag",
            "http://user@127.0.0.1:8123/events",
            "http://127.0.0.1:8123/other",
        ] {
            assert!(endpoint_port(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn cursors_preserve_empty_token_and_reject_malformed_values() {
        assert_eq!(
            parse_cursor("").unwrap(),
            ParsedCursor {
                token: "".into(),
                last_ns: "0".into()
            }
        );
        assert_eq!(
            parse_cursor("v1:~:0").unwrap(),
            ParsedCursor {
                token: "".into(),
                last_ns: "0".into()
            }
        );
        assert_eq!(
            parse_cursor("v1:r1:12").unwrap(),
            ParsedCursor {
                token: "r1".into(),
                last_ns: "12".into()
            }
        );
        assert_eq!(parse_cursor("v1:a:b:12").unwrap().token, "a:b");
        assert_eq!(
            parse_cursor(&format!("gap:{}", "a".repeat(64)))
                .unwrap()
                .last_ns,
            "0"
        );
        for cursor in ["v1:-:01", "v1:bad token:1", "bad", "gap:xyz"] {
            assert!(parse_cursor(cursor).is_err(), "accepted {cursor}");
        }
    }

    #[test]
    fn records_are_metadata_only_and_strict_about_fields_and_types() {
        let lifecycle: RemoteRecord = serde_json::from_str(
            r#"{"id":"x","session":"s","at_ns":"4","kind":"lifecycle","state":"completed","revision":1}"#,
        ).unwrap();
        let projected = source_record(lifecycle).unwrap();
        assert_eq!(projected["event_id"], "inbound:x");
        assert_eq!(projected["payload"], json!({"state":"completed"}));

        for raw in [
            r#"{"id":"x","session":"s","at_ns":"4","kind":"lifecycle","state":"completed","prompt":"private"}"#,
            r#"{"id":"x","session":"s","at_ns":"4","kind":"lifecycle","state":"completed","state":"failed"}"#,
            r#"{"id":"x","session":"s","at_ns":"04","kind":"lifecycle","state":"completed"}"#,
            r#"{"id":"x","session":"s","at_ns":"4","kind":"lifecycle","state":"completed","revision":null}"#,
            r#"{"id":"x","session":"s","at_ns":"4","kind":"lifecycle","state":"completed","reason_code":null}"#,
            r#"{"id":"x","session":"s","at_ns":"4","kind":"lifecycle","state":"completed","usage":null}"#,
            r#"{"id":"x","session":"s","at_ns":"4","kind":"usage","state":null,"usage":{"input_tokens":1}}"#,
            r#"{"id":"x","session":"s","at_ns":"4","kind":"usage","usage":{"input_tokens":1,"input_tokens":2}}"#,
            r#"{"id":"x","session":"s","at_ns":"4","kind":"usage","usage":{"authorization":1}}"#,
            r#"{"id":"x","session":"s","at_ns":"4","kind":"usage","usage":{"input_tokens":true}}"#,
        ] {
            let result: std::result::Result<RemoteRecord, _> = serde_json::from_str(raw);
            if let Ok(record) = result {
                assert!(
                    source_record(record).is_err(),
                    "accepted malformed record {raw}"
                );
            } else {
                assert!(result.is_err(), "accepted malformed record {raw}");
            }
        }
        assert!(
            serde_json::from_str::<RemotePage>(r#"{"records":[],"records":[],"next":""}"#).is_err()
        );
    }

    #[test]
    fn retry_after_is_bounded_and_expired_cursor_yields_one_gap_batch() {
        assert_eq!(parse_retry_after(Some("0")), 1);
        assert_eq!(parse_retry_after(Some("3")), 3);
        assert_eq!(parse_retry_after(Some("999")), 60);
        assert_eq!(parse_retry_after(Some("junk")), 1);
        let batch = expired_batch("source", "v1:revision:9", "9").unwrap();
        let cursor = batch["cursor"].as_str().unwrap();
        assert!(is_gap(cursor));
        assert_eq!(
            batch["records"][0]["payload"],
            json!({"code":"cursor_expired","severity":"warning"})
        );
        assert!(expired_batch("source", "", "0").is_err());
        assert!(expired_batch("source", cursor, "0").is_err());
    }

    #[test]
    fn http_parser_rejects_bad_lengths_chunking_and_response_budgets() {
        let body = br#"{"records":[],"next":""}"#;
        let valid = response(
            "200 OK",
            &[("Content-Length", body.len().to_string())],
            body,
        );
        assert_eq!(parse_http_response(&valid).unwrap().0, HttpResult::Page);

        let wrong = response(
            "200 OK",
            &[("Content-Length", (body.len() + 1).to_string())],
            body,
        );
        assert!(parse_http_response(&wrong).is_err());
        let duplicate = response(
            "200 OK",
            &[
                ("Content-Length", body.len().to_string()),
                ("Content-Length", body.len().to_string()),
            ],
            body,
        );
        assert!(parse_http_response(&duplicate).is_err());
        let chunked = response(
            "200 OK",
            &[("Transfer-Encoding", "chunked".to_owned())],
            body,
        );
        assert!(parse_http_response(&chunked).is_err());

        let overlong_headers = response(
            "200 OK",
            &[
                ("Content-Length", body.len().to_string()),
                ("X-Fill", "x".repeat(MAX_HTTP_HEADERS)),
            ],
            body,
        );
        assert!(parse_http_response(&overlong_headers).is_err());
        let overlong_body = vec![b' '; MAX_HTTP_BODY + 1];
        let overlong_body_response = response(
            "200 OK",
            &[("Content-Length", overlong_body.len().to_string())],
            &overlong_body,
        );
        assert!(parse_http_response(&overlong_body_response).is_err());

        let mut token_flood = String::from("{\"records\":[],\"next\":\"\"");
        for n in 0..1800 {
            token_flood.push_str(&format!(",\"x{n}\":{{}}"));
        }
        token_flood.push('}');
        assert!(structural_tokens(token_flood.as_bytes()) > MAX_STRUCTURAL_TOKENS);
        let token_response = response(
            "200 OK",
            &[("Content-Length", token_flood.len().to_string())],
            token_flood.as_bytes(),
        );
        assert!(parse_http_response(&token_response).is_err());
    }

    #[test]
    fn http_parser_returns_bounded_429_and_410_statuses() {
        let limited = response(
            "429 Too Many Requests",
            &[
                ("Retry-After", "999".to_owned()),
                ("Content-Length", "0".to_owned()),
            ],
            b"",
        );
        assert_eq!(
            parse_http_response(&limited).unwrap().0,
            HttpResult::RateLimited(60)
        );
        let expired = response("410 Gone", &[], b"");
        assert_eq!(
            parse_http_response(&expired).unwrap().0,
            HttpResult::Expired
        );
    }
}
