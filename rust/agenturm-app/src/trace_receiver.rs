//! Loopback-only HTTP ingestion for bounded OTLP trace, metric and activity log exports.

mod logs;
mod metrics;

use crate::trace_import::{
    MAX_EXPORT_BYTES, commit, commit_payloads, decode_json, decode_protobuf, parse_source_id,
    protobuf_response,
};
use crate::trace_progress;
use agenturm_kernel::{
    ExtensionId, Observation,
    native_attention::{NativeAttentionCapture, NativeAttentionReceipt, valid_history_binding},
};
use agenturm_store::{TelemetryBinding, TelemetryPayloadRecord};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

const USAGE: &str = "agenturm serve-traces --listen <loopback-IP:port> --db <path> --source-id otlp.<id> [--payload-scope <standalone|harness-id> --token-file <path>]";
#[cfg(target_os = "linux")]
const BROKER_USAGE: &str = "agenturm broker serve-traces --broker-config <json> --listen <loopback-IP:port> --source-id otlp.<id> --token-file <path> [--payload-scope <standalone|harness-id>] [--native-attention-source <harness-id>]";
pub(super) const NATIVE_ATTENTION_PATH: &str = "/v1/native-attention";
const MAX_NATIVE_ATTENTION_BYTES: usize = 8 * 1024;

type NativeAttentionAdmission<'a> =
    dyn FnMut(&ExtensionId, &NativeAttentionCapture) -> Result<NativeAttentionReceipt, String> + 'a;
const MAX_HEADERS: usize = 8192;
const READ_BUDGET: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceReceiverOptions {
    settings: TraceReceiverSettings,
    db: PathBuf,
    payload_token_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TraceReceiverSettings {
    listen: SocketAddr,
    pub source: ExtensionId,
    pub payload_binding: Option<TelemetryBinding>,
    native_attention_source: Option<ExtensionId>,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct BrokerTraceReceiverOptions {
    pub settings: TraceReceiverSettings,
    pub token_file: PathBuf,
}

#[derive(Clone)]
pub(super) struct ProducerToken(String);

impl ProducerToken {
    #[cfg(target_os = "linux")]
    pub(super) fn from_file(path: &std::path::Path) -> Result<Self, String> {
        const INVALID: &str =
            "Bearer token file must be an owner-only regular file with 64 lowercase hex digits";
        let token = Self::read_private_token(path, 65, INVALID)?;
        let token = token.strip_suffix('\n').unwrap_or(&token);
        if token.len() != 64
            || !token
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(INVALID.into());
        }
        Ok(Self(token.to_owned()))
    }

    #[cfg(target_os = "linux")]
    pub(super) fn from_python_receiver_file(path: &std::path::Path) -> Result<Self, String> {
        const INVALID: &str = "Python receiver token file must be an owner-only regular file containing a URL-safe bearer token";
        let token = Self::read_private_token(path, 258, INVALID)?;
        let token = token.trim_matches([' ', '\t', '\r', '\n']);
        if !(32..=256).contains(&token.len())
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(INVALID.into());
        }
        Ok(Self(token.to_owned()))
    }

    #[cfg(target_os = "linux")]
    fn read_private_token(
        path: &std::path::Path,
        limit: usize,
        invalid: &str,
    ) -> Result<String, String> {
        use std::fs::OpenOptions;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| invalid)?;
        let metadata = file.metadata().map_err(|_| invalid)?;
        // SAFETY: geteuid takes no arguments and has no memory preconditions.
        let owner = unsafe { libc::geteuid() };
        if !metadata.is_file() || metadata.uid() != owner || metadata.mode() & 0o077 != 0 {
            return Err(invalid.into());
        }
        let mut token = String::new();
        file.take((limit + 1) as u64)
            .read_to_string(&mut token)
            .map_err(|_| invalid)?;
        if token.len() > limit {
            return Err(invalid.into());
        }
        Ok(token)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn generate() -> Result<Self, String> {
        use std::fmt::Write as _;

        let mut random = [0; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut random))
            .map_err(|_| "Cannot generate a private OTLP launch token")?;
        let mut token = String::with_capacity(64);
        for byte in random {
            write!(token, "{byte:02x}").expect("formatting a token into memory");
        }
        Ok(Self(token))
    }

    pub(super) fn otel_header(&self) -> String {
        format!("authorization=Bearer%20{}", self.0)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn bearer_header(&self) -> String {
        format!("Bearer {}", self.0)
    }

    pub(super) fn authorizes(&self, header: &str) -> bool {
        let Some((scheme, provided)) = header.split_once(' ') else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case("Bearer") || provided.len() != self.0.len() {
            return false;
        }
        // Check the full fixed-width credential without a short-circuit on
        // individual bytes.
        self.0
            .bytes()
            .zip(provided.bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
    }
}

pub(super) fn parse(args: &[String]) -> Result<TraceReceiverOptions, String> {
    let (settings, db, payload_token_file) = parse_settings(args, true, USAGE)?;
    if settings.payload_binding.is_some() != payload_token_file.is_some() {
        return Err(USAGE.into());
    }
    Ok(TraceReceiverOptions {
        settings,
        db,
        payload_token_file,
    })
}

#[cfg(target_os = "linux")]
pub(super) fn parse_broker(args: &[String]) -> Result<BrokerTraceReceiverOptions, String> {
    let (settings, _, token_file) = parse_settings(args, false, BROKER_USAGE)?;
    Ok(BrokerTraceReceiverOptions {
        settings,
        token_file: token_file.ok_or(BROKER_USAGE)?,
    })
}

fn parse_settings(
    args: &[String],
    local: bool,
    usage: &str,
) -> Result<(TraceReceiverSettings, PathBuf, Option<PathBuf>), String> {
    let (pairs, extras) = args.as_chunks::<2>();
    if !extras.is_empty() {
        return Err(usage.into());
    }
    let (mut listen, mut path, mut source) = (None, None, None);
    let mut scope = None;
    let mut token_file = None;
    let mut native_attention_source = None;
    for [flag, value] in pairs {
        let value = value.as_str();
        if value.is_empty() || value.starts_with("--") {
            return Err(usage.into());
        }
        match flag.as_str() {
            "--listen" if listen.is_none() => {
                let address = value.parse::<SocketAddr>().map_err(|_| usage)?;
                if !address.ip().is_loopback() {
                    return Err("trace receiver requires a loopback IP".into());
                }
                listen = Some(address);
            }
            "--db" if local && path.is_none() && value != "-" && value != ":memory:" => {
                path = Some(PathBuf::from(value))
            }
            "--token-file" if token_file.is_none() => token_file = Some(PathBuf::from(value)),
            "--payload-scope" if scope.is_none() => scope = Some(value),
            "--native-attention-source" if !local && native_attention_source.is_none() => {
                let parsed =
                    ExtensionId::parse(value).map_err(|_| "Invalid native attention source")?;
                if value.starts_with("otlp.") || value.starts_with("host.") {
                    return Err("Native attention requires an explicit native source".into());
                }
                native_attention_source = Some(parsed);
            }
            "--source-id" if source.is_none() => source = Some(parse_source_id(value)?),
            _ => return Err(usage.into()),
        }
    }
    let source = source.ok_or(usage)?;
    let payload_binding = scope
        .map(|scope| parse_payload_scope(&source, scope))
        .transpose()?;
    Ok((
        TraceReceiverSettings {
            listen: listen.ok_or(usage)?,
            source,
            payload_binding,
            native_attention_source,
        },
        if local {
            path.ok_or(usage)?
        } else {
            PathBuf::new()
        },
        token_file,
    ))
}

pub(super) fn parse_payload_scope(
    source: &ExtensionId,
    scope: &str,
) -> Result<TelemetryBinding, String> {
    let native_source_id = if scope == "standalone" {
        None
    } else {
        Some(ExtensionId::parse(scope).map_err(|_| "invalid telemetry payload scope")?)
    };
    let binding = TelemetryBinding {
        source_id: source.clone(),
        native_source_id,
    };
    binding.grant_scope().map_err(|error| error.to_string())?;
    Ok(binding)
}

pub fn execute_trace_receiver(options: &TraceReceiverOptions) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    let token = options
        .payload_token_file
        .as_deref()
        .map(ProducerToken::from_file)
        .transpose()?;
    #[cfg(not(target_os = "linux"))]
    let token = if options.payload_token_file.is_some() {
        return Err("Private trace capture requires Linux producer authentication".into());
    } else {
        None
    };
    execute_with_maintenance(
        &options.settings,
        token,
        Some(options.db.clone()),
        |source, rows, payloads| match &options.settings.payload_binding {
            Some(binding) => commit_payloads(&options.db, binding, &rows, &payloads).map(|_| ()),
            None => commit(&options.db, source, &rows).map(|_| ()),
        },
        None,
    )
}

#[cfg(target_os = "linux")]
pub(super) fn execute_with_commit(
    settings: &TraceReceiverSettings,
    token: Option<ProducerToken>,
    commit: impl FnMut(
        &ExtensionId,
        Vec<Observation>,
        Vec<TelemetryPayloadRecord>,
    ) -> Result<(), String>,
    mut admit_native: impl FnMut(
        &ExtensionId,
        &NativeAttentionCapture,
    ) -> Result<NativeAttentionReceipt, String>,
) -> Result<(), String> {
    execute_with_maintenance(settings, token, None, commit, Some(&mut admit_native))
}

fn execute_with_maintenance(
    settings: &TraceReceiverSettings,
    token: Option<ProducerToken>,
    maintenance_db: Option<PathBuf>,
    commit: impl FnMut(
        &ExtensionId,
        Vec<Observation>,
        Vec<TelemetryPayloadRecord>,
    ) -> Result<(), String>,
    admit_native: Option<&mut NativeAttentionAdmission<'_>>,
) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = crate::WatchSignalRegistrations::new(Arc::clone(&stop))?;
    let receiver =
        TraceReceiver::bind(settings.clone(), None, token)?.with_local_maintenance(maintenance_db);
    println!(
        "Listening for OTLP traces at http://{}/v1/traces",
        receiver.address()
    );
    println!(
        "Listening for OTLP metrics at http://{}/v1/metrics",
        receiver.address()
    );
    println!(
        "Listening for OTLP activity logs at http://{}/v1/logs",
        receiver.address()
    );
    if receiver.native_attention_source.is_some() {
        println!(
            "Listening for native attention at http://{}{NATIVE_ATTENTION_PATH}",
            receiver.address()
        );
    }
    receiver.serve_with_admission(&stop, commit, admit_native)
}

pub(super) struct TraceReceiver {
    maintenance: Option<super::retention_cli::LocalRetentionMaintenance>,
    listener: TcpListener,
    address: SocketAddr,
    source: ExtensionId,
    launch_session: Option<String>,
    token: Option<ProducerToken>,
    payload_binding: Option<TelemetryBinding>,
    native_attention_source: Option<ExtensionId>,
}

impl TraceReceiver {
    fn bind(
        settings: TraceReceiverSettings,
        launch_session: Option<String>,
        token: Option<ProducerToken>,
    ) -> Result<Self, String> {
        if settings.payload_binding.is_some() && token.is_none() {
            return Err("Private trace capture requires an authenticated producer".into());
        }
        if settings.native_attention_source.is_some() && token.is_none() {
            return Err("Native attention capture requires an authenticated producer".into());
        }
        let listener = TcpListener::bind(settings.listen)
            .map_err(|error| format!("bind trace receiver: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        Ok(Self {
            maintenance: None,
            listener,
            address,
            source: settings.source,
            launch_session,
            token,
            payload_binding: settings.payload_binding,
            native_attention_source: settings.native_attention_source,
        })
    }

    pub(super) fn bind_ephemeral(
        source: ExtensionId,
        launch_session: String,
        token: Option<ProducerToken>,
        payload_binding: Option<TelemetryBinding>,
    ) -> Result<Self, String> {
        Self::bind(
            TraceReceiverSettings {
                listen: SocketAddr::from(([127, 0, 0, 1], 0)),
                source,
                payload_binding,
                native_attention_source: None,
            },
            Some(launch_session),
            token,
        )
    }

    pub(super) fn bind_cockpit() -> Result<Self, String> {
        let port = std::env::var_os("AGENTURM_OTLP_PORT")
            .map(|value| {
                value
                    .to_str()
                    .and_then(|text| text.parse::<u16>().ok())
                    .filter(|port| *port > 0)
                    .ok_or("AGENTURM_OTLP_PORT must name a valid port")
            })
            .transpose()?
            .unwrap_or(14318);
        Self::bind(
            TraceReceiverSettings {
                listen: SocketAddr::from(([127, 0, 0, 1], port)),
                source: parse_source_id("otlp.local")?,
                payload_binding: None,
                native_attention_source: None,
            },
            None,
            None,
        )
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(super) fn with_local_maintenance(mut self, db: Option<PathBuf>) -> Self {
        self.maintenance = db.map(super::retention_cli::LocalRetentionMaintenance::new);
        self
    }

    pub(super) fn serve_with(
        self,
        stop: &AtomicBool,
        mut store: impl FnMut(&ExtensionId, Vec<Observation>) -> Result<(), String>,
    ) -> Result<(), String> {
        self.serve_with_payloads(stop, |source, rows, _| store(source, rows))
    }

    pub(super) fn serve_with_payloads(
        self,
        stop: &AtomicBool,
        store: impl FnMut(
            &ExtensionId,
            Vec<Observation>,
            Vec<TelemetryPayloadRecord>,
        ) -> Result<(), String>,
    ) -> Result<(), String> {
        self.serve_with_admission(stop, store, None)
    }

    fn serve_with_admission(
        mut self,
        stop: &AtomicBool,
        mut store: impl FnMut(
            &ExtensionId,
            Vec<Observation>,
            Vec<TelemetryPayloadRecord>,
        ) -> Result<(), String>,
        mut admit_native: Option<&mut NativeAttentionAdmission<'_>>,
    ) -> Result<(), String> {
        while !stop.load(Ordering::Relaxed) {
            if let Some(maintenance) = self.maintenance.as_mut() {
                maintenance.tick();
            }
            match self.listener.accept() {
                Ok((stream, peer)) if peer.ip().is_loopback() => {
                    handle(stream, &self, &mut store, &mut admit_native)
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Err(error) => return Err(format!("trace receiver accept failed: {error}")),
            }
        }
        Ok(())
    }
}

fn handle(
    mut stream: TcpStream,
    receiver: &TraceReceiver,
    commit: &mut impl FnMut(
        &ExtensionId,
        Vec<Observation>,
        Vec<TelemetryPayloadRecord>,
    ) -> Result<(), String>,
    admit_native: &mut Option<&mut NativeAttentionAdmission<'_>>,
) {
    let source = &receiver.source;
    let launch_session = receiver.launch_session.as_deref();
    let binding = receiver.payload_binding.as_ref();
    let deadline = Instant::now() + READ_BUDGET;
    let result = read_request(&stream, receiver.address, receiver.token.as_ref(), deadline)
        .and_then(|(request, body)| {
            let encoding = request.encoding;
            if encoding == Encoding::NativeAttention {
                let target = receiver
                    .native_attention_source
                    .as_ref()
                    .ok_or("404 Not Found")?;
                let admit = admit_native.as_deref_mut().ok_or("404 Not Found")?;
                let history = request.history_binding.ok_or("400 Bad Request")?;
                let reply = ingest_native_attention(admit, source, target, &history, &body)?;
                return Ok((encoding, reply, Some(history)));
            }
            let decoded = match encoding {
                Encoding::NativeAttention => unreachable!("handled above"),
                Encoding::Json => decode_json(&body, source, launch_session, binding),
                Encoding::Protobuf => decode_protobuf(&body, source, launch_session, binding),
                Encoding::ProgressJson => trace_progress::decode(&body, source, launch_session)
                    .map(|row| (vec![row], 0, Vec::new()))
                    .map_err(str::to_owned),
                Encoding::LogsJson => logs::decode_json(&body, source),
                Encoding::LogsProtobuf => logs::decode_protobuf(&body, source),
                Encoding::MetricsJson => {
                    metrics::decode_json(&body, source).map(|rows| (rows, 0, Vec::new()))
                }
                Encoding::MetricsProtobuf => {
                    metrics::decode_protobuf(&body, source).map(|rows| (rows, 0, Vec::new()))
                }
            };
            let (rows, skipped, payloads) = decoded.map_err(|_| "400 Bad Request")?;
            if !rows.is_empty() {
                commit(source, rows, payloads).map_err(|_| "503 Service Unavailable")?;
            }
            let bytes = match encoding {
                Encoding::Json if skipped == 0 => b"{}".to_vec(),
                Encoding::Json => serde_json::json!({"partialSuccess": {
                    "rejectedSpans": skipped.to_string(),
                    "errorMessage": "Span lacks an explicit session identity",
                }})
                .to_string()
                .into_bytes(),
                Encoding::Protobuf => protobuf_response(skipped),
                Encoding::LogsJson => logs::json_response(skipped),
                Encoding::LogsProtobuf => logs::protobuf_response(skipped),
                Encoding::NativeAttention => unreachable!("handled above"),
                Encoding::ProgressJson | Encoding::MetricsJson => b"{}".to_vec(),
                Encoding::MetricsProtobuf => Vec::new(),
            };
            Ok((encoding, bytes, None))
        });
    let (status, encoding, response, history) = match result {
        Ok((encoding, response, history)) => ("200 OK", encoding, response, history),
        Err(status) => (
            status,
            Encoding::Json,
            serde_json::json!({"message":status})
                .to_string()
                .into_bytes(),
            None,
        ),
    };
    let history_header = history.map_or_else(String::new, |binding| {
        format!("X-Agenturm-History-Binding: {binding}\r\n")
    });
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    if write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {}\r\nConnection: close\r\nContent-Length: {}\r\n{history_header}\r\n",
        encoding.media_type(),
        response.len(),
    )
    .is_ok()
    {
        let _ = stream.write_all(&response);
    }
}

fn ingest_native_attention(
    admit: &mut NativeAttentionAdmission<'_>,
    source: &ExtensionId,
    target: &ExtensionId,
    history: &str,
    body: &[u8],
) -> Result<Vec<u8>, &'static str> {
    let capture: NativeAttentionCapture =
        serde_json::from_slice(body).map_err(|_| "400 Bad Request")?;
    if !capture.is_valid() {
        return Err("400 Bad Request");
    }
    if capture.history_binding != history {
        return Err("409 Conflict");
    }
    if capture.scope.source_id != target.as_str() {
        return Err("403 Forbidden");
    }
    let receipt = admit(source, &capture).map_err(|_| "503 Service Unavailable")?;
    if receipt.history_binding != history
        || receipt.event_id
            != agenturm_store::native_attention::canonical_event_id(source, &capture)
    {
        return Err("503 Service Unavailable");
    }
    serde_json::to_vec(&receipt).map_err(|_| "503 Service Unavailable")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Encoding {
    Json,
    Protobuf,
    ProgressJson,
    MetricsJson,
    MetricsProtobuf,
    LogsJson,
    LogsProtobuf,
    NativeAttention,
}

impl Encoding {
    fn media_type(self) -> &'static str {
        match self {
            Self::Json
            | Self::ProgressJson
            | Self::MetricsJson
            | Self::LogsJson
            | Self::NativeAttention => "application/json",
            Self::Protobuf | Self::MetricsProtobuf | Self::LogsProtobuf => "application/x-protobuf",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct RequestHead {
    encoding: Encoding,
    length: usize,
    history_binding: Option<String>,
}

fn read_request(
    stream: &TcpStream,
    address: SocketAddr,
    token: Option<&ProducerToken>,
    deadline: Instant,
) -> Result<(RequestHead, Vec<u8>), &'static str> {
    let mut reader = BufReader::new(stream);
    let mut raw = Vec::new();
    while !raw.ends_with(b"\r\n\r\n") {
        if raw.len() >= MAX_HEADERS {
            return Err("431 Request Header Fields Too Large");
        }
        timeout(stream, deadline)?;
        let chunk = reader.fill_buf().map_err(|_| "408 Request Timeout")?;
        let Some(first) = chunk.first() else {
            return Err("400 Bad Request");
        };
        raw.push(*first);
        reader.consume(1);
    }
    let headers = std::str::from_utf8(&raw).map_err(|_| "400 Bad Request")?;
    let request = parse_headers(headers, address, token)?;
    let length = request.length;
    let mut body = vec![0; length];
    let mut consumed = 0;
    while consumed < length {
        timeout(stream, deadline)?;
        let count = reader
            .read(&mut body[consumed..])
            .map_err(|_| "408 Request Timeout")?;
        if count == 0 {
            return Err("400 Bad Request");
        }
        consumed += count;
    }
    Ok((request, body))
}

fn timeout(stream: &TcpStream, deadline: Instant) -> Result<(), &'static str> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or("408 Request Timeout")?;
    stream
        .set_read_timeout(Some(remaining))
        .map_err(|_| "408 Request Timeout")
}

fn parse_headers(
    headers: &str,
    address: SocketAddr,
    token: Option<&ProducerToken>,
) -> Result<RequestHead, &'static str> {
    let mut lines = headers.split("\r\n");
    let request = lines.next().ok_or("404 Not Found")?;
    let (path, version) = request
        .strip_prefix("POST ")
        .and_then(|request| request.split_once(' '))
        .ok_or("404 Not Found")?;
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err("404 Not Found");
    }
    if !matches!(
        path,
        "/v1/traces" | "/v1/metrics" | "/v1/logs" | trace_progress::PATH | NATIVE_ATTENTION_PATH
    ) {
        return Err("404 Not Found");
    }
    let mut host = false;
    let mut authorization = None;
    let mut history_binding = None;
    let mut encoding = None;
    let mut length = None;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("400 Bad Request")?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        {
            return Err("400 Bad Request");
        }
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "host" if !host => {
                host = value.parse::<SocketAddr>().ok() == Some(address)
                    || value == format!("localhost:{}", address.port());
                if !host {
                    return Err("400 Bad Request");
                }
            }
            "content-type" if encoding.is_none() => {
                let format = [Encoding::Json, Encoding::Protobuf]
                    .into_iter()
                    .find(|format| value.eq_ignore_ascii_case(format.media_type()))
                    .ok_or("415 Unsupported Media Type")?;
                encoding = Some(format);
            }
            "content-length"
                if length.is_none() && value.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                length = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| "413 Content Too Large")?,
                );
            }
            "authorization" if token.is_some() => {
                if authorization.replace(value).is_some() {
                    return Err("401 Unauthorized");
                }
            }
            "x-agenturm-history-binding"
                if path == NATIVE_ATTENTION_PATH
                    && history_binding.is_none()
                    && valid_history_binding(value) =>
            {
                history_binding = Some(value.to_owned());
            }
            "x-agenturm-history-binding" => return Err("400 Bad Request"),
            "origin" | "transfer-encoding" | "content-encoding" | "expect" | "host"
            | "content-length" => {
                return Err("400 Bad Request");
            }
            "content-type" => return Err("415 Unsupported Media Type"),
            _ => {}
        }
    }
    if !host {
        return Err("400 Bad Request");
    }
    if let Some(token) = token
        && !authorization.is_some_and(|value| token.authorizes(value))
    {
        return Err("401 Unauthorized");
    }
    if path == NATIVE_ATTENTION_PATH && token.is_none() {
        return Err("404 Not Found");
    }
    if path == NATIVE_ATTENTION_PATH && history_binding.is_none() {
        return Err("400 Bad Request");
    }
    let encoding = match (path, encoding.ok_or("400 Bad Request")?) {
        (trace_progress::PATH, Encoding::Json) => Encoding::ProgressJson,
        (trace_progress::PATH, _) => return Err("415 Unsupported Media Type"),
        ("/v1/metrics", Encoding::Json) => Encoding::MetricsJson,
        ("/v1/metrics", Encoding::Protobuf) => Encoding::MetricsProtobuf,
        ("/v1/logs", Encoding::Json) => Encoding::LogsJson,
        ("/v1/logs", Encoding::Protobuf) => Encoding::LogsProtobuf,
        (NATIVE_ATTENTION_PATH, Encoding::Json) => Encoding::NativeAttention,
        (NATIVE_ATTENTION_PATH, _) => return Err("415 Unsupported Media Type"),
        (_, encoding) => encoding,
    };
    let length = length.ok_or("411 Length Required")?;
    let max_body = match path {
        trace_progress::PATH => trace_progress::MAX_BYTES as u64,
        NATIVE_ATTENTION_PATH => MAX_NATIVE_ATTENTION_BYTES as u64,
        _ => MAX_EXPORT_BYTES,
    };
    if length as u64 > max_body {
        return Err("413 Content Too Large");
    }
    Ok(RequestHead {
        encoding,
        length,
        history_binding,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_attention_requires_explicit_header_and_authenticated_json() {
        let address = "127.0.0.1:4318".parse().unwrap();
        let token = ProducerToken("ab".repeat(32));
        let authorization = format!("Authorization: Bearer {}\r\n", token.0);
        let binding = format!("{}:{}", "1".repeat(32), "2".repeat(32));
        let tagged = format!("X-Agenturm-History-Binding: {binding}\r\n");
        let head = |media: &str, authorization: &str, tagged: &str| {
            format!(
                "POST {NATIVE_ATTENTION_PATH} HTTP/1.1\r\nHost: 127.0.0.1:4318\r\nContent-Type: {media}\r\nContent-Length: 128\r\n{authorization}{tagged}\r\n"
            )
        };
        let request = head("application/json", &authorization, &tagged);
        assert_eq!(
            parse_headers(&request, address, Some(&token)),
            Ok(RequestHead {
                encoding: Encoding::NativeAttention,
                length: 128,
                history_binding: Some(binding),
            })
        );
        assert_eq!(parse_headers(&request, address, None), Err("404 Not Found"));
        assert_eq!(
            parse_headers(
                &head("application/json", "", &tagged),
                address,
                Some(&token)
            ),
            Err("401 Unauthorized")
        );
        for rejected in [
            "".to_owned(),
            tagged.repeat(2),
            format!("{tagged}X-Agenturm-History-Binding: invalid\r\n"),
        ] {
            assert_eq!(
                parse_headers(
                    &head("application/json", &authorization, &rejected),
                    address,
                    Some(&token)
                ),
                Err("400 Bad Request")
            );
        }
        assert_eq!(
            parse_headers(
                &head("application/x-protobuf", &authorization, &tagged),
                address,
                Some(&token)
            ),
            Err("415 Unsupported Media Type")
        );
    }

    #[test]
    fn native_attention_body_cannot_change_header_history_or_configured_native_source() {
        use agenturm_kernel::native_attention::{NativeAttentionKind, NativeAttentionScope};
        use std::cell::Cell;
        let source = ExtensionId::parse("otlp.lifecycle").unwrap();
        let target = ExtensionId::parse("harness.codex.fixture").unwrap();
        let history = "a".repeat(32);
        let mut capture = NativeAttentionCapture {
            history_binding: history.clone(),
            scope: NativeAttentionScope {
                source_id: target.as_str().into(),
                session_id: "fixture-session".into(),
                turn_id: "fixture-turn".into(),
                worker_id: None,
            },
            sender_id: "b".repeat(32),
            event_id: "fixture-event".into(),
            sequence: 1,
            kind: NativeAttentionKind::ApprovalRequested,
            request_id: Some("fixture-request".into()),
            source_time_unix_ns: None,
        };
        let admissions = Cell::new(0);
        let mut admit = |source: &ExtensionId, capture: &NativeAttentionCapture| {
            admissions.set(admissions.get() + 1);
            Ok(NativeAttentionReceipt {
                history_binding: capture.history_binding.clone(),
                event_id: agenturm_store::native_attention::canonical_event_id(source, capture),
                replayed: false,
            })
        };
        let body = serde_json::to_vec(&capture).unwrap();
        assert_eq!(
            ingest_native_attention(&mut admit, &source, &target, &"b".repeat(32), &body),
            Err("409 Conflict")
        );
        assert_eq!(
            ingest_native_attention(
                &mut admit,
                &source,
                &target,
                &history,
                br#"{"decision":"approve"}"#
            ),
            Err("400 Bad Request")
        );
        capture.scope.source_id = "harness.codex.other".into();
        assert_eq!(
            ingest_native_attention(
                &mut admit,
                &source,
                &target,
                &history,
                &serde_json::to_vec(&capture).unwrap()
            ),
            Err("403 Forbidden")
        );
        assert_eq!(admissions.get(), 0);
        let accepted =
            ingest_native_attention(&mut admit, &source, &target, &history, &body).unwrap();
        let receipt: NativeAttentionReceipt = serde_json::from_slice(&accepted).unwrap();
        assert_eq!(receipt.history_binding, history);
        assert_eq!(admissions.get(), 1);
    }

    #[test]
    fn log_formats_use_the_same_authenticated_route() {
        let address = "127.0.0.1:4318".parse().unwrap();
        let token = ProducerToken("ab".repeat(32));
        let authorized = format!("Authorization: Bearer {}\r\n", token.0);
        let head = |media: &str, authorization: &str| {
            format!(
                "POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:4318\r\nContent-Type: {media}\r\nContent-Length: 32\r\n{authorization}\r\n"
            )
        };
        for encoding in [Encoding::LogsJson, Encoding::LogsProtobuf] {
            let headers = |authorization| head(encoding.media_type(), authorization);
            assert_eq!(
                parse_headers(&headers(&authorized), address, Some(&token)),
                Ok(RequestHead {
                    encoding,
                    length: 32,
                    history_binding: None
                })
            );
            assert_eq!(
                parse_headers(&headers(""), address, Some(&token)),
                Err("401 Unauthorized")
            );
            assert_eq!(
                parse_headers(&headers(&authorized.repeat(2)), address, Some(&token)),
                Err("401 Unauthorized")
            );
        }
    }

    #[test]
    fn metrics_requires_existing_producer_authorization_and_exports_replay_once() {
        use agenturm_kernel::retained_history::{MetricHistoryPage, MetricHistoryQuery};
        let address = "127.0.0.1:4318".parse().unwrap();
        let token = ProducerToken("ab".repeat(32));
        let head = |media: &str, auth: &str| {
            format!(
                "POST /v1/metrics HTTP/1.1\r\nHost: 127.0.0.1:4318\r\nContent-Type: {media}\r\nContent-Length: 32\r\n{auth}\r\n",
            )
        };
        let authorized = format!("Authorization: Bearer {}\r\n", token.0);
        assert_eq!(
            parse_headers(
                &head("application/json", &authorized),
                address,
                Some(&token)
            ),
            Ok(RequestHead {
                encoding: Encoding::MetricsJson,
                length: 32,
                history_binding: None
            })
        );
        assert_eq!(
            parse_headers(
                &head("application/x-protobuf", &authorized),
                address,
                Some(&token)
            ),
            Ok(RequestHead {
                encoding: Encoding::MetricsProtobuf,
                length: 32,
                history_binding: None
            })
        );
        assert_eq!(
            parse_headers(&head("application/json", ""), address, Some(&token)),
            Err("401 Unauthorized")
        );
        assert_eq!(
            parse_headers(
                &head("application/json", &format!("{authorized}{authorized}")),
                address,
                Some(&token)
            ),
            Err("401 Unauthorized")
        );
        assert_eq!(
            parse_headers(
                &head("application/json", &authorized).replace("32", "4194305"),
                address,
                Some(&token)
            ),
            Err("413 Content Too Large")
        );

        let source = parse_source_id("otlp.metrics-fixture").unwrap();
        let json = metrics::decode_json(
            include_bytes!("trace_receiver/metrics/fixtures/sdk-metrics.json"),
            &source,
        )
        .unwrap();
        let proto = metrics::decode_protobuf(
            include_bytes!("trace_receiver/metrics/fixtures/sdk-metrics.pb"),
            &source,
        )
        .unwrap();
        assert!(!json.is_empty());
        assert_eq!(json, proto);
        let mut store = agenturm_store::Store::open(":memory:").unwrap();
        let commit = |store: &mut agenturm_store::Store, rows| {
            crate::trace_import::commit_with_store(store, &source, rows).unwrap()
        };
        assert_eq!(commit(&mut store, &json), json.len());
        assert_eq!(commit(&mut store, &proto), 0);
        assert!(
            matches!(store.query_metric_history(&MetricHistoryQuery::Series {
            source_id: Some(source), snapshot_id: None, after: None, limit: 64,
        }, None).unwrap(), MetricHistoryPage::Series { rows, .. } if !rows.is_empty())
        );
    }

    #[test]
    fn progress_uses_the_same_bearer_and_a_small_json_only_body() {
        let address = "127.0.0.1:4318".parse().unwrap();
        let token = ProducerToken("ab".repeat(32));
        let prefix = format!(
            "POST {} HTTP/1.1\r\nHost: 127.0.0.1:4318\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n",
            trace_progress::PATH,
        );
        let authorized = format!("{prefix}Authorization: Bearer {}\r\n\r\n", token.0);
        assert_eq!(
            parse_headers(&authorized, address, Some(&token)),
            Ok(RequestHead {
                encoding: Encoding::ProgressJson,
                length: 4096,
                history_binding: None
            })
        );
        assert_eq!(
            parse_headers(&format!("{prefix}\r\n"), address, Some(&token)),
            Err("401 Unauthorized")
        );
        assert_eq!(
            parse_headers(&authorized.replace("4096", "4097"), address, Some(&token)),
            Err("413 Content Too Large")
        );
        assert_eq!(
            parse_headers(
                &authorized.replace("application/json", "application/x-protobuf"),
                address,
                Some(&token),
            ),
            Err("415 Unsupported Media Type")
        );
    }

    #[test]
    fn rejects_cross_origin_and_ambiguous_lengths() {
        let address = "127.0.0.1:4318".parse().unwrap();
        let prefix = "POST /v1/traces HTTP/1.1\r\nHost: 127.0.0.1:4318\r\nContent-Type: application/json\r\nContent-Length: 2\r\n";
        assert_eq!(
            parse_headers(&format!("{prefix}\r\n"), address, None),
            Ok(RequestHead {
                encoding: Encoding::Json,
                length: 2,
                history_binding: None
            })
        );
        assert_eq!(
            parse_headers(
                &format!("{prefix}\r\n").replace("application/json", "application/x-protobuf"),
                address,
                None,
            ),
            Ok(RequestHead {
                encoding: Encoding::Protobuf,
                length: 2,
                history_binding: None
            }),
        );
        assert!(
            parse_headers(&format!("{prefix}Content-Length: 3\r\n\r\n"), address, None).is_err()
        );
        assert!(
            parse_headers(
                &format!("{prefix}Origin: https://example.invalid\r\n\r\n"),
                address,
                None,
            )
            .is_err()
        );
        assert!(
            parse_headers(
                &format!("{prefix}Transfer-Encoding: chunked\r\n\r\n"),
                address,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn protected_receiver_requires_one_exact_bearer_header() {
        let address = "127.0.0.1:4318".parse().unwrap();
        let token = ProducerToken("aa".repeat(32));
        let prefix = "POST /v1/traces HTTP/1.1\r\nHost: 127.0.0.1:4318\r\nContent-Type: application/json\r\nContent-Length: 2\r\n";
        let authorization = format!("Authorization: Bearer {}\r\n", token.0);
        for rejected in [
            String::new(),
            format!("Authorization: Bearer {}\r\n", "bb".repeat(32)),
            format!("{authorization}{authorization}"),
            format!("Proxy-{authorization}"),
        ] {
            assert_eq!(
                parse_headers(&format!("{prefix}{rejected}\r\n"), address, Some(&token)),
                Err("401 Unauthorized")
            );
        }
        assert_eq!(
            parse_headers(
                &format!("{prefix}{authorization}\r\n"),
                address,
                Some(&token)
            ),
            Ok(RequestHead {
                encoding: Encoding::Json,
                length: 2,
                history_binding: None
            })
        );
        assert_eq!(
            parse_headers(&format!("{prefix}\r\n"), address, None),
            Ok(RequestHead {
                encoding: Encoding::Json,
                length: 2,
                history_binding: None
            })
        );
    }
}
