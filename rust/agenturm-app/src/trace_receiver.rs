//! Loopback-only HTTP ingestion for bounded OTLP JSON trace exports.

use crate::trace_import::{MAX_EXPORT_BYTES, commit, decode, parse_source_id};
use agenturm_kernel::ExtensionId;
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
    })
}

pub fn execute_trace_receiver(options: &TraceReceiverOptions) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = crate::WatchSignalRegistrations::new(Arc::clone(&stop))?;
    let listener = TcpListener::bind(options.listen)
        .map_err(|error| format!("bind trace receiver: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    println!("Listening for OTLP JSON traces at http://{address}/v1/traces");
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) if peer.ip().is_loopback() => handle(stream, address, options),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25))
            }
            Err(error) => return Err(format!("trace receiver accept failed: {error}")),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream, address: SocketAddr, options: &TraceReceiverOptions) {
    let deadline = Instant::now() + READ_BUDGET;
    let body = read_request(&stream, address, deadline);
    let result = body.and_then(|body| {
        let (rows, skipped) = decode(&body, &options.source).map_err(|_| "400 Bad Request")?;
        commit(&options.db, &options.source, &rows).map_err(|_| "503 Service Unavailable")?;
        Ok(if skipped == 0 {
            "{}".to_owned()
        } else {
            serde_json::json!({"partialSuccess": {
                "rejectedSpans": skipped.to_string(),
                "errorMessage": "Span lacks an explicit session identity",
            }})
            .to_string()
        })
    });
    let (status, response) = match result {
        Ok(response) => ("200 OK", response),
        Err(status) => (status, serde_json::json!({"message":status}).to_string()),
    };
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{response}",
        response.len()
    );
}

fn read_request(
    stream: &TcpStream,
    address: SocketAddr,
    deadline: Instant,
) -> Result<Vec<u8>, &'static str> {
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
    let length = parse_headers(headers, address)?;
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
    Ok(body)
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

fn parse_headers(headers: &str, address: SocketAddr) -> Result<usize, &'static str> {
    let mut lines = headers.split("\r\n");
    if !matches!(
        lines.next(),
        Some("POST /v1/traces HTTP/1.0" | "POST /v1/traces HTTP/1.1")
    ) {
        return Err("404 Not Found");
    }
    let mut host = false;
    let mut json = false;
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
            "content-type" if !json && value.eq_ignore_ascii_case("application/json") => {
                json = true
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
    if !host || !json {
        return Err("400 Bad Request");
    }
    let length = length.ok_or("411 Length Required")?;
    if length as u64 > MAX_EXPORT_BYTES {
        return Err("413 Content Too Large");
    }
    Ok(length)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_cross_origin_and_ambiguous_lengths() {
        let address = "127.0.0.1:4318".parse().unwrap();
        let prefix = "POST /v1/traces HTTP/1.1\r\nHost: 127.0.0.1:4318\r\nContent-Type: application/json\r\nContent-Length: 2\r\n";
        assert_eq!(parse_headers(&format!("{prefix}\r\n"), address), Ok(2));
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
