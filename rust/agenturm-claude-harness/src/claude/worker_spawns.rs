//! Structured Claude Task/Agent declaration and result correlation.

use super::{digest, identifier, worker_checkpoint};
use agenturm_jsonl_source::{MAX_CURSOR, Result};
use agenturm_protocol::observation::WorkerSpawn;
use serde::Deserialize;
use std::{collections::BTreeSet, fmt::Write};

pub(super) const PREFIX: &str = ":-w:";
const MAX_DECLARATIONS: usize = 3;
// The shared source cursor reserves 261 bytes for identity and prefix proof.
const MAX_CHECKPOINT_BYTES: usize = MAX_CURSOR - 261;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct WorkerSpawns {
    turn_worker: Option<String>,
    declarations: Vec<Declaration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Declaration {
    operation: String,
    assistant: String,
    consumed: bool,
}

impl Declaration {
    fn wire_len(&self) -> usize {
        self.operation.len() + self.assistant.len() + 4
    }
}

#[derive(Deserialize)]
struct Opening<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(rename = "sessionId")]
    session: &'a str,
    #[serde(rename = "agentId")]
    worker: Option<&'a str>,
    #[serde(rename = "timestamp")]
    _timestamp: &'a str,
    uuid: &'a str,
    #[serde(borrow)]
    message: OpeningMessage<'a>,
}

#[derive(Deserialize)]
struct OpeningMessage<'a> {
    role: &'a str,
}

#[derive(Deserialize)]
struct Row<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(rename = "sessionId")]
    session: &'a str,
    #[serde(rename = "agentId")]
    worker: Option<&'a str>,
    #[serde(rename = "timestamp")]
    _timestamp: &'a str,
    uuid: Option<&'a str>,
    #[serde(rename = "sourceToolAssistantUUID")]
    assistant: Option<&'a str>,
    #[serde(borrow)]
    message: Message<'a>,
    #[serde(rename = "toolUseResult", borrow)]
    result: Option<ToolResult<'a>>,
}

#[derive(Deserialize)]
struct Message<'a> {
    role: &'a str,
    #[serde(borrow)]
    content: Vec<Block<'a>>,
}

#[derive(Deserialize)]
struct Block<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    id: Option<&'a str>,
    name: Option<&'a str>,
    tool_use_id: Option<&'a str>,
    is_error: Option<bool>,
}

#[derive(Deserialize)]
struct ToolResult<'a> {
    status: Option<&'a str>,
    #[serde(rename = "agentId")]
    child: Option<&'a str>,
}

impl Row<'_> {
    fn has_owner(&self, session: &str, worker: &str, kind: &str) -> bool {
        self.kind == kind
            && self.message.role == kind
            && self.session == session
            && self.worker.unwrap_or("") == worker
    }
}

impl WorkerSpawns {
    pub(super) fn start_turn(raw: &[u8], session: &str, worker: &str) -> Self {
        let turn_worker = serde_json::from_slice::<Opening<'_>>(raw)
            .ok()
            .filter(|row| {
                row.kind == "user"
                    && row.message.role == "user"
                    && row.session == session
                    && row.worker.unwrap_or("") == worker
                    && identifier(row.uuid)
            })
            .map(|_| worker_checkpoint(worker));
        Self {
            turn_worker,
            ..Self::default()
        }
    }

    pub(super) fn has_turn_owner(&self) -> bool {
        self.turn_worker.is_some()
    }

    pub(super) fn is_tracking(&self) -> bool {
        !self.declarations.is_empty()
    }

    fn withhold_for_turn(&mut self) {
        *self = Self::default();
    }

    fn matches_turn_worker(&self, worker: &str) -> bool {
        self.turn_worker
            .as_ref()
            .is_some_and(|owner| *owner == worker_checkpoint(worker))
    }

    pub(super) fn note_declarations(&mut self, raw: &[u8], session: &str, worker: &str) {
        if !self.matches_turn_worker(worker) {
            return;
        }
        let Ok(row) = serde_json::from_slice::<Row<'_>>(raw) else {
            // An ambiguous declaration might reuse an already tracked operation.
            self.withhold_for_turn();
            return;
        };
        if !row.has_owner(session, worker, "assistant") {
            return;
        }
        let Some(assistant) = row.uuid.filter(|id| identifier(id)).map(digest) else {
            return;
        };
        let mut seen = BTreeSet::new();
        let mut repeated = BTreeSet::new();
        for block in &row.message.content {
            if block.kind == "tool_use"
                && let Some(id) = block.id
                && !seen.insert(id)
            {
                repeated.insert(id);
            }
        }
        for block in row.message.content {
            if block.kind != "tool_use" {
                continue;
            }
            let Some(id) = block.id.filter(|id| identifier(id)) else {
                continue;
            };
            let operation = digest(id);
            if let Some(prior) = self
                .declarations
                .iter_mut()
                .find(|prior| prior.operation == operation)
            {
                // Any repeated operation declares ambiguous launch authority.
                prior.consumed = true;
            } else if matches!(block.name, Some("Task" | "Agent")) {
                if self.declarations.len() >= MAX_DECLARATIONS {
                    self.withhold_for_turn();
                    return;
                }
                self.declarations.push(Declaration {
                    operation,
                    assistant: assistant.clone(),
                    consumed: repeated.contains(id),
                });
            }
        }
    }

    pub(super) fn take_result(
        &mut self,
        raw: &[u8],
        session: &str,
        worker: &str,
    ) -> Option<(String, WorkerSpawn)> {
        if !self.matches_turn_worker(worker) {
            return None;
        }
        let row: Row<'_> = serde_json::from_slice(raw).ok()?;
        if !row.has_owner(session, worker, "user") {
            return None;
        }
        let [block] = row.message.content.as_slice() else {
            return None;
        };
        if block.kind != "tool_result" {
            return None;
        }
        let operation = digest(block.tool_use_id.filter(|id| identifier(id))?);
        let assistant = digest(row.assistant.filter(|id| identifier(id))?);
        let proof = self
            .declarations
            .iter_mut()
            .find(|proof| proof.operation == operation && proof.assistant == assistant)?;
        if std::mem::replace(&mut proof.consumed, true) || block.is_error == Some(true) {
            return None;
        }
        let result = row.result?;
        if result.status != Some("completed") {
            return None;
        }
        let child_worker_id = result.child.filter(|id| identifier(id))?.to_owned();
        let spawn = WorkerSpawn { child_worker_id };
        spawn
            .is_valid_for(Some(worker))
            .then_some((operation, spawn))
    }

    pub(super) fn enforce_checkpoint_budget(&mut self, base_bytes: usize) {
        let Some(owner) = &self.turn_worker else {
            return;
        };
        let entries_bytes = self
            .declarations
            .iter()
            .map(Declaration::wire_len)
            .sum::<usize>();
        if base_bytes + PREFIX.len() + owner.len() + entries_bytes > MAX_CHECKPOINT_BYTES {
            self.withhold_for_turn();
        }
    }

    pub(super) fn encode(&self, checkpoint: &mut String) {
        if let Some(owner) = &self.turn_worker {
            checkpoint.push_str(PREFIX);
            checkpoint.push_str(owner);
            for declaration in &self.declarations {
                let status = if declaration.consumed { 'x' } else { 'a' };
                write!(
                    checkpoint,
                    ":{}:{}:{status}",
                    declaration.operation, declaration.assistant,
                )
                .expect("writing a string does not fail");
            }
        }
    }

    pub(super) fn restore(
        suffix: Option<&str>,
        has_turn: bool,
        checkpoint_bytes: usize,
    ) -> Result<Self> {
        let Some(suffix) = suffix else {
            return Ok(Self::default());
        };
        let invalid = || "invalid Claude worker spawn checkpoint";
        if !has_turn || checkpoint_bytes > MAX_CHECKPOINT_BYTES {
            return Err(invalid().into());
        }
        let hash = |id: &str| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        let mut fields = suffix.split(':');
        let owner = fields
            .next()
            .filter(|owner| *owner == "x" || hash(owner))
            .ok_or_else(invalid)?;
        let fields: Vec<_> = fields.collect();
        let (items, remainder) = fields.as_chunks::<3>();
        if items.len() > MAX_DECLARATIONS || !remainder.is_empty() {
            return Err(invalid().into());
        }
        let mut declarations: Vec<Declaration> = Vec::new();
        for &[operation, assistant, status] in items {
            if !hash(operation)
                || !hash(assistant)
                || !matches!(status, "a" | "x")
                || declarations
                    .iter()
                    .any(|prior| prior.operation == operation)
            {
                return Err(invalid().into());
            }
            declarations.push(Declaration {
                operation: operation.to_owned(),
                assistant: assistant.to_owned(),
                consumed: status == "x",
            });
        }
        Ok(Self {
            turn_worker: Some(owner.to_owned()),
            declarations,
        })
    }
}
