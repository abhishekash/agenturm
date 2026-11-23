//! Loopback-only HTTP ingestion for bounded OTLP trace exports.

use crate::trace_import::{
    MAX_EXPORT_BYTES, commit, commit_payloads, decode_json, decode_protobuf, parse_source_id,
    protobuf_response,
};
use agenturm_kernel::{ExtensionId, Observation};
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
const BROKER_USAGE: &str = "agenturm broker serve-traces --broker-config <json> --listen <loopback-IP:port> --source-id otlp.<id> --token-file <path> [--payload-scope <standalone|harness-id>]";
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
        use std::fs::OpenOptions;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        const INVALID: &str =
            "Bearer token file must be an owner-only regular file with 64 lowercase hex digits";
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| INVALID)?;
        let metadata = file.metadata().map_err(|_| INVALID)?;
        // SAFETY: geteuid takes no arguments and has no memory preconditions.
        let owner = unsafe { libc::geteuid() };
        if !metadata.is_file() || metadata.uid() != owner || metadata.mode() & 0o077 != 0 {
            return Err(INVALID.into());
        }
        let mut token = String::new();
        file.take(66)
            .read_to_string(&mut token)
            .map_err(|_| INVALID)?;
        if token.ends_with('\n') {
            token.pop();
        }
        if token.len() != 64
            || !token
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(INVALID.into());
        }
        Ok(Self(token))
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
            Some(binding) => commit_payloads(&options.db, binding, &rows, &payloads),
            None => commit(&options.db, source, &rows).map(|_| ()),
        },
    )
}

pub(super) fn execute_with_commit(
    settings: &TraceReceiverSettings,
    token: Option<ProducerToken>,
    commit: impl FnMut(
        &ExtensionId,
        Vec<Observation>,
        Vec<TelemetryPayloadRecord>,
    ) -> Result<(), String>,
) -> Result<(), String> {
    execute_with_maintenance(settings, token, None, commit)
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
) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = crate::WatchSignalRegistrations::new(Arc::clone(&stop))?;
    let receiver =
        TraceReceiver::bind(settings.clone(), None, token)?.with_local_maintenance(maintenance_db);
    println!(
        "Listening for OTLP traces at http://{}/v1/traces",
        receiver.address()
    );
    receiver.serve_with_payloads(&stop, commit)
}

pub(super) struct TraceReceiver {
    maintenance: Option<super::retention_cli::LocalRetentionMaintenance>,
    listener: TcpListener,
    address: SocketAddr,
    source: ExtensionId,
    launch_session: Option<String>,
    token: Option<ProducerToken>,
    payload_binding: Option<TelemetryBinding>,
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
        mut self,
        stop: &AtomicBool,
        mut store: impl FnMut(
            &ExtensionId,
            Vec<Observation>,
            Vec<TelemetryPayloadRecord>,
        ) -> Result<(), String>,
    ) -> Result<(), String> {
        while !stop.load(Ordering::Relaxed) {
            if let Some(maintenance) = self.maintenance.as_mut() {
                maintenance.tick();
            }
            match self.listener.accept() {
                Ok((stream, peer)) if peer.ip().is_loopback() => handle(
                    stream,
                    self.address,
                    &self.source,
                    self.launch_session.as_deref(),
                    self.token.as_ref(),
                    self.payload_binding.as_ref(),
                    &mut store,
                ),
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
    address: SocketAddr,
    source: &ExtensionId,
    launch_session: Option<&str>,
    token: Option<&ProducerToken>,
    binding: Option<&TelemetryBinding>,
    commit: &mut impl FnMut(
        &ExtensionId,
        Vec<Observation>,
        Vec<TelemetryPayloadRecord>,
    ) -> Result<(), String>,
) {
    let deadline = Instant::now() + READ_BUDGET;
    let result = read_request(&stream, address, token, deadline).and_then(|(encoding, body)| {
        let decoded = match encoding {
            Encoding::Json => decode_json(&body, source, launch_session, binding),
            Encoding::Protobuf => decode_protobuf(&body, source, launch_session, binding),
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
        };
        Ok((encoding, bytes))
    });
    let (status, encoding, response) = match result {
        Ok((encoding, response)) => ("200 OK", encoding, response),
        Err(status) => (
            status,
            Encoding::Json,
            serde_json::json!({"message":status})
                .to_string()
                .into_bytes(),
        ),
    };
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    if write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        encoding.media_type(),
        response.len(),
    )
    .is_ok()
    {
        let _ = stream.write_all(&response);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Encoding {
    Json,
    Protobuf,
}

impl Encoding {
    fn media_type(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::Protobuf => "application/x-protobuf",
        }
    }
}

fn read_request(
    stream: &TcpStream,
    address: SocketAddr,
    token: Option<&ProducerToken>,
    deadline: Instant,
) -> Result<(Encoding, Vec<u8>), &'static str> {
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
    let (encoding, length) = parse_headers(headers, address, token)?;
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
    Ok((encoding, body))
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
) -> Result<(Encoding, usize), &'static str> {
    let mut lines = headers.split("\r\n");
    if !matches!(
        lines.next(),
        Some("POST /v1/traces HTTP/1.0" | "POST /v1/traces HTTP/1.1")
    ) {
        return Err("404 Not Found");
    }
    let mut host = false;
    let mut authorization = None;
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
    let encoding = encoding.ok_or("400 Bad Request")?;
    let length = length.ok_or("411 Length Required")?;
    if length as u64 > MAX_EXPORT_BYTES {
        return Err("413 Content Too Large");
    }
    Ok((encoding, length))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_cross_origin_and_ambiguous_lengths() {
        let address = "127.0.0.1:4318".parse().unwrap();
        let prefix = "POST /v1/traces HTTP/1.1\r\nHost: 127.0.0.1:4318\r\nContent-Type: application/json\r\nContent-Length: 2\r\n";
        assert_eq!(
            parse_headers(&format!("{prefix}\r\n"), address, None),
            Ok((Encoding::Json, 2))
        );
        assert_eq!(
            parse_headers(
                &format!("{prefix}\r\n").replace("application/json", "application/x-protobuf"),
                address,
                None,
            ),
            Ok((Encoding::Protobuf, 2)),
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
            Ok((Encoding::Json, 2))
        );
        assert_eq!(
            parse_headers(&format!("{prefix}\r\n"), address, None),
            Ok((Encoding::Json, 2))
        );
    }
}
