//! Loopback-only HTTP ingestion for bounded OTLP trace exports.

use crate::trace_import::{
    MAX_EXPORT_BYTES, commit, decode_json, decode_protobuf, parse_source_id, protobuf_response,
};
use agenturm_kernel::{ExtensionId, Observation};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

const USAGE: &str =
    "agenturm serve-traces --listen <loopback-IP:port> --db <path> --source-id otlp.<id>";
const MAX_HEADERS: usize = 8192;
const READ_BUDGET: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceReceiverOptions {
    listen: SocketAddr,
    db: PathBuf,
    source: ExtensionId,
    launch_session: Option<String>,
}

pub(super) fn parse(args: &[String]) -> Result<TraceReceiverOptions, String> {
    if args.len() != 6 {
        return Err(USAGE.into());
    }
    let (mut listen, mut db, mut source) = (None, None, None);
    for pair in args.as_chunks::<2>().0 {
        let value = pair[1].as_str();
        if value.is_empty() || value.starts_with("--") {
            return Err(USAGE.into());
        }
        match pair[0].as_str() {
            "--listen" if listen.is_none() => {
                let address = value.parse::<SocketAddr>().map_err(|_| USAGE)?;
                if !address.ip().is_loopback() {
                    return Err("trace receiver requires a loopback IP".into());
                }
                listen = Some(address);
            }
            "--db" if db.is_none() && value != "-" && value != ":memory:" => {
                db = Some(PathBuf::from(value))
            }
            "--source-id" if source.is_none() => source = Some(parse_source_id(value)?),
            _ => return Err(USAGE.into()),
        }
    }
    Ok(TraceReceiverOptions {
        listen: listen.ok_or(USAGE)?,
        db: db.ok_or(USAGE)?,
        source: source.ok_or(USAGE)?,
        launch_session: None,
    })
}

pub fn execute_trace_receiver(options: &TraceReceiverOptions) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = crate::WatchSignalRegistrations::new(Arc::clone(&stop))?;
    let receiver = TraceReceiver::bind(options.clone())?;
    println!(
        "Listening for OTLP traces at http://{}/v1/traces",
        receiver.address()
    );
    receiver.serve(&stop)
}

pub(super) struct TraceReceiver {
    listener: TcpListener,
    address: SocketAddr,
    options: TraceReceiverOptions,
}

impl TraceReceiver {
    fn bind(options: TraceReceiverOptions) -> Result<Self, String> {
        let listener = TcpListener::bind(options.listen)
            .map_err(|error| format!("bind trace receiver: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        Ok(Self {
            listener,
            address,
            options,
        })
    }

    pub(super) fn bind_ephemeral(
        db: PathBuf,
        source: ExtensionId,
        launch_session: String,
    ) -> Result<Self, String> {
        Self::bind(TraceReceiverOptions {
            listen: SocketAddr::from(([127, 0, 0, 1], 0)),
            db,
            source,
            launch_session: Some(launch_session),
        })
    }

    pub(super) fn bind_cockpit(db: PathBuf) -> Result<Self, String> {
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
        Self::bind(TraceReceiverOptions {
            listen: SocketAddr::from(([127, 0, 0, 1], port)),
            db,
            source: parse_source_id("otlp.local")?,
            launch_session: None,
        })
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(super) fn serve(self, stop: &AtomicBool) -> Result<(), String> {
        let db = self.options.db.clone();
        self.serve_with(stop, |source, rows| commit(&db, source, &rows).map(|_| ()))
    }

    pub(super) fn serve_with(
        self,
        stop: &AtomicBool,
        mut store: impl FnMut(&ExtensionId, Vec<Observation>) -> Result<(), String>,
    ) -> Result<(), String> {
        while !stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, peer)) if peer.ip().is_loopback() => handle(
                    stream,
                    self.address,
                    &self.options.source,
                    self.options.launch_session.as_deref(),
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
    commit: &mut impl FnMut(&ExtensionId, Vec<Observation>) -> Result<(), String>,
) {
    let deadline = Instant::now() + READ_BUDGET;
    let result = read_request(&stream, address, deadline).and_then(|(encoding, body)| {
        let decoded = match encoding {
            Encoding::Json => decode_json(&body, source, launch_session),
            Encoding::Protobuf => decode_protobuf(&body, source, launch_session),
        };
        let (rows, skipped) = decoded.map_err(|_| "400 Bad Request")?;
        if !rows.is_empty() {
            commit(source, rows).map_err(|_| "503 Service Unavailable")?;
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
    let (encoding, length) = parse_headers(headers, address)?;
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

fn parse_headers(headers: &str, address: SocketAddr) -> Result<(Encoding, usize), &'static str> {
    let mut lines = headers.split("\r\n");
    if !matches!(
        lines.next(),
        Some("POST /v1/traces HTTP/1.0" | "POST /v1/traces HTTP/1.1")
    ) {
        return Err("404 Not Found");
    }
    let mut host = false;
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
            parse_headers(&format!("{prefix}\r\n"), address),
            Ok((Encoding::Json, 2))
        );
        assert_eq!(
            parse_headers(
                &format!("{prefix}\r\n").replace("application/json", "application/x-protobuf"),
                address
            ),
            Ok((Encoding::Protobuf, 2)),
        );
        assert!(parse_headers(&format!("{prefix}Content-Length: 3\r\n\r\n"), address).is_err());
        assert!(
            parse_headers(
                &format!("{prefix}Origin: https://example.invalid\r\n\r\n"),
                address
            )
            .is_err()
        );
        assert!(
            parse_headers(
                &format!("{prefix}Transfer-Encoding: chunked\r\n\r\n"),
                address
            )
            .is_err()
        );
    }
}
