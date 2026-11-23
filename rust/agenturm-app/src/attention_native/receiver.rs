//! Send explicit native events to a paired, history-fenced Rust receiver.

use super::{MAX_BYTES, USAGE};
use crate::{
    remote_history::{RemoteHistoryOptions, RemoteStore, http, parse_attach},
    trace_import::parse_source_id,
    trace_receiver::{NATIVE_ATTENTION_PATH, ProducerToken},
};
use agenturm_kernel::{
    ExtensionId,
    native_attention::{NativeAttentionCapture, NativeAttentionReceipt, NativeAttentionScope},
};
use agenturm_storage_service::logical_read_identity;
use agenturm_store::{
    WorkspaceAnchor, WorkspaceHistory,
    native_attention::canonical_event_id,
    native_outbox::{Event, EventKind, QueuedEvent, hex_id},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::BufReader,
    net::{SocketAddr, TcpStream},
    path::Path,
    time::{Duration, Instant},
};

pub(super) const PROTOCOL: &str = "rust-v1";
const BINDING_SCHEMA: &str = "agenturm-rust-native-binding-v1";
const EVENT_SCHEMA: &str = "agenturm-rust-native-event-v1";

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustBinding {
    pub(super) sender_id: String,
    schema: String,
    reader_endpoint: String,
    collector_endpoint: String,
    collector_source: ExtensionId,
    history_binding: String,
    scope: NativeAttentionScope,
}

impl RustBinding {
    pub(super) fn parse(flags: &mut BTreeMap<&str, String>) -> Result<Self, String> {
        Ok(Self {
            sender_id: String::new(),
            schema: BINDING_SCHEMA.into(),
            reader_endpoint: flags.remove("--endpoint").ok_or(USAGE)?,
            collector_endpoint: flags.remove("--collector-endpoint").ok_or(USAGE)?,
            collector_source: parse_source_id(&flags.remove("--collector-source").ok_or(USAGE)?)?,
            history_binding: String::new(),
            scope: NativeAttentionScope {
                source_id: flags.remove("--source-id").ok_or(USAGE)?,
                session_id: flags.remove("--session-id").ok_or(USAGE)?,
                turn_id: flags.remove("--turn-id").ok_or(USAGE)?,
                worker_id: flags.remove("--worker-id"),
            },
        })
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if self.schema != BINDING_SCHEMA
            || !hex_id(&self.sender_id, 32)
            || !self.collector_source.as_str().starts_with("otlp.")
            || logical_read_identity(&self.history_binding).is_none()
        {
            return Err("Invalid Rust native lifecycle binding metadata".into());
        }
        self.anchor()?;
        Ok(())
    }

    fn anchor(&self) -> Result<WorkspaceAnchor, String> {
        if !self.scope.is_valid() || self.scope.source_id.starts_with("host.") {
            return Err("Rust native binding requires an exact native session and turn".into());
        }
        let harness_id =
            ExtensionId::parse(&self.scope.source_id).map_err(|_| "Invalid bound native source")?;
        Ok(WorkspaceAnchor {
            harness_id,
            session_id: self.scope.session_id.clone(),
            turn_id: Some(self.scope.turn_id.clone()),
            worker_id: self.scope.worker_id.clone(),
        })
    }

    pub(super) fn event(&self, bytes: &[u8]) -> Result<Event, String> {
        if bytes.len() > MAX_BYTES {
            return Err("Native input exceeds the 8 KiB limit".into());
        }
        let input: NativeEventInput =
            serde_json::from_slice(bytes).map_err(|_| "Invalid Rust native event metadata")?;
        if input.schema != EVENT_SCHEMA
            || input.sender_id != self.sender_id
            || input.history_binding != self.history_binding
            || input.scope != self.scope
        {
            return Err("Rust native event contradicts the exact saved binding".into());
        }
        let event = Event {
            event_id: input.event_id,
            sequence: Some(input.sequence),
            kind: input.kind,
            trace_id: None,
            turn_id: input.scope.turn_id,
            request_id: input.request_id,
            source_time_unix_ns: input.source_time_unix_ns,
        };
        if !self.capture(&event, input.sequence).is_valid() {
            return Err("Rust native event metadata is invalid".into());
        }
        Ok(event)
    }

    fn capture(&self, event: &Event, sequence: u64) -> NativeAttentionCapture {
        NativeAttentionCapture {
            history_binding: self.history_binding.clone(),
            scope: self.scope.clone(),
            sender_id: self.sender_id.clone(),
            event_id: event.event_id.clone(),
            sequence,
            kind: event.kind,
            request_id: event.request_id.clone(),
            source_time_unix_ns: event.source_time_unix_ns,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeEventInput {
    schema: String,
    sender_id: String,
    history_binding: String,
    scope: NativeAttentionScope,
    event_id: String,
    sequence: u64,
    kind: EventKind,
    request_id: Option<String>,
    source_time_unix_ns: Option<u64>,
}

pub(super) struct RustReceiver {
    reader: RemoteStore,
    collector: SocketAddr,
    token: ProducerToken,
}

impl RustReceiver {
    pub(super) fn bind(
        binding: &mut RustBinding,
        reader_path: &Path,
        collector_path: &Path,
    ) -> Result<(), String> {
        let receiver = Self::open(binding, reader_path, collector_path)?;
        binding.history_binding = receiver.reader.read_binding().into();
        binding.validate()?;
        receiver.verify(binding)
    }

    pub(super) fn connect(
        binding: &RustBinding,
        reader_path: &Path,
        collector_path: &Path,
    ) -> Result<Self, String> {
        binding.validate()?;
        let receiver = Self::open(binding, reader_path, collector_path)?;
        if receiver.reader.read_binding() != binding.history_binding {
            return Err("Rust native receiver history changed; pending events retained".into());
        }
        Ok(receiver)
    }

    fn open(
        binding: &RustBinding,
        reader_path: &Path,
        collector_path: &Path,
    ) -> Result<Self, String> {
        binding.anchor()?;
        let reader_options = endpoint(&binding.reader_endpoint, reader_path)?;
        let collector = endpoint(&binding.collector_endpoint, collector_path)?.endpoint();
        let token = ProducerToken::from_file(collector_path)?;
        let reader_token = ProducerToken::from_file(reader_path)?;
        if collector == reader_options.endpoint() || token.authorizes(&reader_token.bearer_header())
        {
            return Err(
                "Rust native delivery requires distinct reader and collector endpoints and tokens"
                    .into(),
            );
        }
        let reader = RemoteStore::open(&reader_options)?;
        Ok(Self {
            reader,
            collector,
            token,
        })
    }

    pub(super) fn verify(&self, binding: &RustBinding) -> Result<(), String> {
        if self.reader.read_binding() != binding.history_binding
            || !self.reader.contains_anchor(&binding.anchor()?)?
        {
            return Err("Rust native receiver no longer confirms the exact bound turn".into());
        }
        Ok(())
    }

    pub(super) fn send(&self, binding: &RustBinding, queued: &QueuedEvent) -> Result<(), String> {
        if queued.event.trace_id.is_some()
            || queued.event.turn_id != binding.scope.turn_id
            || queued.event.sequence != Some(queued.sequence)
        {
            return Err("Queued Rust native event contradicts the exact saved binding".into());
        }
        self.verify(binding)?;
        let capture = binding.capture(&queued.event, queued.sequence);
        self.post(&binding.collector_source, &capture)?;
        self.verify(binding)
    }

    fn post(&self, source: &ExtensionId, capture: &NativeAttentionCapture) -> Result<(), String> {
        const UNCERTAIN: &str =
            "Rust native event acceptance is unconfirmed; pending event retained";
        let body = serde_json::to_vec(capture).map_err(|_| UNCERTAIN)?;
        if body.len() > MAX_BYTES {
            return Err(UNCERTAIN.into());
        }
        let timeout = Duration::from_secs(5);
        let deadline = Instant::now() + timeout;
        let stream = TcpStream::connect_timeout(&self.collector, timeout).map_err(|_| UNCERTAIN)?;
        let head = format!(
            "POST {NATIVE_ATTENTION_PATH} HTTP/1.1\r\nHost: {}\r\nAuthorization: {}\r\nX-Agenturm-History-Binding: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.collector,
            self.token.bearer_header(),
            capture.history_binding,
            body.len(),
        );
        http::write(&stream, deadline, &head, &body).map_err(|_| UNCERTAIN)?;
        let mut reader = BufReader::new(&stream);
        let head = http::read_head(&mut reader, deadline, MAX_BYTES).map_err(|_| UNCERTAIN)?;
        let mut status = head.start.split_whitespace();
        if !matches!(status.next(), Some("HTTP/1.0" | "HTTP/1.1"))
            || status.next() != Some("200")
            || head.history_binding.as_deref() != Some(&capture.history_binding)
        {
            return Err(UNCERTAIN.into());
        }
        let body = http::read_body(&mut reader, deadline, head.length).map_err(|_| UNCERTAIN)?;
        let receipt: NativeAttentionReceipt =
            serde_json::from_slice(&body).map_err(|_| UNCERTAIN)?;
        if receipt.history_binding != capture.history_binding
            || receipt.event_id != canonical_event_id(source, capture)
        {
            return Err(UNCERTAIN.into());
        }
        Ok(())
    }
}

fn endpoint(url: &str, token: &Path) -> Result<RemoteHistoryOptions, String> {
    let token = token.to_str().ok_or("Invalid native token file path")?;
    parse_attach(&[
        "--endpoint".into(),
        url.into(),
        "--token-file".into(),
        token.into(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding() -> RustBinding {
        RustBinding {
            schema: BINDING_SCHEMA.into(),
            sender_id: "1".repeat(32),
            reader_endpoint: "http://127.0.0.1:2000".into(),
            collector_endpoint: "http://127.0.0.1:2001".into(),
            collector_source: parse_source_id("otlp.attention").unwrap(),
            history_binding: format!("{}:{}", "2".repeat(32), "3".repeat(32)),
            scope: NativeAttentionScope {
                source_id: "harness.codex".into(),
                session_id: "session-4".into(),
                turn_id: "turn/5".into(),
                worker_id: None,
            },
        }
    }

    #[test]
    fn input_is_metadata_only_and_bound_to_exact_history_source_turn_worker_and_sender() {
        let binding = binding();
        binding.validate().unwrap();
        let anchor = binding.anchor().unwrap();
        assert_eq!(anchor.turn_id.as_deref(), Some("turn/5"));
        assert_eq!(anchor.worker_id, None);

        let input = json!({
            "schema": EVENT_SCHEMA,
            "sender_id": binding.sender_id,
            "history_binding": binding.history_binding,
            "scope": binding.scope,
            "event_id": "native-request-1",
            "sequence": 1,
            "kind": "approval_requested",
            "request_id": "request-1"
        });
        let parse = |value: &serde_json::Value| binding.event(value.to_string().as_bytes());
        let event = parse(&input).unwrap();
        assert_eq!(event.trace_id, None);
        assert_eq!(event.turn_id, "turn/5");
        assert_eq!(event.source_time_unix_ns, None);
        let mut epoch = input.clone();
        epoch["source_time_unix_ns"] = json!(0);
        assert_eq!(parse(&epoch).unwrap().source_time_unix_ns, Some(0));

        for (field, value) in [
            ("source_id", json!("harness.claude")),
            ("session_id", json!("another-session")),
            ("turn_id", json!("another-turn")),
            ("worker_id", json!("sibling-worker")),
        ] {
            let mut changed = input.clone();
            changed["scope"][field] = value;
            assert!(parse(&changed).is_err(), "{field}");
        }
        for (field, value) in [
            ("history_binding", json!("2".repeat(32))),
            ("sender_id", json!("4".repeat(32))),
            ("prompt", json!("sensitive-fixture")),
            ("decision", json!("approved")),
        ] {
            let mut changed = input.clone();
            changed[field] = value;
            assert!(parse(&changed).is_err(), "{field}");
        }
    }
}
