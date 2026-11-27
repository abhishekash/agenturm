//! Structured Claude Task/Agent declaration and result correlation.

use super::{digest, identifier, worker_checkpoint};
use agenturm_jsonl_source::{MAX_CURSOR, Result};
use agenturm_protocol::observation::{WorkerLaunch, WorkerSpawn};
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
    #[serde(rename = "isAsync")]
    is_async: Option<bool>,
    #[serde(rename = "agentId")]
    child: Option<&'a str>,
}

/// A correlated Agent result claims either a spawned child or an asynchronously
/// launched child. Both consume the declaration; neither claims a resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WorkerOutcome {
    Spawned(WorkerSpawn),
    Launched(WorkerLaunch),
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
    ) -> Option<(String, WorkerOutcome)> {
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
        let outcome = match (result.is_async, result.status) {
            (_, Some("completed")) => WorkerOutcome::Spawned(WorkerSpawn {
                child_worker_id: result.child.filter(|id| identifier(id))?.to_owned(),
            }),
            (Some(true), Some("async_launched")) => WorkerOutcome::Launched(WorkerLaunch {
                child_worker_id: result.child.filter(|id| identifier(id))?.to_owned(),
            }),
            _ => return None,
        };
        let valid = match &outcome {
            WorkerOutcome::Spawned(spawn) => spawn.is_valid_for(Some(worker)),
            WorkerOutcome::Launched(launch) => launch.is_valid_for(Some(worker)),
        };
        valid.then_some((operation, outcome))
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

#[cfg(test)]
mod tests {
    use super::*;

    const OPENING: &[u8] = br#"{"type":"user","sessionId":"session","agentId":"parent","uuid":"turn","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user"}}"#;
    const DECLARATION: &[u8] = br#"{"type":"assistant","sessionId":"session","agentId":"parent","uuid":"declaration","timestamp":"2026-10-24T12:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"Agent","id":"toolu_agent1","input":{"prompt":"private launch"}}]}}"#;
    const ASYNC_RESULT: &str = r#"{"type":"user","sessionId":"session","agentId":"parent","uuid":"launch","sourceToolAssistantUUID":"declaration","timestamp":"2026-10-24T12:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent1","content":"private result"}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"child","description":"side quest","resolvedModel":"claude-sonnet","prompt":"private prompt","outputFile":"/tmp/claude/child-output","canReadOutputFile":true,"canContinueAgent":true}}"#;
    const COMPLETED_RESULT: &str = r#"{"type":"user","sessionId":"session","agentId":"parent","uuid":"completion","sourceToolAssistantUUID":"declaration","timestamp":"2026-10-24T12:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent1","content":"private result"}]},"toolUseResult":{"status":"completed","agentId":"child"}}"#;

    fn tracked() -> WorkerSpawns {
        let mut spawns = WorkerSpawns::start_turn(OPENING, "session", "parent");
        spawns.note_declarations(DECLARATION, "session", "parent");
        spawns
    }

    fn take(spawns: &mut WorkerSpawns, raw: &str) -> Option<(String, WorkerOutcome)> {
        spawns.take_result(raw.as_bytes(), "session", "parent")
    }

    #[test]
    fn real_async_shape_yields_a_launch_and_consumes_the_declaration() {
        let mut spawns = tracked();
        let (operation, outcome) = take(&mut spawns, ASYNC_RESULT).unwrap();
        assert_eq!(operation, digest("toolu_agent1"));
        assert_eq!(
            outcome,
            WorkerOutcome::Launched(WorkerLaunch {
                child_worker_id: "child".into(),
            })
        );
        // First consumer wins: a later completed result cannot double-claim.
        assert!(take(&mut spawns, COMPLETED_RESULT).is_none());
        assert!(take(&mut spawns, ASYNC_RESULT).is_none());
    }

    #[test]
    fn completed_result_still_yields_a_spawn() {
        let mut spawns = tracked();
        let (operation, outcome) = take(&mut spawns, COMPLETED_RESULT).unwrap();
        assert_eq!(operation, digest("toolu_agent1"));
        assert_eq!(
            outcome,
            WorkerOutcome::Spawned(WorkerSpawn {
                child_worker_id: "child".into(),
            })
        );
    }

    #[test]
    fn unsupported_async_evidence_fails_closed() {
        for (from, to) in [
            (r#""isAsync":true"#, r#""isAsync":false"#),
            (r#""isAsync":true,"#, ""),
            (r#""status":"async_launched""#, r#""status":"queued""#),
            (r#""agentId":"child","#, ""),
            (r#""agentId":"child""#, r#""agentId":"not a valid id""#),
            (
                r#""tool_use_id":"toolu_agent1""#,
                r#""tool_use_id":"toolu_agent1","is_error":true"#,
            ),
            (
                r#""sourceToolAssistantUUID":"declaration""#,
                r#""sourceToolAssistantUUID":"other""#,
            ),
        ] {
            let raw = ASYNC_RESULT.replace(from, to);
            assert_ne!(raw, ASYNC_RESULT);
            let mut spawns = tracked();
            assert!(take(&mut spawns, &raw).is_none(), "{raw}");
        }
    }
}
