//! Strict v1 binding and bounded event normalization for native senders.

use super::{MAX_BYTES, read_random};
use agenturm_store::native_outbox::{Event, EventKind, hex_id, valid_identifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

pub(super) const BINDING_SCHEMA: &str = "agenturm-native-binding-v1";
const EVENT_SCHEMA: &str = "agenturm-native-event-v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) enum Adapter {
    #[serde(rename = "generic-v1")]
    GenericV1,
    #[serde(rename = "codex-notify-v1")]
    CodexNotifyV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SourceClock {
    Unknown,
    Receiver,
}

impl SourceClock {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Receiver => "receiver",
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    pub schema: String,
    pub sender_id: String,
    pub endpoint: String,
    pub adapter: Adapter,
    pub native_turn_id: String,
    pub source_clock: SourceClock,
    pub follow_recorded_turns: bool,
    pub receiver_id: String,
    pub service: String,
    pub agent_id: String,
    pub session_id: String,
    pub trace_id: String,
    pub turn_id: String,
    pub source_id: String,
    pub parent_agent_id: String,
}

impl Binding {
    pub(super) fn validate(&self) -> Result<(), String> {
        let optional_turn = self.turn_id.is_empty() || hex_id(&self.turn_id, 32);
        let optional_parent = self.parent_agent_id.is_empty() || actor(&self.parent_agent_id);
        let optional_native =
            self.native_turn_id.is_empty() || valid_identifier(&self.native_turn_id);
        let adapter = match self.adapter {
            Adapter::GenericV1 => !self.follow_recorded_turns && self.native_turn_id.is_empty(),
            Adapter::CodexNotifyV1 => self.follow_recorded_turns || !self.native_turn_id.is_empty(),
        };
        if self.schema != BINDING_SCHEMA
            || !valid_identifier(&self.sender_id)
            || !hex_id(&self.receiver_id, 32)
            || !actor(&self.agent_id)
            || !hex_id(&self.trace_id, 32)
            || !valid_identifier(&self.service)
            || !valid_identifier(&self.session_id)
            || !valid_identifier(&self.source_id)
            || self.endpoint.len() > 1024
            || !optional_turn
            || !optional_parent
            || !optional_native
            || !adapter
        {
            return Err("Invalid native lifecycle binding metadata".into());
        }
        Ok(())
    }

    pub(super) fn fingerprint(&self) -> Result<[u8; 32], String> {
        let bytes =
            serde_json::to_vec(self).map_err(|_| "Cannot encode native binding metadata")?;
        Ok(Sha256::digest(bytes).into())
    }

    pub(super) fn event(&self, bytes: &[u8]) -> Result<Event, String> {
        if bytes.len() > MAX_BYTES {
            return Err("Native input exceeds the 8 KiB limit".into());
        }
        match self.adapter {
            Adapter::GenericV1 => self.generic(bytes),
            Adapter::CodexNotifyV1 => self.codex(bytes),
        }
    }

    fn generic(&self, bytes: &[u8]) -> Result<Event, String> {
        let value: EventEnvelope =
            serde_json::from_slice(bytes).map_err(|_| "Invalid native event metadata")?;
        if value.schema != EVENT_SCHEMA
            || value.sender_id != self.sender_id
            || value.receiver_id != self.receiver_id
            || value.service != self.service
            || value.agent_id != self.agent_id
            || value.session_id != self.session_id
            || value.trace_id != self.trace_id
            || value.turn_id != self.turn_id
            || value.source_id != self.source_id
            || value.parent_agent_id != self.parent_agent_id
        {
            return Err("Native event contradicts the configured binding".into());
        }
        Ok(Event {
            event_id: value.event_id,
            sequence: Some(value.sequence),
            kind: value.kind,
            trace_id: value.trace_id,
            turn_id: value.turn_id,
            request_id: value.request_id.filter(|id| !id.is_empty()),
            source_time_unix_ns: value.time_unix_nano.unwrap_or(0),
        })
    }

    fn codex(&self, bytes: &[u8]) -> Result<Event, String> {
        let value: CodexNotify =
            serde_json::from_slice(bytes).map_err(|_| "Invalid Codex notify metadata")?;
        if value.kind != "agent-turn-complete"
            || value.thread_id != self.session_id
            || !valid_identifier(&value.turn_id)
            || (!self.follow_recorded_turns && value.turn_id != self.native_turn_id)
        {
            return Err(
                "Codex notification session or turn contradicts the configured binding".into(),
            );
        }
        let (trace_id, turn_id, sequence) = if self.follow_recorded_turns {
            let digest = Sha256::digest(format!("{}\0{}", self.source_id, value.turn_id));
            (format!("{digest:x}")[..32].into(), String::new(), None)
        } else {
            (self.trace_id.clone(), self.turn_id.clone(), Some(1))
        };
        let event_id = format!("codex-complete:{}", value.turn_id);
        Ok(Event {
            event_id,
            sequence,
            kind: EventKind::TurnCompleted,
            trace_id,
            turn_id,
            request_id: None,
            source_time_unix_ns: 0,
        })
    }
}

fn actor(value: &str) -> bool {
    value.strip_prefix("ag1_").is_some_and(|id| hex_id(id, 32))
}

pub(super) fn sender_id() -> Result<String, String> {
    let mut random = [0u8; 16];
    read_random(&mut random)?;
    let mut id = String::with_capacity(32);
    for byte in random {
        write!(id, "{byte:02x}").expect("writing to a string");
    }
    Ok(id)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventEnvelope {
    schema: String,
    sender_id: String,
    receiver_id: String,
    service: String,
    agent_id: String,
    session_id: String,
    trace_id: String,
    turn_id: String,
    source_id: String,
    parent_agent_id: String,
    event_id: String,
    sequence: u64,
    kind: EventKind,
    request_id: Option<String>,
    time_unix_nano: Option<u64>,
}

#[derive(Deserialize)]
struct CodexNotify {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "thread-id")]
    thread_id: String,
    #[serde(rename = "turn-id")]
    turn_id: String,
}
