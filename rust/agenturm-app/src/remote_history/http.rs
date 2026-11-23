//! One bounded HTTP message per connection. The attached reader dials loopback directly.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub(super) const PATH: &str = "/v1/history/read";
pub(super) const MAX_REQUEST: usize = 16 * 1024;
pub(super) const MAX_REPLY: usize = 4 * 1024 * 1024;
pub(super) const IO_BUDGET: Duration = Duration::from_secs(5);
const MAX_HEAD: usize = 8 * 1024;

pub(super) struct Head {
    pub start: String,
    pub length: usize,
    pub host: Option<String>,
    pub bearer: Option<String>,
}

pub(super) fn read_head(
    reader: &mut BufReader<&TcpStream>,
    deadline: Instant,
    max_body: usize,
) -> io::Result<Head> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() == MAX_HEAD {
            return Err(invalid());
        }
        reader
            .get_ref()
            .set_read_timeout(Some(remaining(deadline)?))?;
        let next = reader.fill_buf()?.first().copied().ok_or_else(invalid)?;
        bytes.push(next);
        reader.consume(1);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    let mut lines = text.split("\r\n");
    let start = lines
        .next()
        .filter(|start| !start.is_empty())
        .ok_or_else(invalid)?;
    let (mut host, mut bearer, mut length, mut json) = (None, None, None, false);
    for raw in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = raw.split_once(':').ok_or_else(invalid)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || value
                .bytes()
                .any(|byte| byte.is_ascii_control() && byte != b'\t')
        {
            return Err(invalid());
        }
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length"
                if length.is_none() && value.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                length = Some(value.parse().map_err(|_| invalid())?);
            }
            "content-type" if !json && value.eq_ignore_ascii_case("application/json") => {
                json = true
            }
            "host" if host.is_none() => host = Some(value.to_owned()),
            "authorization" if bearer.is_none() => bearer = Some(value.to_owned()),
            "content-length" | "content-type" | "host" | "authorization" | "transfer-encoding"
            | "content-encoding" | "origin" | "expect" => return Err(invalid()),
            _ => {}
        }
    }
    let length = length
        .filter(|length| *length <= max_body)
        .ok_or_else(invalid)?;
    if !json {
        return Err(invalid());
    }
    Ok(Head {
        start: start.to_owned(),
        length,
        host,
        bearer,
    })
}

pub(super) fn read_body(
    reader: &mut BufReader<&TcpStream>,
    deadline: Instant,
    length: usize,
) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0; length];
    let mut offset = 0;
    while offset < bytes.len() {
        reader
            .get_ref()
            .set_read_timeout(Some(remaining(deadline)?))?;
        let read = reader.read(&mut bytes[offset..])?;
        if read == 0 {
            return Err(invalid());
        }
        offset += read;
    }
    Ok(bytes)
}

pub(super) fn write(
    stream: &TcpStream,
    deadline: Instant,
    head: &str,
    body: &[u8],
) -> io::Result<()> {
    let mut stream = stream;
    for mut remaining_bytes in [head.as_bytes(), body] {
        while !remaining_bytes.is_empty() {
            stream.set_write_timeout(Some(remaining(deadline)?))?;
            let written = stream.write(remaining_bytes)?;
            if written == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            remaining_bytes = &remaining_bytes[written..];
        }
    }
    Ok(())
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid or oversized HTTP message",
    )
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))
}
