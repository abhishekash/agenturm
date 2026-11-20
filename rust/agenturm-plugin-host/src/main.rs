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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

const CONTROL_LIMIT: usize = 64 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
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
    if std::env::var_os("AGENTURM_PLUGIN_DISCOVERY_MODE").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        command.env("AGENTURM_PLUGIN_DISCOVERY_MODE", "1");
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
    healthy: Arc<AtomicBool>,
    response_budget: Option<Arc<AtomicUsize>>,
) -> Receiver<Option<String>> {
    let (sender, receiver) = sync_channel(2);
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        loop {
            match read_frame(&mut reader, limit) {
                Ok(Some(frame)) => {
                    // The parent control queue may fill while a child blocks.
                    // Never block this reader: it must observe EOF and let
                    // the relay reap that child. Child frames are permitted
                    // only when a hello/poll response is actually expected.
                    if response_budget.as_ref().is_some_and(|budget| {
                        budget
                            .try_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                                remaining.checked_sub(1)
                            })
                            .is_err()
                    }) || sender.try_send(Some(frame)).is_err()
                    {
                        healthy.store(false, Ordering::Release);
                        return;
                    }
                }
                Ok(None) | Err(_) => {
                    healthy.store(false, Ordering::Release);
                    let _ = sender.try_send(None);
                    return;
                }
            }
        }
    });
    receiver
}

fn control_frame(
    parent_input: &Receiver<Option<String>>,
    parent_alive: &AtomicBool,
) -> Result<Option<String>> {
    loop {
        if !parent_alive.load(Ordering::Acquire) {
            return Err(invalid("parent control pipe closed or exceeded its queue"));
        }
        match parent_input.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => {
                if !parent_alive.load(Ordering::Acquire) {
                    return Err(invalid("parent control pipe closed or exceeded its queue"));
                }
                return Ok(frame);
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(None),
            Err(RecvTimeoutError::Timeout) => continue,
        }
    }
}

struct RelayChannels<'a> {
    parent_input: &'a Receiver<Option<String>>,
    parent_alive: &'a AtomicBool,
    child_output: &'a Receiver<Option<String>>,
    child_alive: &'a AtomicBool,
    response_budget: &'a AtomicUsize,
}

// A blocked pipe must never park the supervisor that owns and reaps the child.
// Exactly one writer owns each pipe, with at most one outstanding frame. On
// failure the process exits; a blocked writer is not reused or joined.
struct FrameWriter {
    requests: SyncSender<String>,
    completed: Receiver<bool>,
}

impl FrameWriter {
    fn new(writer: impl Write + Send + 'static, limit: usize) -> Self {
        let (requests, incoming) = sync_channel::<String>(1);
        let (finished, completed) = sync_channel(1);
        thread::spawn(move || {
            let mut writer = BufWriter::new(writer);
            while let Ok(frame) = incoming.recv() {
                let success = write_frame(&mut writer, &frame, limit).is_ok();
                if finished.send(success).is_err() || !success {
                    return;
                }
            }
        });
        Self {
            requests,
            completed,
        }
    }

    fn write(&self, frame: String, peers: &[&AtomicBool]) -> Result<()> {
        self.requests
            .try_send(frame)
            .map_err(|_| invalid("pipe writer unavailable"))?;
        let deadline = Instant::now() + WRITE_TIMEOUT;
        loop {
            if peers.iter().any(|peer| !peer.load(Ordering::Acquire)) {
                return Err(invalid("peer failed while writing frame"));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(invalid("pipe write timed out"));
            }
            match self
                .completed
                .recv_timeout(remaining.min(Duration::from_millis(100)))
            {
                Ok(true) => return Ok(()),
                Ok(false) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(invalid("pipe write failed"));
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

fn relay(
    channels: RelayChannels<'_>,
    parent_output: &FrameWriter,
    child_input: &FrameWriter,
    child: &mut OwnedChild,
) -> Result<()> {
    let mut state = RelayState::default();
    loop {
        let Some(raw) = control_frame(channels.parent_input, channels.parent_alive)? else {
            return Ok(());
        };
        let kind = state.request(&raw)?;
        if !channels.child_alive.load(Ordering::Acquire) {
            return Err(invalid(
                "child data pipe closed or sent an unsolicited frame",
            ));
        }
        if matches!(kind, "hello" | "poll") {
            channels.response_budget.store(1, Ordering::Release);
        }
        // A child may close stdout immediately after reading stop, so its EOF
        // is expected during that write. Parent EOF still aborts supervision.
        let mut peers = vec![channels.parent_alive];
        if kind != "stop" {
            peers.push(channels.child_alive);
        }
        child_input.write(raw, &peers)?;
        if kind == "stop" {
            wait_child_stop(child);
            return Ok(());
        }
        if kind == "ack" {
            continue;
        }
        let response = loop {
            if !channels.parent_alive.load(Ordering::Acquire) {
                return Err(invalid("parent control pipe closed while child was busy"));
            }
            if !channels.child_alive.load(Ordering::Acquire) {
                return Err(invalid(
                    "child data pipe closed or sent an unsolicited frame",
                ));
            }
            match channels
                .child_output
                .recv_timeout(Duration::from_millis(100))
            {
                Ok(Some(frame)) => break frame,
                Ok(None) => return Err(invalid("child closed its data pipe")),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(invalid("child data pipe failed"));
                }
                Err(RecvTimeoutError::Timeout) => continue,
            }
        };
        state.response(kind, &response)?;
        parent_output.write(response, &[channels.parent_alive, channels.child_alive])?;
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
    let child_input = FrameWriter::new(input, CONTROL_LIMIT);
    let parent_alive = Arc::new(AtomicBool::new(true));
    let child_alive = Arc::new(AtomicBool::new(true));
    let response_budget = Arc::new(AtomicUsize::new(0));
    let parent_input = frame_reader(io::stdin(), CONTROL_LIMIT, Arc::clone(&parent_alive), None);
    let child_output = frame_reader(
        output,
        MAX_FRAME_BYTES,
        Arc::clone(&child_alive),
        Some(Arc::clone(&response_budget)),
    );
    let parent_output = FrameWriter::new(io::stdout(), MAX_FRAME_BYTES);
    relay(
        RelayChannels {
            parent_input: &parent_input,
            parent_alive: &parent_alive,
            child_output: &child_output,
            child_alive: &child_alive,
            response_budget: &response_budget,
        },
        &parent_output,
        &child_input,
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
    use std::io::Cursor;

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

    #[test]
    fn saturated_parent_queue_fails_closed_without_blocking_reader() {
        let mut input = Vec::new();
        for _ in 0..3 {
            write_frame(&mut input, "{}", CONTROL_LIMIT).unwrap();
        }
        let healthy = Arc::new(AtomicBool::new(true));
        let receiver = frame_reader(
            Cursor::new(input),
            CONTROL_LIMIT,
            Arc::clone(&healthy),
            None,
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        while healthy.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!healthy.load(Ordering::Acquire));
        assert_eq!(receiver.try_iter().count(), 2);
    }

    #[test]
    fn unsolicited_child_frame_exhausts_one_response_budget() {
        let mut input = Vec::new();
        for _ in 0..2 {
            write_frame(&mut input, "{}", MAX_FRAME_BYTES).unwrap();
        }
        let healthy = Arc::new(AtomicBool::new(true));
        let budget = Arc::new(AtomicUsize::new(1));
        let receiver = frame_reader(
            Cursor::new(input),
            MAX_FRAME_BYTES,
            Arc::clone(&healthy),
            Some(Arc::clone(&budget)),
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        while healthy.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!healthy.load(Ordering::Acquire));
        assert_eq!(budget.load(Ordering::Acquire), 0);
        assert_eq!(receiver.try_iter().count(), 1);
    }
}
