//! Supervised process adapters for the Agenturm kernel.
//!
//! Process supervision is independent of harness identity. `SourceBatchDecoder`
//! implementations project bounded wire batches into kernel facts. The catalog
//! selects a shared observation format independent of the source adapter.
//! Cursor commit and ACK ordering live in the kernel.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod connector;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod connector_sandbox;
mod observation;
mod observation_extension;

pub use agenturm_protocol::observation::ObservationFormat;
pub use observation::ObservationV1Decoder;
/// Previous name for the shared version 1 observation decoder.
pub use observation::ObservationV1Decoder as BundledObservationDecoder;

pub use observation_extension::{
    ExtensionDataGrant, ProcessObservationExtension, ProcessObservationExtensionConfig,
    verify_extension_artifact,
};

use agenturm_kernel::{PreparedSourceBatch, SourceAdapter};
use agenturm_protocol::{read_frame, write_frame};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const PROTOCOL_VERSION: u64 = 1;
const CONTROL_FRAME_LIMIT: usize = 64 * 1024;
const DATA_FRAME_LIMIT: usize = 64 * 1024;
const MAX_SOURCE_CURSOR_BYTES: usize = 800;
const MAX_PLUGIN_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_POLL_RECORDS: u16 = 256;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
pub const CODEX_PLUGIN_ID: &str = "example.rust-codex-harness";

pub type Result<T> = std::result::Result<T, RuntimeError>;

#[derive(Debug)]
pub struct RuntimeError(String);

impl RuntimeError {
    pub fn new(message: impl Into<String>) -> Self {
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
    fn from(_: serde_json::Error) -> Self {
        Self::new("source message has invalid JSON")
    }
}

/// Host-side projection of a shared wire format into typed kernel observations.
/// Implementations must reject unknown or malformed envelopes and keep source
/// content out of returned error messages.
pub trait SourceBatchDecoder: Send + Sync {
    fn decode_batch(&self, encoded: &str, expected_source_id: &str) -> Result<PreparedSourceBatch>;
}

/// Configures a Rust adapter launched through the Rust plugin host.
#[derive(Clone, Debug)]
pub struct ProcessConfig {
    pub plugin_host: PathBuf,
    pub adapter: PathBuf,
    pub source_root: PathBuf,
    pub source_id: String,
    pub plugin_id: String,
    pub timeout: Duration,
    pub stop_flag: Option<Arc<AtomicBool>>,
    expected_sha256: Option<String>,
    expected_features: Option<Vec<String>>,
    observation_format: ObservationFormat,
}

impl ProcessConfig {
    /// Configuration for an adapter registered by the Rust plugin host.
    pub fn new(
        plugin_host: impl Into<PathBuf>,
        adapter: impl Into<PathBuf>,
        source_root: impl Into<PathBuf>,
        source_id: impl Into<String>,
        plugin_id: impl Into<String>,
    ) -> Self {
        Self {
            plugin_host: plugin_host.into(),
            adapter: adapter.into(),
            source_root: source_root.into(),
            source_id: source_id.into(),
            plugin_id: plugin_id.into(),
            timeout: DEFAULT_TIMEOUT,
            stop_flag: None,
            expected_sha256: None,
            expected_features: None,
            observation_format: ObservationFormat::default(),
        }
    }

    /// Stops waiting for child replies when the shared flag is set. The
    /// process group is terminated on cancellation and is not restarted.
    pub fn with_stop_flag(mut self, stop_flag: Arc<AtomicBool>) -> Self {
        self.stop_flag = Some(stop_flag);
        self
    }

    /// Pins the adapter executable and verifies it before every host launch.
    pub fn with_artifact_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.expected_sha256 = Some(sha256.into());
        self
    }

    /// Requires the child handshake to advertise exactly the catalog features.
    pub fn with_expected_features(mut self, features: Vec<String>) -> Self {
        self.expected_features = Some(features);
        self
    }

    /// Selects the host decoder for the adapter’s observation wire format.
    pub fn with_observation_format(mut self, format: ObservationFormat) -> Self {
        self.observation_format = format;
        self
    }

    /// Configuration for the bundled Codex adapter.
    pub fn codex(
        plugin_host: impl Into<PathBuf>,
        adapter: impl Into<PathBuf>,
        source_root: impl Into<PathBuf>,
        source_id: impl Into<String>,
    ) -> Self {
        Self::new(
            plugin_host,
            adapter,
            source_root,
            source_id,
            CODEX_PLUGIN_ID,
        )
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
    decoder: Arc<dyn SourceBatchDecoder>,
}

impl ProcessSource {
    pub fn spawn(config: ProcessConfig) -> Result<Self> {
        match config.observation_format {
            ObservationFormat::V1 => Self::spawn_with_decoder(config, ObservationV1Decoder),
        }
    }

    pub fn spawn_with_decoder(
        config: ProcessConfig,
        decoder: impl SourceBatchDecoder + 'static,
    ) -> Result<Self> {
        Self::spawn_with_shared_decoder(config, Arc::new(decoder))
    }

    fn spawn_with_shared_decoder(
        config: ProcessConfig,
        decoder: Arc<dyn SourceBatchDecoder>,
    ) -> Result<Self> {
        validate_process_config(&config)?;
        if let Some(expected_sha256) = &config.expected_sha256 {
            verify_artifact_sha256(&config.adapter, expected_sha256)?;
        }
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
            decoder,
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
            || !ack
                .features
                .iter()
                .any(|feature| feature == "source-adapter/1")
            || !ack
                .features
                .iter()
                .any(|feature| feature == "configured-root/1")
            || config_features_mismatch(&self.config, &ack.features)
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
        match receive_until_cancelled(
            &self.replies,
            self.config.timeout,
            self.config.stop_flag.as_deref(),
        ) {
            Ok(Ok(Some(frame))) => Ok(frame),
            Ok(Ok(None)) => Err(RuntimeError::new("plugin host closed its data pipe")),
            Ok(Err(_)) => Err(RuntimeError::new("plugin host returned an invalid frame")),
            Err(ReplyWaitError::TimedOut) => {
                self.terminate();
                Err(RuntimeError::new(
                    "source adapter exceeded its response deadline",
                ))
            }
            Err(ReplyWaitError::Disconnected) => {
                Err(RuntimeError::new("plugin host frame reader stopped"))
            }
            Err(ReplyWaitError::Cancelled) => Err(RuntimeError::new("source operation cancelled")),
        }
    }
}

fn config_features_mismatch(config: &ProcessConfig, actual: &[String]) -> bool {
    let Some(expected_values) = &config.expected_features else {
        return false;
    };
    let expected = expected_values
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    let actual_set = actual.iter().collect::<std::collections::BTreeSet<_>>();
    expected != actual_set
        || expected.len() != expected_values.len()
        || actual_set.len() != actual.len()
}

/// Verifies an adapter file against a lower-case SHA-256 pin without loading
/// the entire artifact into memory. This is an integrity check, not a sandbox.
pub fn verify_artifact_sha256(path: &Path, expected: &str) -> Result<()> {
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RuntimeError::new("adapter SHA-256 pin is invalid"));
    }
    let file =
        File::open(path).map_err(|_| RuntimeError::new("adapter artifact is unavailable"))?;
    let metadata = file
        .metadata()
        .map_err(|_| RuntimeError::new("adapter artifact metadata is unavailable"))?;
    if !metadata.is_file() || metadata.len() > MAX_PLUGIN_ARTIFACT_BYTES {
        return Err(RuntimeError::new(
            "adapter artifact is not a bounded regular file",
        ));
    }
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| RuntimeError::new("adapter artifact could not be read"))?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count as u64);
        if total > MAX_PLUGIN_ARTIFACT_BYTES {
            return Err(RuntimeError::new("adapter artifact exceeds the size limit"));
        }
        hasher.update(&buffer[..count]);
    }
    let actual = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual != expected {
        return Err(RuntimeError::new("adapter artifact SHA-256 mismatch"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplyWaitError {
    TimedOut,
    Disconnected,
    Cancelled,
}

fn receive_until_cancelled<T>(
    receiver: &Receiver<T>,
    timeout: Duration,
    stop_flag: Option<&AtomicBool>,
) -> std::result::Result<T, ReplyWaitError> {
    let deadline = Instant::now() + timeout;
    loop {
        if stop_flag.is_some_and(|stop| stop.load(Ordering::Relaxed)) {
            return Err(ReplyWaitError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ReplyWaitError::TimedOut);
        }
        let wait = if stop_flag.is_some() {
            remaining.min(Duration::from_millis(25))
        } else {
            remaining
        };
        match receiver.recv_timeout(wait) {
            Ok(value) => return Ok(value),
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(ReplyWaitError::TimedOut),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(ReplyWaitError::Disconnected),
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
            self.decoder.decode_batch(&reply, &self.config.source_id)
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
        if self
            .config
            .stop_flag
            .as_ref()
            .is_some_and(|stop| stop.load(Ordering::Relaxed))
        {
            self.terminate();
            return Ok(());
        }
        let config = self.config.clone();
        let decoder = self.decoder.clone();
        self.terminate();
        let replacement =
            Self::spawn_with_shared_decoder(config, decoder).map_err(|error| error.to_string())?;
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
        if self
            .config
            .stop_flag
            .as_ref()
            .is_some_and(|stop| stop.load(Ordering::Relaxed))
        {
            self.terminate();
            return;
        }
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
    if !valid_plugin_id(&config.plugin_id) {
        return Err(RuntimeError::new("plugin ID is invalid"));
    }
    if config.timeout.is_zero() {
        return Err(RuntimeError::new("source timeout must be positive"));
    }
    if config.expected_sha256.as_ref().is_some_and(|digest| {
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(RuntimeError::new("adapter SHA-256 pin is invalid"));
    }
    if config.expected_features.as_ref().is_some_and(|features| {
        features.len() > 64
            || features
                .iter()
                .any(|feature| !valid_plugin_feature(feature))
            || features
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != features.len()
            || !features.iter().any(|feature| feature == "source-adapter/1")
            || !features
                .iter()
                .any(|feature| feature == "configured-root/1")
    }) {
        return Err(RuntimeError::new("adapter feature declaration is invalid"));
    }
    Ok(())
}

fn valid_plugin_feature(value: &str) -> bool {
    let Some((name, version)) = value.split_once('/') else {
        return false;
    };
    !name.is_empty()
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
        && !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_digit())
        && !version.starts_with('0')
}

fn valid_plugin_id(value: &str) -> bool {
    agenturm_kernel::ExtensionId::parse(value.to_owned()).is_ok()
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloAck {
    #[serde(rename = "type")]
    kind: String,
    protocol: u64,
    plugin_id: String,
    features: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{parse_decimal_timestamp, parse_source_batch};
    use agenturm_kernel::ObservationKind;

    #[test]
    fn artifact_hash_verification_accepts_only_matching_regular_files_and_lowercase_pins() {
        let path = std::env::temp_dir().join(format!(
            "agenturm-plugin-artifact-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"adapter-artifact").unwrap();
        let expected = format!("{:x}", Sha256::digest(b"adapter-artifact"));
        assert!(verify_artifact_sha256(&path, &expected).is_ok());
        assert!(verify_artifact_sha256(&path, &"0".repeat(64)).is_err());
        assert!(verify_artifact_sha256(&path, &expected.to_uppercase()).is_err());
        assert!(verify_artifact_sha256(&path, "not-a-pin").is_err());
        assert!(verify_artifact_sha256(&path.with_extension("missing"), &expected).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn catalog_feature_handshake_is_exact_and_rejects_duplicate_ack_features() {
        let config =
            ProcessConfig::new("/host", "/adapter", "/root", "harness.test", "example.test")
                .with_expected_features(vec![
                    "source-adapter/1".into(),
                    "configured-root/1".into(),
                ]);
        let expected = [
            "source-adapter/1".to_owned(),
            "configured-root/1".to_owned(),
        ];
        assert!(!config_features_mismatch(&config, &expected));
        assert!(config_features_mismatch(
            &config,
            &["source-adapter/1".into(), "source-adapter/1".into()]
        ));
        assert!(config_features_mismatch(
            &config,
            &[
                "source-adapter/1".into(),
                "configured-root/1".into(),
                "extra/1".into()
            ]
        ));
    }

    #[test]
    fn reply_wait_observes_stop_flag_before_long_deadline() {
        let (_sender, receiver) = mpsc::channel::<u8>();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stop);
        let setter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            signal.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let result = receive_until_cancelled(&receiver, Duration::from_secs(5), Some(&stop));
        setter.join().unwrap();
        assert_eq!(result, Err(ReplyWaitError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn plugin_ids_use_the_kernel_extension_identifier_grammar() {
        assert!(valid_plugin_id("example.rust-pi-harness"));
        assert!(valid_plugin_id("example.rust_codex/harness"));
        assert!(!valid_plugin_id(""));
        assert!(!valid_plugin_id("Example.rust-pi-harness"));
        assert!(!valid_plugin_id("example.pi harness"));
        assert!(!valid_plugin_id("example.pi\n-harness"));
        assert!(!valid_plugin_id(&"a".repeat(97)));
    }

    #[test]
    fn parser_preserves_native_event_identity_revision_and_nanoseconds() {
        let response = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"codex:file:1:usage:hash","revision":2,"kind":"usage","time_ns":"1700000000123456789","identity":{"session_id":"session-1","worker_id":"worker-1","turn_id":"turn-1","operation_id":"operation-1"},"payload":{"input_tokens":7,"output_tokens":3}}]}"#;
        let batch = parse_source_batch(response, "harness.codex").unwrap();
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
    fn shared_decoder_rejects_ambiguous_fields_without_reported_source_content() {
        let encoded = r#"{"type":"batch","protocol":1,"source_id":"harness.test","cursor":"next","records":[{"event_id":"e1","revision":0,"kind":"usage","time_ns":"1","identity":{"session_id":"s1"},"payload":{"input_tokens":1}}]}"#;
        assert!(
            ObservationV1Decoder
                .decode_batch(encoded, "harness.test")
                .is_ok()
        );
        for (original, replacement) in [
            (
                r#""input_tokens":1"#,
                r#""input_tokens":1,"input_tokens":2"#,
            ),
            (r#""event_id":"e1""#, r#""event_id":"e1","event_id":"e2""#),
            (
                r#""session_id":"s1""#,
                r#""session_id":"s1","session_id":"s2""#,
            ),
            (r#""usage""#, r#""private-observation-content""#),
        ] {
            let error = ObservationV1Decoder
                .decode_batch(&encoded.replace(original, replacement), "harness.test")
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "source batch rejected: invalid observation schema"
            );
        }
        assert!(
            ObservationV1Decoder
                .decode_batch(encoded, "harness.other")
                .is_err()
        );
        assert!(
            ObservationV1Decoder
                .decode_batch(
                    &encoded.replace(r#""protocol":1"#, r#""protocol":2"#),
                    "harness.test"
                )
                .is_err()
        );
    }

    #[test]
    fn strict_usage_wire_schema_rejects_unknown_fields_and_noncanonical_times() {
        let unknown = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"e1","revision":0,"kind":"usage","time_ns":"1","identity":{"session_id":"s1"},"payload":{"input_tokens":1,"output_tokens":2,"cached_input_tokens":3}}]}"#;
        assert!(parse_source_batch(unknown, "harness.codex").is_err());
        assert!(parse_decimal_timestamp("01").is_err());
        assert!(parse_decimal_timestamp("+1").is_err());
        assert!(parse_decimal_timestamp("9223372036854775808").is_err());
    }

    #[test]
    fn bundled_projection_preserves_usage_and_typed_tool_facts() {
        let response = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"tool-1","revision":0,"kind":"tool","time_ns":"1","identity":{"session_id":"s1"},"payload":{"category":"shell","result":"succeeded"}},{"event_id":"usage-1","revision":0,"kind":"usage","time_ns":"2","identity":{"session_id":"s1"},"payload":{"cache_read_tokens":0,"cache_write_tokens":4,"total_tokens":4}}]}"#;
        let batch = parse_source_batch(response, "harness.codex").unwrap();
        assert_eq!(batch.skipped_unprojected_facts, 0);
        assert_eq!(batch.observations.len(), 2);
        assert_eq!(
            batch.observations[0].kind,
            ObservationKind::Tool {
                category: "shell".into(),
                result: "succeeded".into(),
            }
        );
        assert_eq!(
            batch.observations[1].kind,
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
    fn lifecycle_projection_preserves_state_and_safe_reason_code() {
        let response = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"lifecycle-1","revision":0,"kind":"lifecycle","time_ns":"9","identity":{"session_id":"s1","turn_id":"turn-1"},"payload":{"state":"interrupted","reason_code":"codex_task_aborted"}}]}"#;
        let batch = parse_source_batch(response, "harness.codex").unwrap();
        assert_eq!(batch.skipped_unprojected_facts, 0);
        assert_eq!(batch.observations.len(), 1);
        assert_eq!(
            batch.observations[0].identity.turn_id.as_deref(),
            Some("turn-1")
        );
        assert_eq!(
            batch.observations[0].kind,
            ObservationKind::Lifecycle {
                state: "interrupted".into(),
                reason_code: Some("codex_task_aborted".into()),
            }
        );
        let invalid_state = response.replace("interrupted", "crashed");
        assert!(parse_source_batch(&invalid_state, "harness.codex").is_err());
        let invalid_reason = response.replace("codex_task_aborted", "Raw error message");
        assert!(parse_source_batch(&invalid_reason, "harness.codex").is_err());
        let unknown_field = response.replace(
            "\"reason_code\":\"codex_task_aborted\"",
            "\"reason_code\":\"codex_task_aborted\",\"message\":\"secret\"",
        );
        assert!(parse_source_batch(&unknown_field, "harness.codex").is_err());
    }

    #[test]
    fn check_projection_preserves_only_typed_category_and_result() {
        let response = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"check-1","revision":0,"kind":"check","time_ns":"10","identity":{"session_id":"s1","turn_id":"turn-1"},"payload":{"category":"lint","result":"passed"}}]}"#;
        let batch = parse_source_batch(response, "harness.codex").unwrap();
        assert_eq!(batch.skipped_unprojected_facts, 0);
        assert_eq!(batch.observations.len(), 1);
        assert_eq!(
            batch.observations[0].kind,
            ObservationKind::Check {
                category: "lint".into(),
                result: "passed".into(),
            }
        );
        let invalid_result = response.replace("passed", "successful");
        assert!(parse_source_batch(&invalid_result, "harness.codex").is_err());
        let invalid_category = response.replace("lint", "coverage");
        assert!(parse_source_batch(&invalid_category, "harness.codex").is_err());
        let unknown_field = response.replace(
            "\"category\":\"lint\"",
            "\"category\":\"lint\",\"command\":\"secret\"",
        );
        assert!(parse_source_batch(&unknown_field, "harness.codex").is_err());
    }

    #[test]
    fn message_projection_preserves_only_role() {
        let response = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"message-1","revision":0,"kind":"message","time_ns":"11","identity":{"session_id":"s1","turn_id":"turn-1"},"payload":{"role":"user"}},{"event_id":"message-2","revision":0,"kind":"message","time_ns":"12","identity":{"session_id":"s1","turn_id":"turn-1"},"payload":{"role":"assistant"}}]}"#;
        let batch = parse_source_batch(response, "harness.codex").unwrap();
        assert_eq!(batch.observations.len(), 2);
        assert_eq!(batch.skipped_unprojected_facts, 0);
        assert!(batch.observations.iter().any(|row| row.kind
            == ObservationKind::Message {
                role: "user".into()
            }));
        assert!(batch.observations.iter().any(|row| row.kind
            == ObservationKind::Message {
                role: "assistant".into()
            }));
        assert!(parse_source_batch(&response.replace("user", "system"), "harness.codex").is_err());
        let extra_content = response.replace(
            "\"role\":\"user\"",
            "\"role\":\"user\",\"content\":\"private prompt\"",
        );
        assert!(parse_source_batch(&extra_content, "harness.codex").is_err());
    }

    #[test]
    fn identity_bearing_error_diagnostics_are_validated_and_counted() {
        let response = r#"{"type":"batch","protocol":1,"source_id":"harness.kiro","cursor":"next","records":[{"event_id":"kiro:display-error","revision":0,"kind":"diagnostic","time_ns":"11","identity":{"session_id":"s1","turn_id":"turn-1"},"payload":{"code":"kiro_display_error","severity":"error"}}]}"#;
        let batch = parse_source_batch(response, "harness.kiro").unwrap();
        assert_eq!(batch.observations.len(), 1);
        assert_eq!(batch.skipped_unprojected_facts, 0);
        assert_eq!(
            batch.observations[0].identity.scope,
            agenturm_kernel::ObservationScope::Session
        );
        assert_eq!(
            batch.observations[0].kind,
            ObservationKind::Diagnostic {
                code: "kiro_display_error".into(),
                severity: "error".into(),
            }
        );
        let unsupported_severity = response.replace("\"error\"", "\"critical\"");
        assert!(parse_source_batch(&unsupported_severity, "harness.kiro").is_err());
        let raw_message = response.replace(
            "\"severity\":\"error\"",
            "\"severity\":\"error\",\"message\":\"private text\"",
        );
        assert!(parse_source_batch(&raw_message, "harness.kiro").is_err());
    }

    #[test]
    fn source_diagnostics_use_explicit_source_scope_and_closed_code_catalog() {
        let response = r#"{"type":"batch","protocol":1,"source_id":"harness.pi","cursor":"next","records":[{"event_id":"pi:gap","revision":0,"kind":"diagnostic","time_ns":"11","payload":{"code":"possible_source_gap","severity":"warning"}},{"event_id":"pi:custom","revision":0,"kind":"diagnostic","time_ns":"12","payload":{"code":"private_content_in_code","severity":"warning"}}]}"#;
        let batch = parse_source_batch(response, "harness.pi").unwrap();
        assert_eq!(batch.observations.len(), 1);
        assert_eq!(batch.skipped_unprojected_facts, 1);
        assert_eq!(
            batch.observations[0].identity.scope,
            agenturm_kernel::ObservationScope::Source
        );
        assert_eq!(batch.observations[0].identity.session_id, None);
        assert_eq!(
            batch.observations[0].kind,
            ObservationKind::Diagnostic {
                code: "possible_source_gap".into(),
                severity: "warning".into(),
            }
        );
        let added_payload = response.replace(
            "\"severity\":\"warning\"",
            "\"severity\":\"warning\",\"message\":\"private text\"",
        );
        assert!(parse_source_batch(&added_payload, "harness.pi").is_err());
    }

    #[test]
    fn projection_rejects_unknown_or_malformed_non_usage_facts() {
        let unknown_kind = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"e1","revision":0,"kind":"usgae","time_ns":"1","identity":{"session_id":"s1"},"payload":{"input_tokens":1}}]}"#;
        assert!(parse_source_batch(unknown_kind, "harness.codex").is_err());

        let malformed_tool = r#"{"type":"batch","protocol":1,"source_id":"harness.codex","cursor":"next","records":[{"event_id":"e1","revision":0,"kind":"tool","time_ns":"1","identity":{"session_id":"s1"},"payload":{"category":"shell"}}]}"#;
        assert!(parse_source_batch(malformed_tool, "harness.codex").is_err());
    }
}
