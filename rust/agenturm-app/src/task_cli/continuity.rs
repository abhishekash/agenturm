//! CLI composition of current source proofs and explicit task approvals.

use super::*;
use agenturm_store::{TaskDraftApproval, TaskSourceProof, WorkspaceHistory};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const FORMAT: &str = "agenturm-rust-task-draft/1";
const MAX_DRAFT_BYTES: u64 = 64 * 1024;

pub(crate) trait TaskPromptVerifier {
    #[cfg(target_os = "linux")]
    fn with_prompt(
        &self,
        source: &ExtensionId,
        session: &str,
        event: &str,
        revision: u32,
        consume: &mut dyn FnMut(&agenturm_content::VerifiedPrompt) -> Result<(), String>,
    ) -> Result<(), String>;
}

#[cfg(target_os = "linux")]
impl TaskPromptVerifier for agenturm_storage_service::NativeStore {
    fn with_prompt(
        &self,
        source: &ExtensionId,
        session: &str,
        event: &str,
        revision: u32,
        consume: &mut dyn FnMut(&agenturm_content::VerifiedPrompt) -> Result<(), String>,
    ) -> Result<(), String> {
        self.with_verified_prompt(source, session, event, revision, consume)
            .map_err(string)
    }
}

pub(super) struct SourceAccess<'a> {
    pub history: &'a Path,
    pub verifier: Option<&'a dyn TaskPromptVerifier>,
}

struct FencedHistory(TaskSourceProof);

impl WorkspaceHistory for FencedHistory {
    fn contains_anchor(&self, anchor: &WorkspaceAnchor) -> Result<bool, String> {
        Ok(&self.0.anchor == anchor)
    }

    fn contains_event(
        &self,
        anchor: &WorkspaceAnchor,
        event: &str,
        revision: u32,
    ) -> Result<bool, String> {
        Ok(&self.0.anchor == anchor && self.0.event_id == event && self.0.revision == revision)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ExactSource {
    pub anchor: WorkspaceAnchor,
    pub event_id: String,
    pub revision: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DraftRequest {
    pub source: ExactSource,
    pub config: Option<PathBuf>,
    pub task_id: Option<u64>,
    pub output: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ApprovalRequest {
    pub file: PathBuf,
    pub config: Option<PathBuf>,
    pub title: Option<String>,
    pub workspace: Option<String>,
    pub expected_revision: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DecisionSource {
    pub file: PathBuf,
    pub config: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftTarget {
    task_id: u64,
    revision: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    format: String,
    history_binding: String,
    prepared_at_unix_ns: u64,
    target: Option<DraftTarget>,
    source: TaskSourceProof,
    draft_sha256: String,
    excerpt: String,
    title: String,
    continuation: String,
}

impl Draft {
    fn binding(&self) -> TaskResult<String> {
        let frozen = serde_json::to_vec(&(
            &self.format,
            &self.history_binding,
            self.prepared_at_unix_ns,
            &self.target,
            &self.source,
        ))
        .map_err(string)?;
        Ok(hash(frozen))
    }

    fn load(path: &Path, ledger: &WorkspaceLedger) -> TaskResult<Self> {
        let draft: Self = serde_json::from_str(&read_input(path, MAX_DRAFT_BYTES)?)
            .map_err(|_| "Choose a Rust Agenturm task draft JSON file")?;
        if draft.format != FORMAT
            || draft.draft_sha256 != draft.binding()?
            || draft.source.excerpt_sha256 != hash(&draft.excerpt)
        {
            return Err("The draft source fields or excerpt changed. Create a new draft for the selected evidence.".into());
        }
        if draft.history_binding != hex(ledger.task_draft_binding()?) {
            return Err(
                "The draft belongs to another history. Prepare a draft against this history."
                    .into(),
            );
        }
        Ok(draft)
    }
}

pub(super) fn draft(
    access: &SourceAccess<'_>,
    ledger: &WorkspaceLedger,
    request: &DraftRequest,
    output: &mut dyn Write,
) -> TaskResult<()> {
    let target = request.task_id.map(|id| task(ledger, id)).transpose()?;
    let frozen_target = target.as_ref().map(|task| DraftTarget {
        task_id: task.id,
        revision: task.revision,
    });
    with_source(
        access,
        request.config.as_deref(),
        &request.source,
        |proof, excerpt| {
            let prepared_at_unix_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|time| u64::try_from(time.as_nanos()).ok())
                .ok_or("Cannot timestamp the draft")?;
            let mut draft = Draft {
                format: FORMAT.into(),
                history_binding: hex(ledger.task_draft_binding()?),
                prepared_at_unix_ns,
                target: frozen_target,
                source: proof,
                draft_sha256: String::new(),
                excerpt: excerpt.into(),
                title: target.map_or_else(|| "Continue recorded work".into(), |task| task.title),
                continuation: String::new(),
            };
            draft.draft_sha256 = draft.binding()?;
            if let Some(path) = &request.output {
                let mut encoded = serde_json::to_vec_pretty(&draft).map_err(string)?;
                encoded.push(b'\n');
                write_draft_file(path, &encoded)?;
                write_json(
                    output,
                    &json!({ "draft": path, "draft_sha256": draft.draft_sha256 }),
                )
            } else {
                write_json(output, &draft)
            }
        },
    )
}

pub(super) fn approve(
    access: &SourceAccess<'_>,
    ledger: &WorkspaceLedger,
    request: &ApprovalRequest,
) -> TaskResult<Value> {
    let draft = Draft::load(&request.file, ledger)?;
    let workspace = workspace_directory(request.workspace.as_deref())?;
    if draft.target.is_some() && (request.title.is_some() || workspace.is_some()) {
        return Err("Title and workspace options apply to drafts for new tasks".into());
    }
    if request.expected_revision.is_some() && draft.target.is_none() {
        return Err("A draft for a new task has no expected revision".into());
    }
    let expected = draft.target.as_ref().map(|target| target.revision);
    match (request.expected_revision, expected) {
        (Some(expected), Some(current)) if expected != current => {
            return Err(WorkspaceError::TaskRevisionConflict { expected, current }.into());
        }
        _ => {}
    }
    with_frozen_source(
        access,
        request.config.as_deref(),
        ledger,
        &draft.source,
        |ledger| {
            let approved = TaskDraftApproval {
                draft_sha256: draft.draft_sha256.clone(),
                source: draft.source.clone(),
                continuation: draft.continuation,
            };
            let (task, approval) = if let Some(target) = draft.target {
                ledger
                    .approve_draft_for_existing_task(target.task_id, target.revision, &approved)?
                    .ok_or(MISSING_TASK)?
            } else {
                ledger.approve_draft_for_new_task(
                    request.title.as_deref().unwrap_or(&draft.title),
                    workspace.as_deref(),
                    &approved,
                )?
            };
            Ok(json!({ "task": task_json(&task), "approval": approval }))
        },
    )
}

pub(super) fn add_decision(
    access: &SourceAccess<'_>,
    ledger: &WorkspaceLedger,
    id: u64,
    expected_revision: Option<u64>,
    input: &TaskDecisionInput,
    source: &DecisionSource,
) -> TaskResult<(SavedTask, SavedTaskDecision)> {
    let draft = Draft::load(&source.file, ledger)?;
    let expected =
        if let Some(target) = draft.target {
            if target.task_id != id {
                return Err("The source draft belongs to another task".into());
            }
            if let Some(expected) = expected_revision
                && expected != target.revision
            {
                return Err(WorkspaceError::TaskRevisionConflict {
                    expected,
                    current: target.revision,
                }
                .into());
            }
            Some(target.revision)
        } else {
            Some(expected_revision.ok_or(
                "Supply --expected-revision when citing a draft prepared without --task-id",
            )?)
        };
    with_frozen_source(
        access,
        source.config.as_deref(),
        ledger,
        &draft.source,
        |ledger| {
            Ok(ledger
                .add_task_decision_with_source(id, input, expected, Some(&draft.source))?
                .ok_or(MISSING_TASK)?)
        },
    )
}

pub(super) fn read(
    access: &SourceAccess<'_>,
    ledger: &WorkspaceLedger,
    id: u64,
    reveal: Option<(Option<&Path>, u64)>,
    output: &mut dyn Write,
) -> TaskResult<()> {
    let handoff = ledger.task_handoff(id)?.ok_or(MISSING_TASK)?;
    let Some((config, approval_id)) = reveal else {
        return write_json(
            output,
            &json!({ "task_id": id, "revision": handoff.task.revision, "approvals": handoff.continuity }),
        );
    };
    let approval = handoff
        .continuity
        .iter()
        .find(|item| item.id == approval_id)
        .ok_or("The approved continuity version is unavailable")?;
    with_source(
        access,
        config,
        &from_proof(&approval.source),
        |current, excerpt| {
            check_proof(&approval.source, &current)?;
            let retained = ledger.task_handoff(id)?.ok_or(MISSING_TASK)?;
            if !retained.continuity.contains(approval) {
                return Err("The approved continuity version changed".into());
            }
            write_json(
                output,
                &json!({ "task_id": id, "approval": approval, "excerpt": excerpt }),
            )
        },
    )
}

fn with_frozen_source<T>(
    access: &SourceAccess<'_>,
    config: Option<&Path>,
    ledger: &WorkspaceLedger,
    frozen: &TaskSourceProof,
    use_source: impl FnOnce(&WorkspaceLedger) -> TaskResult<T>,
) -> TaskResult<T> {
    with_source(access, config, &from_proof(frozen), |current, _| {
        check_proof(frozen, &current)?;
        if access.verifier.is_some() {
            // A broker waits for the callback's ACK. Verify the exact anchor
            // using this live proof so saving cannot perform a nested RPC.
            let fenced =
                WorkspaceLedger::open_with_history(access.history, FencedHistory(current))?;
            if fenced.task_draft_binding()? != ledger.task_draft_binding()? {
                return Err("The task workspace changed during source verification".into());
            }
            use_source(&fenced)
        } else {
            use_source(ledger)
        }
    })
}

fn check_proof(frozen: &TaskSourceProof, current: &TaskSourceProof) -> TaskResult<()> {
    if frozen != current {
        return Err("The source evidence or its approval changed. Prepare a new draft against the current evidence.".into());
    }
    Ok(())
}

fn from_proof(proof: &TaskSourceProof) -> ExactSource {
    ExactSource {
        anchor: proof.anchor.clone(),
        event_id: proof.event_id.clone(),
        revision: proof.revision,
    }
}

fn with_source<T>(
    access: &SourceAccess<'_>,
    config: Option<&Path>,
    source: &ExactSource,
    use_source: impl FnOnce(TaskSourceProof, &str) -> TaskResult<T>,
) -> TaskResult<T> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (access, config, source, use_source);
        Err("Source-backed tasks are available on Linux".into())
    }
    #[cfg(target_os = "linux")]
    {
        use crate::content_preview::{PromptSelection, with_verified_prompt};
        let selected = &source.anchor;
        let consume = |current: &agenturm_content::VerifiedPrompt| {
            if selected.session_id != current.session_id
                || selected.worker_id != current.worker_id
                || selected.turn_id.as_deref() != Some(current.turn_id.as_str())
            {
                return Err("The prompt is outside the selected session, worker, or turn".into());
            }
            use_source(
                TaskSourceProof {
                    anchor: selected.clone(),
                    event_id: source.event_id.clone(),
                    revision: source.revision,
                    observed_at_unix_ns: current.observed_at_unix_ns,
                    grant_sha256: hex(current.grant_binding),
                    excerpt_sha256: hash(&current.text),
                },
                &current.text,
            )
        };
        if let Some(verifier) = access.verifier {
            if config.is_some() {
                return Err("Broker task proofs use the broker's configured sources".into());
            }
            let mut consume = Some(consume);
            let mut outcome = None;
            let transport = verifier.with_prompt(
                &selected.harness_id,
                &selected.session_id,
                &source.event_id,
                source.revision,
                &mut |proof| {
                    let consume = consume.take().ok_or("Prompt proof was already delivered")?;
                    let result = consume(proof);
                    let status = result.as_ref().map(|_| ()).map_err(string);
                    outcome = Some(result);
                    status
                },
            );
            return match outcome {
                Some(Err(error)) => Err(error),
                Some(Ok(value)) => {
                    transport?;
                    Ok(value)
                }
                None => {
                    transport?;
                    Err("No prompt proof was delivered".into())
                }
            };
        }
        let config = config.ok_or("Supply --config for local source verification")?;
        with_verified_prompt(
            access.history,
            config,
            &selected.harness_id,
            PromptSelection {
                session_id: &selected.session_id,
                event_id: &source.event_id,
                revision: source.revision,
                snapshot_id: None,
                history_identity: None,
            },
            consume,
        )?
    }
}

fn write_json(mut output: impl Write, value: &impl Serialize) -> TaskResult<()> {
    serde_json::to_writer(&mut output, value).map_err(string)?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(string)?;
    Ok(())
}

fn write_draft_file(path: &Path, bytes: &[u8]) -> TaskResult<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent.canonicalize().map_err(string)?;
    let destination = parent.join(path.file_name().ok_or("Supply a new draft JSON file")?);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(string)?
        .as_nanos();
    let staging = parent.join(format!(
        ".agenturm-draft-{}-{stamp}.pending",
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&staging).map_err(string)?;
    let published = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::hard_link(&staging, &destination));
    let removed = std::fs::remove_file(&staging);
    published.map_err(string)?;
    removed.map_err(string)?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(string)?;
    Ok(())
}

fn hash(value: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn hex(value: [u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
