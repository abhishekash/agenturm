//! Authenticated handoff to an explicitly selected existing OTLP listener.

use crate::{remote_history::http, trace_receiver::ProducerToken};
use std::{
    io::{self, BufReader},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

const BUDGET: Duration = Duration::from_secs(3);
const EMPTY_EXPORT: &[u8] = br#"{"resourceSpans":[]}"#;
const MAX_PROBE_REPLY: usize = 512;

pub(super) fn parse(value: &str) -> Result<SocketAddr, String> {
    const INVALID: &str = "Trace launch requires a literal loopback HTTP endpoint and port";
    let authority = value.strip_prefix("http://").ok_or(INVALID)?;
    let authority = authority
        .strip_suffix("/v1/traces")
        .or_else(|| authority.strip_suffix('/'))
        .unwrap_or(authority);
    authority
        .parse::<SocketAddr>()
        .ok()
        .filter(|address| {
            address.ip().is_loopback() && address.port() != 0 && !authority.contains('%')
        })
        .ok_or_else(|| INVALID.into())
}

pub(super) fn verify(address: SocketAddr, token: &ProducerToken) -> Result<(), String> {
    const INVALID: &str = "Existing OTLP receiver did not accept the authenticated startup probe";
    let deadline = Instant::now() + BUDGET;
    let (unauthorized, _) = probe(address, None, deadline).map_err(|_| INVALID)?;
    if unauthorized != "HTTP/1.1 401 Unauthorized" {
        return Err("Existing OTLP receiver must require producer bearer authentication".into());
    }
    let (authenticated, body) = probe(address, Some(token), deadline).map_err(|_| INVALID)?;
    if authenticated != "HTTP/1.1 200 OK" || body != b"{}" {
        return Err(INVALID.into());
    }
    Ok(())
}

fn probe(
    address: SocketAddr,
    token: Option<&ProducerToken>,
    deadline: Instant,
) -> io::Result<(String, Vec<u8>)> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(io::ErrorKind::TimedOut)?;
    let stream = TcpStream::connect_timeout(&address, remaining)?;
    let auth = token.map_or_else(String::new, |token| {
        format!("Authorization: {}\r\n", token.bearer_header())
    });
    let head = format!(
        "POST /v1/traces HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{auth}Connection: close\r\n\r\n",
        EMPTY_EXPORT.len(),
    );
    http::write(&stream, deadline, &head, EMPTY_EXPORT)?;
    let mut reader = BufReader::new(&stream);
    let reply = http::read_head(&mut reader, deadline, MAX_PROBE_REPLY)?;
    let body = http::read_body(&mut reader, deadline, reply.length)?;
    Ok((reply.start, body))
}
