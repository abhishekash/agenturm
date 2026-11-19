//! Bounded process transport for one separately installed harness adapter.
//!
//! Python remains the authority for manifest verification, grants, policy,
//! leases, Store commits and acknowledgments. This process owns child I/O and
//! cleanup. A parent timeout kills its dedicated process group, including the
//! child, before it can be restarted from the last durable cursor.

#![forbid(unsafe_code)]

use agenturm_protocol::{MAX_FRAME_BYTES, read_frame, write_frame};
use serde_json::Value;
use std::error::Error;
use std::ffi::OsString;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

const CONTROL_LIMIT: usize = 64 * 1024;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn spawn_child(path: &PathBuf) -> Result<(OwnedChild, ChildStdin, ChildStdout)> {
    if !path.is_absolute() {
        return Err(invalid("child path must be absolute"));
    }
    let mut command = Command::new(path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    command
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin")),
        )
        .env("LANG", "C.UTF-8")
        .env("AGENTURM_PLUGIN_PROTOCOL", "1");
    if let Some(root) = std::env::var_os("AGENTURM_PLUGIN_SOURCE_ROOT") {
        command.env("AGENTURM_PLUGIN_SOURCE_ROOT", root);
    }
    let mut child = OwnedChild(command.spawn()?);
    let input = child.0.stdin.take().ok_or("child stdin unavailable")?;
    let output = child.0.stdout.take().ok_or("child stdout unavailable")?;
    Ok((child, input, output))
}

#[derive(Default)]
struct RelayState {
    hello: bool,
    pending: Option<String>,
}

impl RelayState {
    fn request(&mut self, raw: &str) -> Result<&'static str> {
        let message: Value = serde_json::from_str(raw)?;
        let object = message
            .as_object()
            .ok_or("control frame must be an object")?;
        if object.get("protocol").and_then(Value::as_u64) != Some(1) {
            return Err(invalid("protocol mismatch"));
        }
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or("missing control type")?;
        if !self.hello {
            if kind != "hello" {
                return Err(invalid("hello required first"));
            }
            self.hello = true;
            return Ok("hello");
        }
        match kind {
            "poll" if self.pending.is_none() => Ok("poll"),
            "ack" => {
                let cursor = object
                    .get("cursor")
                    .and_then(Value::as_str)
                    .ok_or("missing ack cursor")?;
                if self.pending.as_deref() != Some(cursor) {
                    return Err(invalid("ack cursor mismatch"));
                }
                self.pending = None;
                Ok("ack")
            }
            "stop" => Ok("stop"),
            _ => Err(invalid("invalid control sequence")),
        }
    }

    fn response(&mut self, kind: &str, raw: &str) -> Result<()> {
        let message: Value = serde_json::from_str(raw)?;
        if kind == "hello" {
            if message.get("type").and_then(Value::as_str) != Some("hello_ack") {
                return Err(invalid("invalid child hello response"));
            }
        } else if kind == "poll" {
            if message.get("type").and_then(Value::as_str) != Some("batch") {
                return Err(invalid("invalid child poll response"));
            }
            self.pending = Some(
                message
                    .get("cursor")
                    .and_then(Value::as_str)
                    .ok_or("missing batch cursor")?
                    .to_owned(),
            );
        }
        Ok(())
    }
}

fn wait_child_stop(child: &mut OwnedChild) {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if child.0.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn frame_reader<R: Read + Send + 'static>(
    reader: R,
    limit: usize,
    alive: Option<Arc<AtomicBool>>,
) -> Receiver<Option<String>> {
    let (sender, receiver) = sync_channel(2);
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        loop {
            match read_frame(&mut reader, limit) {
                Ok(Some(frame)) => {
                    if sender.send(Some(frame)).is_err() {
                        return;
                    }
                }
                Ok(None) | Err(_) => {
                    if let Some(alive) = alive {
                        alive.store(false, Ordering::Release);
                    }
                    let _ = sender.send(None);
                    return;
                }
            }
        }
    });
    receiver
}

fn relay(
    parent_input: &Receiver<Option<String>>,
    parent_alive: &AtomicBool,
    parent_output: &mut impl Write,
    child_input: &mut impl Write,
    child_output: &Receiver<Option<String>>,
    child: &mut OwnedChild,
) -> Result<()> {
    let mut state = RelayState::default();
    loop {
        let Some(raw) = parent_input.recv()? else {
            return Ok(());
        };
        let kind = state.request(&raw)?;
        write_frame(child_input, &raw, CONTROL_LIMIT)?;
        if kind == "stop" {
            wait_child_stop(child);
            return Ok(());
        }
        if kind == "ack" {
            continue;
        }
        let response = loop {
            if !parent_alive.load(Ordering::Acquire) {
                return Err(invalid("parent control pipe closed while child was busy"));
            }
            match child_output.recv_timeout(Duration::from_millis(100)) {
                Ok(Some(frame)) => break frame,
                Ok(None) => return Err(invalid("child closed its data pipe")),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(invalid("child data pipe failed"));
                }
                Err(RecvTimeoutError::Timeout) => continue,
            }
        };
        state.response(kind, &response)?;
        write_frame(parent_output, &response, MAX_FRAME_BYTES)?;
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--child")) {
        return Err(invalid("expected --child"));
    }
    let path = PathBuf::from(args.next().ok_or("missing child path")?);
    if args.next().is_some() {
        return Err(invalid("unexpected host arguments"));
    }
    let (mut child, input, output) = spawn_child(&path)?;
    let mut child_input = BufWriter::new(input);
    let parent_alive = Arc::new(AtomicBool::new(true));
    let parent_input = frame_reader(io::stdin(), CONTROL_LIMIT, Some(Arc::clone(&parent_alive)));
    let child_output = frame_reader(output, MAX_FRAME_BYTES, None);
    let stdout = io::stdout();
    let mut parent_output = BufWriter::new(stdout.lock());
    relay(
        &parent_input,
        &parent_alive,
        &mut parent_output,
        &mut child_input,
        &child_output,
        &mut child,
    )
}

fn main() {
    // Parent surfaces coarse transport failure; never print child paths or
    // source content from an untrusted adapter's error to stderr.
    if run().is_err() {
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_poll_ack_sequence() {
        let mut state = RelayState::default();
        let request = |kind, cursor| {
            serde_json::json!({"type":kind,"protocol":1,
            "cursor":cursor})
            .to_string()
        };
        assert!(state.request(&request("poll", "")).is_err());
        assert_eq!(state.request(&request("hello", "")).unwrap(), "hello");
        state.response("hello", r#"{"type":"hello_ack"}"#).unwrap();
        assert_eq!(state.request(&request("poll", "")).unwrap(), "poll");
        state
            .response("poll", r#"{"type":"batch","cursor":"next"}"#)
            .unwrap();
        assert!(state.request(&request("poll", "next")).is_err());
        assert!(state.request(&request("ack", "wrong")).is_err());
        assert_eq!(state.request(&request("ack", "next")).unwrap(), "ack");
        assert_eq!(state.request(&request("poll", "next")).unwrap(), "poll");
    }

    #[test]
    fn malformed_child_response_never_sets_pending() {
        let mut state = RelayState::default();
        state.request(r#"{"type":"hello","protocol":1}"#).unwrap();
        assert!(state.response("hello", r#"{"type":"batch"}"#).is_err());
        assert!(state.response("poll", r#"{"type":"batch"}"#).is_err());
        assert!(state.pending.is_none());
    }
}
