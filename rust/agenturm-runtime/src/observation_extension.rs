//! Strict, bounded process boundary for runtime-installable capability and
//! pane extensions. Children receive a privacy-limited projection and run
//! synchronously under one total invocation deadline.

use crate::connector::ConnectorLaunch;
use agenturm_kernel::{
    ExtensionId, MetricRow, Observation, ObservationExtension, ObservationExtensionInput,
    ObservationExtensionOutput, ObservationKind, PaneModel,
};
use serde::{Deserialize, Serialize, de::Deserializer};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROTOCOL: u32 = 1;
const FRAME_LIMIT: usize = 4 * 1024 * 1024;
const MAX_OBSERVATIONS: usize = 500;
const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_INVOCATION: Duration = Duration::from_millis(500);
const MAX_ROWS: usize = 256;
const MAX_TITLE_BYTES: usize = 128;
const MAX_LABEL_BYTES: usize = 128;
const MAX_VALUE_BYTES: usize = 1024;
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static CHILD_REAPER: OnceLock<Mutex<ReaperState>> = OnceLock::new();

/// Typed observation families a process extension may receive.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ExtensionDataGrant {
    Usage,
    MessageRole,
    Tool,
    Check,
    Lifecycle,
    Diagnostic,
}

impl ExtensionDataGrant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::MessageRole => "message-role",
            Self::Tool => "tool",
            Self::Check => "check",
            Self::Lifecycle => "lifecycle",
            Self::Diagnostic => "diagnostic",
        }
    }
}

/// Configuration resolved from an explicitly selected user-managed catalog.
#[derive(Clone, Debug)]
pub struct ProcessObservationExtensionConfig {
    pub plugin_id: ExtensionId,
    pub executable: PathBuf,
    pub sha256: String,
    pub features: Vec<String>,
    pub grants: BTreeSet<ExtensionDataGrant>,
}

/// One runtime-installable, supervised metrics-and-pane extension.
pub struct ProcessObservationExtension {
    config: ProcessObservationExtensionConfig,
}

impl ProcessObservationExtension {
    pub fn new(mut config: ProcessObservationExtensionConfig) -> Result<Self, String> {
        validate_config(&config)?;
        config.executable = std::fs::canonicalize(&config.executable)
            .map_err(|_| "extension executable path cannot be resolved".to_owned())?;
        Ok(Self { config })
    }

    pub fn plugin_id(&self) -> &ExtensionId {
        &self.config.plugin_id
    }
}

impl ObservationExtension for ProcessObservationExtension {
    fn evaluate(
        &self,
        input: &ObservationExtensionInput<'_>,
    ) -> Result<ObservationExtensionOutput, String> {
        invoke(&self.config, input).map_err(|error| error.to_string())
    }
}

/// Verify the selected extension executable before opening the store.
pub fn verify_extension_artifact(path: &Path, sha256: &str) -> Result<(), String> {
    verify_artifact(path, sha256, MAX_ARTIFACT_BYTES, None).map_err(|error| error.to_string())
}

/// Runs one credential-free, pinned connector exchange on the same bounded
/// process transport used by observation extensions.
pub(crate) fn invoke_connector(
    executable: &Path,
    sha256: &str,
    launch: ConnectorLaunch,
    request: &[u8],
) -> Result<Vec<u8>, &'static str> {
    #[cfg(unix)]
    fn run(
        executable: &Path,
        sha256: &str,
        launch: ConnectorLaunch,
        request: &[u8],
    ) -> Result<Vec<u8>, InvocationError> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let artifact = open_connector_file(executable)?;
        let process = match launch {
            ConnectorLaunch::Trusted => {
                verify_artifact_file(&artifact, sha256, MAX_ARTIFACT_BYTES, Some(deadline))?;
                #[cfg(target_os = "linux")]
                let pinned_path = connector_fd_path(&artifact);
                #[cfg(target_os = "linux")]
                let executable = &pinned_path;
                #[cfg(not(target_os = "linux"))]
                let _artifact = artifact;
                connector_command(executable)
                    .spawn()
                    .map_err(|_| InvocationError::Launch)?
            }
            ConnectorLaunch::IsolatedLinux => spawn_isolated_connector(artifact, sha256, deadline)?,
        };
        let mut guard = GuardedChild(Some(process));
        let process = guard.0.as_mut().expect("child guard populated");
        let input = process
            .stdin
            .as_ref()
            .ok_or(InvocationError::Pipe)?
            .as_raw_fd();
        let output = process
            .stdout
            .as_ref()
            .ok_or(InvocationError::Pipe)?
            .as_raw_fd();
        set_nonblocking(input).map_err(|_| InvocationError::Pipe)?;
        set_nonblocking(output).map_err(|_| InvocationError::Pipe)?;
        write_frame_deadline(input, request, deadline)?;
        drop(process.stdin.take());
        let response = read_frame_deadline(output, deadline)?;
        require_eof_deadline(output, deadline)?;
        wait_child_deadline(process, deadline)?;
        if !process.wait().map_err(|_| InvocationError::Io)?.success() {
            return Err(InvocationError::Exit);
        }
        Ok(response)
    }
    #[cfg(unix)]
    return run(executable, sha256, launch, request).map_err(|error| match error {
        InvocationError::Timeout => "connector_timeout",
        InvocationError::Sandbox => "connector_isolation_unavailable",
        InvocationError::Protocol(_) => "connector_protocol_error",
        _ => "connector_process_error",
    });
    #[cfg(not(unix))]
    {
        let _ = (executable, sha256, launch, request);
        Err("connector_unsupported_platform")
    }
}

pub(crate) fn verify_connector_artifact(
    executable: &Path,
    sha256: &str,
    launch: ConnectorLaunch,
) -> Result<(), &'static str> {
    #[cfg(unix)]
    {
        let artifact =
            open_connector_file(executable).map_err(|_| "connector_artifact_mismatch")?;
        verify_artifact_file(
            &artifact,
            sha256,
            MAX_ARTIFACT_BYTES,
            Some(Instant::now() + Duration::from_secs(2)),
        )
        .map_err(|_| "connector_artifact_mismatch")?;
        if launch == ConnectorLaunch::IsolatedLinux {
            #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
            return crate::connector_sandbox::check_static_elf(&artifact)
                .map_err(|_| "connector_isolation_artifact_unsupported");
            #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
            return Err("connector_isolation_unavailable");
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (executable, sha256, launch);
        Err("connector_unsupported_platform")
    }
}

#[cfg(unix)]
fn connector_command(executable: &Path) -> Command {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(executable);
    command
        .current_dir("/")
        .env_clear()
        .env("AGENTURM_CONNECTOR_PROTOCOL", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    command
}

#[cfg(target_os = "linux")]
fn connector_fd_path(artifact: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", artifact.as_raw_fd()))
}

#[cfg(unix)]
fn spawn_isolated_connector(
    artifact: File,
    sha256: &str,
    deadline: Instant,
) -> Result<Child, InvocationError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        use crate::connector_sandbox::{Stage, check_static_elf, spawn};
        let stage = Stage::new().map_err(|_| InvocationError::Sandbox)?;
        let staged = stage
            .copy(&artifact, MAX_ARTIFACT_BYTES)
            .map_err(|_| InvocationError::Sandbox)?;
        // Authenticate the bytes the kernel will execute.
        verify_artifact_file(&staged, sha256, MAX_ARTIFACT_BYTES, Some(deadline))?;
        check_static_elf(&staged).map_err(|_| InvocationError::Sandbox)?;
        if Instant::now() >= deadline {
            return Err(InvocationError::Timeout);
        }
        spawn(connector_command(&connector_fd_path(&staged)), &staged)
            .map_err(|_| InvocationError::Sandbox)
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (artifact, sha256, deadline);
        Err(InvocationError::Sandbox)
    }
}

#[cfg(unix)]
fn open_connector_file(path: &Path) -> Result<File, InvocationError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| InvocationError::Protocol("connector executable is unavailable"))?;
    let metadata = file
        .metadata()
        .map_err(|_| InvocationError::Protocol("connector executable metadata is unavailable"))?;
    if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(InvocationError::Protocol(
            "connector executable is not a bounded regular file",
        ));
    }
    Ok(file)
}

fn validate_config(config: &ProcessObservationExtensionConfig) -> Result<(), String> {
    if !config.executable.is_absolute() {
        return Err("extension executable path must be absolute".into());
    }
    validate_sha256(&config.sha256)?;
    if config.features.is_empty()
        || config.features.len() > 2
        || !config
            .features
            .iter()
            .any(|feature| feature == "capability/1")
        || config
            .features
            .iter()
            .any(|feature| !matches!(feature.as_str(), "capability/1" | "ui-contribution/1"))
        || config.features.iter().collect::<BTreeSet<_>>().len() != config.features.len()
    {
        return Err("extension feature declaration is invalid".into());
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("extension SHA-256 pin is invalid".into());
    }
    Ok(())
}

fn invoke(
    config: &ProcessObservationExtensionConfig,
    input: &ObservationExtensionInput<'_>,
) -> Result<ObservationExtensionOutput, InvocationError> {
    let deadline = Instant::now() + MAX_INVOCATION;
    verify_artifact(
        &config.executable,
        &config.sha256,
        MAX_ARTIFACT_BYTES,
        Some(deadline),
    )?;
    let request_id = next_request_id();
    let evaluate = make_evaluate_request(config, input, request_id.clone())?;
    let evaluate_bytes = serde_json::to_vec(&evaluate)
        .map_err(|_| InvocationError::Protocol("could not encode extension request"))?;
    if evaluate_bytes.len() > FRAME_LIMIT {
        return Err(InvocationError::Protocol(
            "extension request exceeds frame limit",
        ));
    }
    if Instant::now() >= deadline {
        return Err(InvocationError::Timeout);
    }

    #[cfg(not(unix))]
    return Err(InvocationError::UnsupportedPlatform);

    #[cfg(unix)]
    invoke_unix(config, &request_id, &evaluate_bytes, deadline)
}

#[cfg(unix)]
fn invoke_unix(
    config: &ProcessObservationExtensionConfig,
    request_id: &str,
    evaluate_bytes: &[u8],
    deadline: Instant,
) -> Result<ObservationExtensionOutput, InvocationError> {
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(&config.executable);
    command
        .current_dir("/")
        .env_clear()
        .env("AGENTURM_EXTENSION_PROTOCOL", PROTOCOL.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = GuardedChild(Some(command.spawn().map_err(|_| InvocationError::Launch)?));
    if Instant::now() >= deadline {
        return Err(InvocationError::Timeout);
    }
    let process = child.0.as_mut().expect("child guard populated");
    let input_fd = process
        .stdin
        .as_ref()
        .ok_or(InvocationError::Pipe)?
        .as_raw_fd();
    let output_fd = process
        .stdout
        .as_ref()
        .ok_or(InvocationError::Pipe)?
        .as_raw_fd();
    set_nonblocking(input_fd).map_err(|_| InvocationError::Pipe)?;
    set_nonblocking(output_fd).map_err(|_| InvocationError::Pipe)?;

    let hello = Hello {
        frame_type: "hello",
        protocol: PROTOCOL,
        plugin_id: config.plugin_id.as_str(),
        request_id,
    };
    let hello = serde_json::to_vec(&hello)
        .map_err(|_| InvocationError::Protocol("could not encode hello"))?;
    write_frame_deadline(input_fd, &hello, deadline)?;
    let hello_frame = read_frame_deadline(output_fd, deadline)?;
    let ack: HelloAck = parse_strict(&hello_frame, "invalid extension handshake")?;
    validate_hello_ack(config, request_id, ack)?;

    write_frame_deadline(input_fd, evaluate_bytes, deadline)?;
    drop(process.stdin.take());
    let response_frame = read_frame_deadline(output_fd, deadline)?;
    let response: EvaluateResult = parse_strict(&response_frame, "invalid extension response")?;
    let result = validate_result(config, request_id, response)?;

    // The protocol is one-shot. Require immediate EOF after the one result so
    // additional frames and escaped pipe holders cannot pin a reader thread.
    require_eof_deadline(output_fd, deadline)?;
    let process = child.0.as_mut().expect("child guard populated");
    wait_child_deadline(process, deadline)?;
    let mut process = child.0.take().expect("child guard populated");
    if !process.wait().map_err(|_| InvocationError::Io)?.success() {
        return Err(InvocationError::Exit);
    }
    Ok(result)
}

fn make_evaluate_request(
    config: &ProcessObservationExtensionConfig,
    input: &ObservationExtensionInput<'_>,
    request_id: String,
) -> Result<EvaluateRequest, InvocationError> {
    let end = input.observations.len();
    let start = end.saturating_sub(MAX_OBSERVATIONS);
    let observations = input.observations[start..]
        .iter()
        .map(|observation| project_observation(observation, &config.grants))
        .collect();
    Ok(EvaluateRequest {
        frame_type: "evaluate",
        protocol: PROTOCOL,
        plugin_id: config.plugin_id.as_str().to_owned(),
        request_id,
        snapshot_id: input.snapshot_id,
        observation_count: input.observations.len() as u64,
        truncated: input.observations_truncated || start > 0,
        selection_scope: match input.selection_scope {
            agenturm_kernel::ObservationSelectionScope::Latest => "latest",
            agenturm_kernel::ObservationSelectionScope::Filtered => "filtered",
            agenturm_kernel::ObservationSelectionScope::Continuation => "continuation",
        },
        observations,
    })
}

#[derive(Serialize)]
struct Hello<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    protocol: u32,
    plugin_id: &'a str,
    request_id: &'a str,
}

#[derive(Serialize)]
struct EvaluateRequest {
    #[serde(rename = "type")]
    frame_type: &'static str,
    protocol: u32,
    plugin_id: String,
    request_id: String,
    snapshot_id: u64,
    observation_count: u64,
    truncated: bool,
    selection_scope: &'static str,
    observations: Vec<WireObservation>,
}

#[derive(Serialize)]
struct WireObservation {
    source_id: String,
    observed_at_unix_ns: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<String>,
}

fn project_observation(
    observation: &Observation,
    grants: &BTreeSet<ExtensionDataGrant>,
) -> WireObservation {
    let mut wire = WireObservation {
        source_id: observation.harness_id.as_str().to_owned(),
        observed_at_unix_ns: observation.observed_at_unix_ns.to_string(),
        kind: kind_name(&observation.kind),
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
        total_tokens: None,
        role: None,
        tool_category: None,
        tool_result: None,
        check_category: None,
        check_result: None,
        state: None,
        reason_code: None,
        diagnostic_code: None,
        severity: None,
    };
    match &observation.kind {
        ObservationKind::Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            total_tokens,
        } if grants.contains(&ExtensionDataGrant::Usage) => {
            wire.input_tokens = *input_tokens;
            wire.output_tokens = *output_tokens;
            wire.cache_read_tokens = *cache_read_tokens;
            wire.cache_write_tokens = *cache_write_tokens;
            wire.total_tokens = *total_tokens;
        }
        ObservationKind::Message { role } if grants.contains(&ExtensionDataGrant::MessageRole) => {
            wire.role = Some(role.clone());
        }
        ObservationKind::Tool { category, result }
            if grants.contains(&ExtensionDataGrant::Tool) =>
        {
            wire.tool_category = Some(category.clone());
            wire.tool_result = Some(result.clone());
        }
        ObservationKind::Check { category, result }
            if grants.contains(&ExtensionDataGrant::Check) =>
        {
            wire.check_category = Some(category.clone());
            wire.check_result = Some(result.clone());
        }
        ObservationKind::Lifecycle { state, reason_code }
            if grants.contains(&ExtensionDataGrant::Lifecycle) =>
        {
            wire.state = Some(state.clone());
            wire.reason_code = reason_code.clone();
        }
        ObservationKind::Diagnostic { code, severity }
            if grants.contains(&ExtensionDataGrant::Diagnostic) =>
        {
            wire.diagnostic_code = Some(code.clone());
            wire.severity = Some(severity.clone());
        }
        _ => {}
    }
    wire
}

fn kind_name(kind: &ObservationKind) -> &'static str {
    match kind {
        ObservationKind::SessionStarted => "session-started",
        ObservationKind::SessionEnded => "session-ended",
        ObservationKind::Activity { .. } => "activity",
        ObservationKind::Tool { .. } => "tool",
        ObservationKind::FileEdit(_) => "file-edit",
        ObservationKind::McpCall { .. } => "mcp-call",
        ObservationKind::TraceSpan(_) => "trace-span",
        ObservationKind::Lifecycle { .. } => "lifecycle",
        ObservationKind::Check { .. } => "check",
        ObservationKind::PullRequestRaised { .. } => "pull-request-raised",
        ObservationKind::Relationship { .. } => "relationship",
        ObservationKind::Message { .. } => "message",
        ObservationKind::Diagnostic { .. } => "diagnostic",
        ObservationKind::Usage { .. } => "usage",
        ObservationKind::CreditUsage { .. } => "credit-usage",
        ObservationKind::ReportedCost(_) => "reported-cost",
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloAck {
    #[serde(rename = "type")]
    frame_type: String,
    protocol: u32,
    plugin_id: String,
    request_id: String,
    features: Vec<String>,
}

fn validate_hello_ack(
    config: &ProcessObservationExtensionConfig,
    request_id: &str,
    ack: HelloAck,
) -> Result<(), InvocationError> {
    let expected = config.features.iter().collect::<BTreeSet<_>>();
    let actual = ack.features.iter().collect::<BTreeSet<_>>();
    if ack.frame_type != "hello_ack"
        || ack.protocol != PROTOCOL
        || ack.plugin_id != config.plugin_id.as_str()
        || ack.request_id != request_id
        || expected != actual
        || actual.len() != ack.features.len()
    {
        return Err(InvocationError::Protocol("extension handshake mismatch"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluateResult {
    #[serde(rename = "type")]
    frame_type: String,
    protocol: u32,
    plugin_id: String,
    request_id: String,
    metrics: Vec<WireMetric>,
    #[serde(deserialize_with = "required_nullable")]
    pane: Option<WirePane>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireMetric {
    label: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePane {
    title: String,
    rows: Vec<WireMetric>,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn validate_result(
    config: &ProcessObservationExtensionConfig,
    request_id: &str,
    response: EvaluateResult,
) -> Result<ObservationExtensionOutput, InvocationError> {
    if response.frame_type != "evaluate_result"
        || response.protocol != PROTOCOL
        || response.plugin_id != config.plugin_id.as_str()
        || response.request_id != request_id
        || response.metrics.len() > MAX_ROWS
    {
        return Err(InvocationError::Protocol(
            "extension result identity or limits are invalid",
        ));
    }
    validate_rows(&response.metrics)?;
    let pane = match response.pane {
        Some(pane) => {
            if !config
                .features
                .iter()
                .any(|feature| feature == "ui-contribution/1")
                || pane.rows.len() > MAX_ROWS
                || pane.title.len() > MAX_TITLE_BYTES
            {
                return Err(InvocationError::Protocol(
                    "extension pane is not permitted or exceeds limits",
                ));
            }
            validate_rows(&pane.rows)?;
            Some(PaneModel {
                title: pane.title,
                rows: pane
                    .rows
                    .into_iter()
                    .map(|row| MetricRow {
                        label: row.label,
                        value: row.value,
                    })
                    .collect(),
            })
        }
        None if config
            .features
            .iter()
            .any(|feature| feature == "ui-contribution/1") =>
        {
            return Err(InvocationError::Protocol(
                "declared extension pane is missing",
            ));
        }
        None => None,
    };
    Ok(ObservationExtensionOutput {
        metrics: response
            .metrics
            .into_iter()
            .map(|row| MetricRow {
                label: row.label,
                value: row.value,
            })
            .collect(),
        pane,
    })
}

fn validate_rows(rows: &[WireMetric]) -> Result<(), InvocationError> {
    if rows.len() > MAX_ROWS
        || rows
            .iter()
            .any(|row| row.label.len() > MAX_LABEL_BYTES || row.value.len() > MAX_VALUE_BYTES)
    {
        return Err(InvocationError::Protocol(
            "extension output exceeds row limits",
        ));
    }
    Ok(())
}

fn parse_strict<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
    message: &'static str,
) -> Result<T, InvocationError> {
    serde_json::from_slice(bytes).map_err(|_| InvocationError::Protocol(message))
}

fn next_request_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{now}-{sequence}", std::process::id())
}

#[derive(Debug)]
enum InvocationError {
    Timeout,
    Launch,
    #[cfg(unix)]
    Sandbox,
    Pipe,
    Exit,
    #[cfg(not(unix))]
    UnsupportedPlatform,
    Io,
    Protocol(&'static str),
}

impl std::fmt::Display for InvocationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Timeout => "extension invocation exceeded its deadline",
            Self::Launch => "extension process could not be started",
            #[cfg(unix)]
            Self::Sandbox => "connector OS isolation could not be established",
            Self::Pipe => "extension process pipes are unavailable",
            Self::Exit => "extension process exited unsuccessfully",
            #[cfg(not(unix))]
            Self::UnsupportedPlatform => "process extensions require Unix process groups",
            Self::Io => "extension process transport failed",
            Self::Protocol(message) => message,
        })
    }
}

impl std::error::Error for InvocationError {}

fn verify_artifact(
    path: &Path,
    expected: &str,
    max_bytes: u64,
    deadline: Option<Instant>,
) -> Result<(), InvocationError> {
    let file = File::open(path)
        .map_err(|_| InvocationError::Protocol("extension executable is unavailable"))?;
    verify_artifact_file(&file, expected, max_bytes, deadline)
}

fn verify_artifact_file(
    file: &File,
    expected: &str,
    max_bytes: u64,
    deadline: Option<Instant>,
) -> Result<(), InvocationError> {
    validate_sha256(expected)
        .map_err(|_| InvocationError::Protocol("extension SHA-256 pin is invalid"))?;
    let metadata = file
        .metadata()
        .map_err(|_| InvocationError::Protocol("extension executable metadata is unavailable"))?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(InvocationError::Protocol(
            "extension executable is not a bounded regular file",
        ));
    }
    if deadline.is_some_and(|end| Instant::now() >= end) {
        return Err(InvocationError::Timeout);
    }
    let mut reader = file;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if deadline.is_some_and(|end| Instant::now() >= end) {
            return Err(InvocationError::Timeout);
        }
        let count = reader
            .read(&mut buffer)
            .map_err(|_| InvocationError::Protocol("extension executable could not be read"))?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > max_bytes {
            return Err(InvocationError::Protocol(
                "extension executable exceeds its size limit",
            ));
        }
        hasher.update(&buffer[..count]);
    }
    let actual = hasher.finalize();
    let actual = actual
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual != expected {
        return Err(InvocationError::Protocol(
            "extension executable SHA-256 mismatch",
        ));
    }
    if deadline.is_some_and(|end| Instant::now() >= end) {
        return Err(InvocationError::Timeout);
    }
    Ok(())
}

#[cfg(unix)]
fn set_nonblocking(fd: i32) -> io::Result<()> {
    // SAFETY: `fcntl` reads and writes descriptor flags for a live pipe FD.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: same live descriptor; flags are the existing flags plus O_NONBLOCK.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn write_frame_deadline(fd: i32, payload: &[u8], deadline: Instant) -> Result<(), InvocationError> {
    if payload.is_empty() || payload.len() > FRAME_LIMIT {
        return Err(InvocationError::Protocol("extension frame exceeds limits"));
    }
    let size = u32::try_from(payload.len())
        .map_err(|_| InvocationError::Protocol("extension frame exceeds limits"))?;
    let mut bytes = Vec::with_capacity(payload.len() + 4);
    bytes.extend_from_slice(&size.to_be_bytes());
    bytes.extend_from_slice(payload);
    let mut offset = 0;
    while offset < bytes.len() {
        poll_fd(fd, libc::POLLOUT, deadline)?;
        // SAFETY: buffer points to initialized bytes and fd is the child stdin pipe.
        let written =
            unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        if written > 0 {
            offset += written as usize;
            continue;
        }
        if written == 0 {
            return Err(InvocationError::Io);
        }
        let error = io::Error::last_os_error();
        if !matches!(
            error.kind(),
            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
        ) {
            return Err(InvocationError::Io);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_frame_deadline(fd: i32, deadline: Instant) -> Result<Vec<u8>, InvocationError> {
    let mut prefix = [0u8; 4];
    read_exact_deadline(fd, &mut prefix, deadline)?;
    let len = u32::from_be_bytes(prefix) as usize;
    if len == 0 || len > FRAME_LIMIT {
        return Err(InvocationError::Protocol(
            "extension frame length is invalid",
        ));
    }
    let mut payload = vec![0; len];
    read_exact_deadline(fd, &mut payload, deadline)?;
    Ok(payload)
}

#[cfg(unix)]
fn read_exact_deadline(
    fd: i32,
    mut target: &mut [u8],
    deadline: Instant,
) -> Result<(), InvocationError> {
    while !target.is_empty() {
        poll_fd(fd, libc::POLLIN, deadline)?;
        // SAFETY: target is a writable slice and fd is the child stdout pipe.
        let count = unsafe { libc::read(fd, target.as_mut_ptr().cast(), target.len()) };
        if count > 0 {
            let (_, rest) = target.split_at_mut(count as usize);
            target = rest;
            continue;
        }
        if count == 0 {
            return Err(InvocationError::Protocol("extension frame ended early"));
        }
        let error = io::Error::last_os_error();
        if !matches!(
            error.kind(),
            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
        ) {
            return Err(InvocationError::Io);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn require_eof_deadline(fd: i32, deadline: Instant) -> Result<(), InvocationError> {
    loop {
        poll_fd(fd, libc::POLLIN, deadline)?;
        let mut byte = [0u8; 1];
        // SAFETY: byte is writable and fd is the child stdout pipe.
        let count = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), 1) };
        match count {
            0 => return Ok(()),
            1 => {
                return Err(InvocationError::Protocol(
                    "extension emitted an extra frame",
                ));
            }
            _ => {
                let error = io::Error::last_os_error();
                if !matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) {
                    return Err(InvocationError::Io);
                }
            }
        }
    }
}

#[cfg(unix)]
fn poll_fd(fd: i32, events: i16, deadline: Instant) -> Result<(), InvocationError> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(InvocationError::Timeout);
        }
        let millis = deadline
            .saturating_duration_since(now)
            .as_millis()
            .clamp(1, 50) as i32;
        let mut poll = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: poll receives one initialized pollfd for the duration of the call.
        let status = unsafe { libc::poll(&mut poll, 1, millis) };
        if status > 0 {
            if poll.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
                return Err(InvocationError::Io);
            }
            if poll.revents & (events | libc::POLLHUP) != 0 {
                return Ok(());
            }
        } else if status < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(InvocationError::Io);
            }
        }
    }
}

#[cfg(unix)]
fn wait_child_deadline(child: &mut Child, deadline: Instant) -> Result<(), InvocationError> {
    loop {
        if child.try_wait().map_err(|_| InvocationError::Io)?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(InvocationError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct GuardedChild(Option<Child>);

impl Drop for GuardedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            #[cfg(unix)]
            {
                // SAFETY: negative child PID targets its process group.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            }
            let _ = child.kill();
            if child.try_wait().ok().flatten().is_none() {
                enqueue_for_reaping(child);
            }
        }
    }
}

fn enqueue_for_reaping(child: Child) {
    let state = CHILD_REAPER.get_or_init(|| Mutex::new(ReaperState::default()));
    let mut state = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.pending.push(child);
    dispatch_pending(&mut state, start_reaper);
}

#[derive(Default)]
struct ReaperState {
    sender: Option<mpsc::Sender<Child>>,
    pending: Vec<Child>,
}

fn start_reaper(state: &mut ReaperState) -> bool {
    ensure_reaper_with(state, |receiver| {
        std::thread::Builder::new()
            .name("agenturm-extension-reaper".into())
            .spawn(move || reap_children(receiver))
            .map(|_| ())
    })
}

fn ensure_reaper_with(
    state: &mut ReaperState,
    spawn: impl FnOnce(mpsc::Receiver<Child>) -> io::Result<()>,
) -> bool {
    if state.sender.is_some() {
        return true;
    }
    let (sender, receiver) = mpsc::channel();
    if spawn(receiver).is_err() {
        return false;
    }
    state.sender = Some(sender);
    true
}

fn dispatch_pending(state: &mut ReaperState, start: impl Fn(&mut ReaperState) -> bool) {
    if state.pending.is_empty() || (state.sender.is_none() && !start(state)) {
        return;
    }
    let Some(sender) = state.sender.clone() else {
        return;
    };
    let mut pending = std::mem::take(&mut state.pending).into_iter();
    while let Some(child) = pending.next() {
        if let Err(error) = sender.send(child) {
            state.sender = None;
            state.pending.push(error.0);
            state.pending.extend(pending);
            break;
        }
    }
}

fn reap_children(receiver: mpsc::Receiver<Child>) {
    let mut children = Vec::new();
    loop {
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(child) => children.push(child),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(child) = receiver.try_recv() {
            children.push(child);
        }
        children.retain_mut(|child| match child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) | Err(_) => true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{ObservationIdentity, ObservationScope};

    fn observation(kind: ObservationKind) -> Observation {
        Observation {
            harness_id: ExtensionId::parse("harness.pi").unwrap(),
            event_id: "private-event-id".into(),
            revision: 7,
            identity: ObservationIdentity {
                scope: ObservationScope::Session,
                session_id: Some("private-session".into()),
                worker_id: Some("private-worker".into()),
                turn_id: Some("private-turn".into()),
                operation_id: Some("private-operation".into()),
            },
            observed_at_unix_ns: u64::MAX,
            kind,
        }
    }

    #[test]
    fn projection_contains_only_envelope_and_granted_typed_fields() {
        let usage = observation(ObservationKind::Usage {
            input_tokens: Some(11),
            output_tokens: Some(12),
            cache_read_tokens: None,
            cache_write_tokens: Some(3),
            total_tokens: Some(26),
        });
        let grants = BTreeSet::from([ExtensionDataGrant::Usage]);
        let request = make_evaluate_request(
            &config(vec!["capability/1"], grants),
            &ObservationExtensionInput {
                observations: &[usage],
                snapshot_id: 19,
                observations_truncated: true,
                selection_scope: agenturm_kernel::ObservationSelectionScope::Filtered,
            },
            "request-1".into(),
        )
        .unwrap();
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("18446744073709551615"));
        assert!(encoded.contains("\"input_tokens\":11"));
        assert!(!encoded.contains("private-event-id"));
        assert!(!encoded.contains("private-session"));
        assert!(!encoded.contains("private-worker"));
        assert!(!encoded.contains("private-turn"));
        assert!(!encoded.contains("private-operation"));
        assert_eq!(request.observation_count, 1);
        assert!(request.truncated);
    }

    #[test]
    fn grant_filter_leaves_only_mandatory_envelope_for_other_kinds() {
        let message = observation(ObservationKind::Message {
            role: "user".into(),
        });
        let projected = project_observation(&message, &BTreeSet::from([ExtensionDataGrant::Usage]));
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(encoded.contains("\"kind\":\"message\""));
        assert!(!encoded.contains("\"role\""));
    }

    #[test]
    fn crop_keeps_newest_five_hundred_and_sets_truncation() {
        let observations = (0..501)
            .map(|index| {
                let mut item = observation(ObservationKind::Activity {
                    category: "work".into(),
                });
                item.observed_at_unix_ns = index;
                item
            })
            .collect::<Vec<_>>();
        let request = make_evaluate_request(
            &config(
                vec!["capability/1"],
                BTreeSet::from([ExtensionDataGrant::Usage]),
            ),
            &ObservationExtensionInput {
                observations: &observations,
                snapshot_id: 1,
                observations_truncated: false,
                selection_scope: agenturm_kernel::ObservationSelectionScope::Latest,
            },
            "request-2".into(),
        )
        .unwrap();
        assert_eq!(request.observations.len(), MAX_OBSERVATIONS);
        assert_eq!(request.observation_count, 501);
        assert_eq!(request.observations[0].observed_at_unix_ns, "1");
        assert!(request.truncated);
    }

    #[test]
    fn response_rejects_missing_pane_field_and_unknown_fields() {
        assert!(parse_strict::<EvaluateResult>(
            br#"{"type":"evaluate_result","protocol":1,"plugin_id":"example.x","request_id":"r","metrics":[]}"#,
            "bad",
        )
        .is_err());
        assert!(parse_strict::<EvaluateResult>(
            br#"{"type":"evaluate_result","protocol":1,"plugin_id":"example.x","request_id":"r","metrics":[],"pane":null,"extra":true}"#,
            "bad",
        )
        .is_err());
    }

    #[test]
    fn failed_reaper_start_keeps_child_for_a_later_retry() {
        let child = std::process::Command::new("/bin/sleep")
            .arg("0.01")
            .spawn()
            .unwrap();
        let mut state = ReaperState {
            sender: None,
            pending: vec![child],
        };
        assert!(!ensure_reaper_with(&mut state, |_receiver| {
            Err(io::Error::other("injected thread creation failure"))
        }));
        assert!(state.sender.is_none());
        assert_eq!(state.pending.len(), 1);

        let started = ensure_reaper_with(&mut state, |receiver| {
            std::thread::Builder::new()
                .spawn(move || reap_children(receiver))
                .map(|_| ())
        });
        assert!(started);
        dispatch_pending(&mut state, start_reaper);
        assert!(state.pending.is_empty());
        assert!(state.sender.is_some());
    }

    fn config(
        features: Vec<&str>,
        grants: BTreeSet<ExtensionDataGrant>,
    ) -> ProcessObservationExtensionConfig {
        ProcessObservationExtensionConfig {
            plugin_id: ExtensionId::parse("example.extension").unwrap(),
            executable: PathBuf::from("/tmp/plugin"),
            sha256: "a".repeat(64),
            features: features.into_iter().map(str::to_owned).collect(),
            grants,
        }
    }
}
