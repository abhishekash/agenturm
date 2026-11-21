//! Supervised process adapters for the Agenturm kernel.
//!
//! The first supported wire subset is metadata-only Codex usage. The explicit
//! usage projection counts and skips validated other fact kinds; malformed or
//! unsupported records fail closed. Cursor commit and ACK ordering live in the
//! kernel, outside this process-only crate.

#![forbid(unsafe_op_in_unsafe_fn)]

use agenturm_kernel::{
    ObservationIdentity, ObservationKind, PreparedSourceBatch, RawObservation, SourceAdapter,
};
use agenturm_protocol::{read_frame, write_frame};
use serde::Deserialize;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const PROTOCOL_VERSION: u64 = 1;
const CONTROL_FRAME_LIMIT: usize = 64 * 1024;
const DATA_FRAME_LIMIT: usize = 64 * 1024;
const MAX_SOURCE_CURSOR_BYTES: usize = 800;
const MAX_POLL_RECORDS: u16 = 256;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const CODEX_PLUGIN_ID: &str = "example.rust-codex-harness";

pub type Result<T> = std::result::Result<T, RuntimeError>;

#[derive(Debug)]
pub struct RuntimeError(String);

impl RuntimeError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for RuntimeError {}

impl From<agenturm_protocol::FrameError> for RuntimeError {
    fn from(error: agenturm_protocol::FrameError) -> Self {
        Self::new(format!("source transport framing failed: {error}"))
    }
}

impl From<serde_json::Error> for RuntimeError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(format!("source batch rejected: {error}"))
    }
}

/// Configures a bundled Rust adapter launched through the Rust plugin host.
#[derive(Clone, Debug)]
pub struct ProcessConfig {
    pub plugin_host: PathBuf,
    pub adapter: PathBuf,
    pub source_root: PathBuf,
    pub source_id: String,
    pub plugin_id: String,
    pub timeout: Duration,
}

impl ProcessConfig {
    /// Configuration for the bundled Codex adapter.
    pub fn codex(
        plugin_host: impl Into<PathBuf>,
        adapter: impl Into<PathBuf>,
        source_root: impl Into<PathBuf>,
        source_id: impl Into<String>,
    ) -> Self {
        Self {
            plugin_host: plugin_host.into(),
            adapter: adapter.into(),
            source_root: source_root.into(),
            source_id: source_id.into(),
            plugin_id: CODEX_PLUGIN_ID.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// Long-lived parent connection to `agenturm-plugin-host --child <adapter>`.
pub struct ProcessSource {
    config: ProcessConfig,
    child: Child,
    child_stdin: ChildStdin,
    replies: Receiver<std::result::Result<Option<String>, String>>,
    pending_cursor: Option<String>,
    pending_from_cursor: Option<String>,
    pending_batch: Option<PreparedSourceBatch>,
}

impl ProcessSource {
    pub fn spawn(config: ProcessConfig) -> Result<Self> {
        validate_process_config(&config)?;
        #[cfg(not(unix))]
        return Err(RuntimeError::new(
            "supervised harness ingestion currently requires Unix process groups",
        ));

        let mut command = Command::new(&config.plugin_host);
        command
            .arg("--child")
            .arg(&config.adapter)
            .env("AGENTURM_PLUGIN_SOURCE_ROOT", &config.source_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_process_group(&mut command);
        let mut child_guard =
            SpawnedChild(Some(command.spawn().map_err(|_| {
                RuntimeError::new("could not start supervised plugin host")
            })?));
        let child = child_guard
            .0
            .as_mut()
            .expect("spawned child guard is populated");
        let child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::new("plugin host control pipe unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::new("plugin host data pipe unavailable"))?;
        let (sender, replies) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("agenturm-source-frame-reader".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let result = read_frame(&mut reader, DATA_FRAME_LIMIT)
                        .map_err(|error| error.to_string());
                    let done = !matches!(&result, Ok(Some(_)));
                    if sender.send(result).is_err() || done {
                        break;
                    }
                }
            })
            .map_err(|_| RuntimeError::new("could not start source frame reader"))?;
        let child = child_guard
            .0
            .take()
            .expect("spawned child guard is populated");
        let mut source = Self {
            config,
            child,
            child_stdin,
            replies,
            pending_cursor: None,
            pending_from_cursor: None,
            pending_batch: None,
        };
        source.hello()?;
        Ok(source)
    }

    fn hello(&mut self) -> Result<()> {
        let hello = serde_json::json!({
            "type": "hello",
            "protocol": PROTOCOL_VERSION,
            "plugin_id": self.config.plugin_id,
            "source_id": self.config.source_id,
            "max_frame_bytes": DATA_FRAME_LIMIT,
        });
        self.write_request(&hello)?;
        let reply = self.read_reply()?;
        let ack: HelloAck = serde_json::from_str(&reply)?;
        if ack.kind != "hello_ack"
            || ack.protocol != PROTOCOL_VERSION
            || ack.plugin_id != self.config.plugin_id
            || ack.features.len() != 2
            || !ack
                .features
                .iter()
                .any(|feature| feature == "source-adapter/1")
            || !ack
                .features
                .iter()
                .any(|feature| feature == "configured-root/1")
        {
            return Err(RuntimeError::new("source adapter handshake mismatch"));
        }
        Ok(())
    }

    fn write_request(&mut self, value: &serde_json::Value) -> Result<()> {
        let encoded = serde_json::to_string(value)?;
        write_frame(&mut self.child_stdin, &encoded, CONTROL_FRAME_LIMIT)?;
        Ok(())
    }

    fn read_reply(&mut self) -> Result<String> {
        match self.replies.recv_timeout(self.config.timeout) {
            Ok(Ok(Some(frame))) => Ok(frame),
            Ok(Ok(None)) => Err(RuntimeError::new("plugin host closed its data pipe")),
            Ok(Err(_)) => Err(RuntimeError::new("plugin host returned an invalid frame")),
            Err(RecvTimeoutError::Timeout) => {
                self.terminate();
                Err(RuntimeError::new(
                    "source adapter exceeded its response deadline",
                ))
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err(RuntimeError::new("plugin host frame reader stopped"))
            }
        }
    }
}

impl SourceAdapter for ProcessSource {
    fn source_id(&self) -> &str {
        &self.config.source_id
    }

    fn poll(&mut self, cursor: &str) -> std::result::Result<PreparedSourceBatch, String> {
        if let Some(batch) = &self.pending_batch {
            if self.pending_from_cursor.as_deref() == Some(cursor) {
                return Ok(batch.clone());
            }
            let reset = self.reset();
            return Err(match reset {
                Ok(()) => "previous source batch belonged to a different durable cursor".into(),
                Err(error) => format!(
                    "previous source batch belonged to a different durable cursor; source reset failed: {error}"
                ),
            });
        }
        validate_cursor(cursor).map_err(|error| error.to_string())?;
        let result = (|| {
            if self.pending_cursor.is_some() {
                return Err(RuntimeError::new(
                    "previous source batch must be ACKed first",
                ));
            }
            let request = serde_json::json!({
                "type": "poll",
                "protocol": PROTOCOL_VERSION,
                "source_id": self.config.source_id,
                "cursor": cursor,
                "max_records": MAX_POLL_RECORDS,
                "max_bytes": DATA_FRAME_LIMIT,
            });
            self.write_request(&request)?;
            let reply = self.read_reply()?;
            let batch = parse_codex_batch(&reply, &self.config.source_id)?;
            Ok(PreparedSourceBatch {
                next_cursor: batch.cursor,
                observations: batch.observations,
                skipped_non_usage_facts: batch.skipped_non_usage_facts,
            })
        })();
        match result {
            Ok(batch) => {
                self.pending_cursor = Some(batch.next_cursor.clone());
                self.pending_from_cursor = Some(cursor.to_owned());
                self.pending_batch = Some(batch.clone());
                Ok(batch)
            }
            Err(error) => {
                let reset = self.reset();
                Err(match reset {
                    Ok(()) => error.to_string(),
                    Err(reset_error) => format!("{error}; source reset failed: {reset_error}"),
                })
            }
        }
    }

    fn ack(&mut self, cursor: &str) -> std::result::Result<(), String> {
        (|| {
            if self.pending_cursor.as_deref() != Some(cursor) {
                return Err(RuntimeError::new("source ACK cursor mismatch"));
            }
            let request = serde_json::json!({
                "type": "ack",
                "protocol": PROTOCOL_VERSION,
                "source_id": self.config.source_id,
                "cursor": cursor,
            });
            self.write_request(&request)?;
            self.pending_cursor = None;
            self.pending_from_cursor = None;
            self.pending_batch = None;
            Ok(())
        })()
        .map_err(|error: RuntimeError| error.to_string())
    }

    fn reset(&mut self) -> std::result::Result<(), String> {
        let config = self.config.clone();
        self.terminate();
        let replacement = Self::spawn(config).map_err(|error| error.to_string())?;
        *self = replacement;
        Ok(())
    }
}

impl Drop for ProcessSource {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl ProcessSource {
    fn terminate(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            let _ = self.child.wait();
            return;
        }
        #[cfg(unix)]
        kill_process_group(self.child.id());
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }

    fn shutdown(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            let _ = self.child.wait();
            return;
        }
        let stop = serde_json::json!({"type":"stop","protocol":PROTOCOL_VERSION});
        let _ = self.write_request(&stop);
        let deadline = Instant::now() + Duration::from_millis(750);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                let _ = self.child.wait();
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.terminate();
    }
}

/// Owns a newly spawned child until every fallible setup step has completed.
/// Dropping it on a setup error prevents an orphan host process.
struct SpawnedChild(Option<Child>);

impl Drop for SpawnedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            #[cfg(unix)]
            kill_process_group(child.id());
            #[cfg(not(unix))]
            {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

fn validate_process_config(config: &ProcessConfig) -> Result<()> {
    for (path, label) in [
        (&config.plugin_host, "plugin host"),
        (&config.adapter, "adapter"),
        (&config.source_root, "source root"),
    ] {
        if !path.is_absolute() {
            return Err(RuntimeError::new(format!("{label} path must be absolute")));
        }
    }
    if !config.plugin_host.is_file() || !config.adapter.is_file() {
        return Err(RuntimeError::new(
            "plugin host or adapter executable is missing",
        ));
    }
    if config.source_id.is_empty()
        || config.source_id.len() > 96
        || !config.source_id.as_bytes()[0].is_ascii_alphanumeric()
        || !config
            .source_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
    {
        return Err(RuntimeError::new("source ID is invalid"));
    }
    if config.plugin_id.is_empty() || config.plugin_id.len() > 96 {
        return Err(RuntimeError::new("plugin ID is invalid"));
    }
    if config.timeout.is_zero() {
        return Err(RuntimeError::new("source timeout must be positive"));
    }
    Ok(())
}

fn validate_cursor(cursor: &str) -> Result<()> {
    if cursor.len() > MAX_SOURCE_CURSOR_BYTES || cursor.chars().any(char::is_control) {
        return Err(RuntimeError::new("source cursor is invalid or too long"));
    }
    Ok(())
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn kill_process_group(process_id: u32) {
    // SAFETY: `kill` receives a negative child PID to target its process group;
    // it dereferences no pointers and has no additional preconditions.
    unsafe {
        libc::kill(-(process_id as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_process_id: u32) {}

fn parse_codex_batch(encoded: &str, expected_source_id: &str) -> Result<ParsedCodexBatch> {
    let batch: WireBatch = serde_json::from_str(encoded)?;
    if batch.kind != "batch"
        || batch.protocol != PROTOCOL_VERSION
        || batch.source_id != expected_source_id
        || batch.records.len() > usize::from(MAX_POLL_RECORDS)
    {
        return Err(RuntimeError::new("source batch envelope mismatch"));
    }
    validate_cursor(&batch.cursor)?;
    let mut observations = Vec::with_capacity(batch.records.len());
    let mut skipped_non_usage_facts = 0;
    for record in batch.records {
        let kind = record
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| RuntimeError::new("source fact is missing a kind"))?;
        if kind != "usage" {
            let skipped: WireSkippedRecord = serde_json::from_value(record)?;
            validate_skipped_record(&skipped)?;
            skipped_non_usage_facts += 1;
            continue;
        }
        let typed: WireUsageRecord = serde_json::from_value(record)?;
        observations.push(typed.try_into()?);
    }
    Ok(ParsedCodexBatch {
        cursor: batch.cursor,
        observations,
        skipped_non_usage_facts,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloAck {
    #[serde(rename = "type")]
    kind: String,
    protocol: u64,
    plugin_id: String,
    features: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireBatch {
    #[serde(rename = "type")]
    kind: String,
    protocol: u64,
    source_id: String,
    cursor: String,
    records: Vec<serde_json::Value>,
}

struct ParsedCodexBatch {
    cursor: String,
    observations: Vec<RawObservation>,
    skipped_non_usage_facts: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireUsageRecord {
    event_id: String,
    revision: u32,
    kind: String,
    time_ns: String,
    identity: WireIdentity,
    payload: WireUsage,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSkippedRecord {
    event_id: String,
    #[serde(rename = "revision")]
    _revision: u32,
    kind: String,
    time_ns: String,
    identity: Option<WireIdentity>,
    payload: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireLifecycle {
    state: String,
    reason_code: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireMessage {
    role: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTool {
    category: String,
    result: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCheck {
    category: String,
    result: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireDiagnostic {
    code: String,
    severity: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireIdentity {
    session_id: String,
    worker_id: Option<String>,
    turn_id: Option<String>,
    operation_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

fn validate_skipped_record(record: &WireSkippedRecord) -> Result<()> {
    if !valid_source_identifier(&record.event_id, 256)
        || parse_decimal_timestamp(&record.time_ns).is_err()
    {
        return Err(RuntimeError::new(
            "skipped Codex fact identity or timestamp is invalid",
        ));
    }
    match record.kind.as_str() {
        "diagnostic" => {
            if let Some(identity) = &record.identity {
                validate_wire_identity(identity)?;
            }
            let diagnostic: WireDiagnostic = serde_json::from_value(record.payload.clone())?;
            if diagnostic.code.is_empty()
                || diagnostic.code.len() > 128
                || !matches!(diagnostic.severity.as_str(), "info" | "warning")
            {
                return Err(RuntimeError::new("Codex diagnostic payload is invalid"));
            }
        }
        "lifecycle" | "message" | "tool" | "check" => {
            let identity = record
                .identity
                .as_ref()
                .ok_or_else(|| RuntimeError::new("Codex fact identity is missing"))?;
            validate_wire_identity(identity)?;
            match record.kind.as_str() {
                "lifecycle" => {
                    let lifecycle: WireLifecycle = serde_json::from_value(record.payload.clone())?;
                    if !matches!(
                        lifecycle.state.as_str(),
                        "running" | "completed" | "interrupted" | "paused"
                    ) || lifecycle
                        .reason_code
                        .as_ref()
                        .is_some_and(|reason| reason.is_empty() || reason.len() > 128)
                    {
                        return Err(RuntimeError::new("Codex lifecycle payload is invalid"));
                    }
                }
                "message" => {
                    let message: WireMessage = serde_json::from_value(record.payload.clone())?;
                    if !matches!(message.role.as_str(), "user" | "assistant") {
                        return Err(RuntimeError::new("Codex message payload is invalid"));
                    }
                }
                "tool" => {
                    let tool: WireTool = serde_json::from_value(record.payload.clone())?;
                    if !matches!(tool.category.as_str(), "shell" | "file" | "mcp" | "other")
                        || !matches!(
                            tool.result.as_str(),
                            "started" | "succeeded" | "failed" | "denied"
                        )
                    {
                        return Err(RuntimeError::new("Codex tool payload is invalid"));
                    }
                }
                "check" => {
                    let check: WireCheck = serde_json::from_value(record.payload.clone())?;
                    if !matches!(check.category.as_str(), "test" | "lint" | "build")
                        || !matches!(check.result.as_str(), "passed" | "failed" | "unknown")
                    {
                        return Err(RuntimeError::new("Codex check payload is invalid"));
                    }
                }
                _ => unreachable!("kind is matched above"),
            }
        }
        _ => return Err(RuntimeError::new("unsupported Codex fact kind")),
    }
    Ok(())
}

fn validate_wire_identity(identity: &WireIdentity) -> Result<()> {
    for value in [
        Some(identity.session_id.as_str()),
        identity.worker_id.as_deref(),
        identity.turn_id.as_deref(),
        identity.operation_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if !valid_source_identifier(value, 512) {
            return Err(RuntimeError::new("Codex native identity is invalid"));
        }
    }
    Ok(())
}

fn valid_source_identifier(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

impl TryFrom<WireUsageRecord> for RawObservation {
    type Error = RuntimeError;

    fn try_from(record: WireUsageRecord) -> Result<Self> {
        if record.kind != "usage" {
            return Err(RuntimeError::new("unsupported source fact kind"));
        }
        let time_ns = parse_decimal_timestamp(&record.time_ns)?;
        let counters = &record.payload;
        if counters.input_tokens.is_none()
            && counters.output_tokens.is_none()
            && counters.cache_read_tokens.is_none()
            && counters.cache_write_tokens.is_none()
            && counters.total_tokens.is_none()
        {
            return Err(RuntimeError::new("Codex usage record has no counters"));
        }
        Ok(RawObservation {
            event_id: record.event_id,
            revision: record.revision,
            identity: ObservationIdentity {
                session_id: record.identity.session_id,
                worker_id: record.identity.worker_id,
                turn_id: record.identity.turn_id,
                operation_id: record.identity.operation_id,
            },
            observed_at_unix_ns: time_ns,
            kind: ObservationKind::Usage {
                input_tokens: counters.input_tokens,
                output_tokens: counters.output_tokens,
                cache_read_tokens: counters.cache_read_tokens,
                cache_write_tokens: counters.cache_write_tokens,
                total_tokens: counters.total_tokens,
            },
        })
    }
}

fn parse_decimal_timestamp(value: &str) -> Result<u64> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(RuntimeError::new(
            "source timestamp is not canonical decimal",
        ));
    }
    let timestamp = value
        .parse::<u64>()
        .map_err(|_| RuntimeError::new("source timestamp is outside range"))?;
    if timestamp > i64::MAX as u64 {
        return Err(RuntimeError::new(
            "source timestamp exceeds supported range",
        ));
    }
    Ok(timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_preserves_native_event_identity_revision_and_nanoseconds() {
        let response = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"codex:file:1:usage:hash","revision":2,"kind":"usage","time_ns":"1700000000123456789","identity":{"session_id":"session-1","worker_id":"worker-1","turn_id":"turn-1","operation_id":"operation-1"},"payload":{"input_tokens":7,"output_tokens":3}}]}"#;
        let batch = parse_codex_batch(response, "harness.codex").unwrap();
        assert_eq!(batch.observations[0].revision, 2);
        assert_eq!(
            batch.observations[0].observed_at_unix_ns,
            1_700_000_000_123_456_789
        );
        assert_eq!(
            batch.observations[0].identity.worker_id.as_deref(),
            Some("worker-1")
        );
    }

    #[test]
    fn strict_usage_wire_schema_rejects_unknown_fields_and_noncanonical_times() {
        let unknown = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"e1","revision":0,"kind":"usage","time_ns":"1","identity":{"session_id":"s1"},"payload":{"input_tokens":1,"output_tokens":2,"cached_input_tokens":3}}]}"#;
        assert!(parse_codex_batch(unknown, "harness.codex").is_err());
        assert!(parse_decimal_timestamp("01").is_err());
        assert!(parse_decimal_timestamp("+1").is_err());
        assert!(parse_decimal_timestamp("9223372036854775808").is_err());
    }

    #[test]
    fn usage_projection_preserves_partial_and_cached_counters_and_counts_skips() {
        let response = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"tool-1","revision":0,"kind":"tool","time_ns":"1","identity":{"session_id":"s1"},"payload":{"category":"shell","result":"succeeded"}},{"event_id":"usage-1","revision":0,"kind":"usage","time_ns":"2","identity":{"session_id":"s1"},"payload":{"cache_read_tokens":0,"cache_write_tokens":4,"total_tokens":4}}]}"#;
        let batch = parse_codex_batch(response, "harness.codex").unwrap();
        assert_eq!(batch.skipped_non_usage_facts, 1);
        assert_eq!(batch.observations.len(), 1);
        assert_eq!(
            batch.observations[0].kind,
            ObservationKind::Usage {
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: Some(0),
                cache_write_tokens: Some(4),
                total_tokens: Some(4),
            }
        );
    }

    #[test]
    fn projection_rejects_unknown_or_malformed_non_usage_facts() {
        let unknown_kind = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"e1","revision":0,"kind":"usgae","time_ns":"1","identity":{"session_id":"s1"},"payload":{"input_tokens":1}}]}"#;
        assert!(parse_codex_batch(unknown_kind, "harness.codex").is_err());

        let malformed_tool = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"e1","revision":0,"kind":"tool","time_ns":"1","identity":{"session_id":"s1"},"payload":{"category":"shell"}}]}"#;
        assert!(parse_codex_batch(malformed_tool, "harness.codex").is_err());
    }
}
