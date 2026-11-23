//! Closed metadata carried by a bound native lifecycle sender.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeAttentionScope {
    pub source_id: String,
    pub session_id: String,
    pub turn_id: String,
    /// Absence identifies the main actor of the retained turn.
    pub worker_id: Option<String>,
}

impl NativeAttentionScope {
    pub fn is_valid(&self) -> bool {
        !self.source_id.starts_with("otlp.")
            && identifier(&self.source_id, 96)
            && identifier(&self.session_id, 512)
            && identifier(&self.turn_id, 512)
            && self
                .worker_id
                .as_deref()
                .is_none_or(|id| identifier(id, 512))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeAttentionKind {
    ApprovalRequested,
    ReplyRequested,
    RequestResolved,
    TurnStarted,
    TurnCompleted,
    TurnInterrupted,
}

impl NativeAttentionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApprovalRequested => "approval_requested",
            Self::ReplyRequested => "reply_requested",
            Self::RequestResolved => "request_resolved",
            Self::TurnStarted => "turn_started",
            Self::TurnCompleted => "turn_completed",
            Self::TurnInterrupted => "turn_interrupted",
        }
    }

    pub const fn is_request(self) -> bool {
        matches!(self, Self::ApprovalRequested | Self::ReplyRequested)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeAttentionCapture {
    pub history_binding: String,
    pub scope: NativeAttentionScope,
    pub sender_id: String,
    pub event_id: String,
    pub sequence: u64,
    pub kind: NativeAttentionKind,
    pub request_id: Option<String>,
    /// Producer metadata only. The receiver assigns the receipt clock.
    pub source_time_unix_ns: Option<u64>,
}

impl NativeAttentionCapture {
    pub fn is_valid(&self) -> bool {
        valid_history_binding(&self.history_binding)
            && self.scope.is_valid()
            && hex(&self.sender_id, 32)
            && identifier(&self.event_id, 200)
            && (1..=i64::MAX as u64).contains(&self.sequence)
            && self
                .source_time_unix_ns
                .is_none_or(|at| at <= i64::MAX as u64)
            && match (&self.request_id, self.kind) {
                (Some(id), kind)
                    if kind.is_request() || kind == NativeAttentionKind::RequestResolved =>
                {
                    identifier(id, 200)
                }
                (None, kind) => !kind.is_request() && kind != NativeAttentionKind::RequestResolved,
                _ => false,
            }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeAttentionReceipt {
    pub history_binding: String,
    pub event_id: String,
    pub replayed: bool,
}

pub fn is_attention_event_id(value: &str) -> bool {
    value
        .strip_prefix("native-attention:")
        .is_some_and(|digest| hex(digest, 64))
}

pub fn valid_history_binding(value: &str) -> bool {
    match value.split_once(':') {
        Some((identity, incarnation)) => hex(identity, 32) && hex(incarnation, 32),
        None => hex(value, 32),
    }
}

fn hex(value: &str, width: usize) -> bool {
    value.len() == width
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn identifier(value: &str, max: usize) -> bool {
    value.len() <= max
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}
