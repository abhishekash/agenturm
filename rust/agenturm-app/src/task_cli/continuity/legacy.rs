//! Local task drafts backed by live, exact Python receiver evidence.

use super::{
    DraftTarget, MAX_DRAFT_BYTES, TaskResult, hash, hex, read_input, write_draft_file, write_json,
};
use crate::remote_history::legacy_review::continuity as evidence;
use crate::{
    RemoteHistoryOptions,
    task_cli::{
        MISSING_TASK, expected_revision, parse_id, required_flag, string, task, task_json,
        workspace_directory,
    },
};
use agenturm_store::{
    ApprovedTaskSource, LegacyTaskSourceProof, TaskDraftApproval, WorkspaceError, WorkspaceLedger,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const FORMAT: &str = "agenturm-rust-python-task-draft/1";
const MAX_REVIEW_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::task_cli) struct Options {
    receiver: RemoteHistoryOptions,
    action: Action,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Action {
    Draft {
        receipt: PathBuf,
        event_id: Option<String>,
        task_id: Option<u64>,
        output: Option<PathBuf>,
    },
    Approve {
        file: PathBuf,
        title: Option<String>,
        workspace: Option<String>,
        expected_revision: Option<u64>,
    },
    Reveal {
        task_id: u64,
        approval_id: u64,
    },
}

pub(in crate::task_cli) fn parse(
    verb: &str,
    id: Option<u64>,
    draft_file: Option<PathBuf>,
    flags: &mut BTreeMap<&str, &str>,
) -> Result<Options, String> {
    let mut receiver = Vec::new();
    for flag in ["--endpoint", "--token-file", "--receiver-protocol"] {
        receiver.extend([flag.to_owned(), required_flag(flags, flag)?.to_owned()]);
    }
    let receiver = crate::remote_history::parse_attach(&receiver)?;
    if !receiver.is_python() {
        return Err("Use --receiver-protocol python-v0.2.8 for Python-backed task evidence".into());
    }
    let action = match (verb, id) {
        ("draft", None) => Action::Draft {
            receipt: required_flag(flags, "--review-file")?.into(),
            event_id: flags.remove("--event-id").map(str::to_owned),
            task_id: flags.remove("--task-id").map(parse_id).transpose()?,
            output: flags.remove("--output").map(PathBuf::from),
        },
        ("approve-draft", None) => Action::Approve {
            file: draft_file.ok_or("Supply the draft JSON file")?,
            title: flags.remove("--title").map(str::to_owned),
            workspace: flags.remove("--workspace").map(str::to_owned),
            expected_revision: expected_revision(flags)?,
        },
        ("continuity", Some(task_id)) => Action::Reveal { task_id, approval_id: parse_id(required_flag(flags, "--approval-id")?)? },
        _ => return Err("Python receiver options are accepted for task draft, approve-draft or continuity --approval-id".into()),
    };
    Ok(Options { receiver, action })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    format: String,
    history_binding: String,
    prepared_at_unix_ns: u64,
    target: Option<DraftTarget>,
    source: LegacyTaskSourceProof,
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
            .map_err(|_| "Choose a Rust Python-backed task draft JSON file")?;
        if draft.format != FORMAT
            || draft.binding()? != draft.draft_sha256
            || hash(&draft.excerpt) != draft.source.excerpt_sha256
        {
            return Err(
                "Draft evidence changed. Prepare a new draft from the exact Review.".into(),
            );
        }
        if draft.history_binding != hex(ledger.task_draft_binding()?) {
            return Err("Draft belongs to another task history".into());
        }
        Ok(draft)
    }
}

pub(in crate::task_cli) fn execute(
    options: &Options,
    ledger: &WorkspaceLedger,
    output: &mut dyn Write,
) -> TaskResult<()> {
    match &options.action {
        Action::Draft {
            receipt,
            event_id,
            task_id,
            output: destination,
        } => {
            let receipt = read_input(receipt, MAX_REVIEW_BYTES)?;
            let selection = evidence::Selection::from_receipt(&receipt, event_id.as_deref())?;
            let target = task_id.map(|id| task(ledger, id)).transpose()?;
            evidence::with_selection(&options.receiver, &selection, |proof, excerpt| {
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .and_then(|time| u64::try_from(time.as_nanos()).ok())
                    .ok_or("Cannot timestamp the draft")?;
                let mut draft = Draft {
                    format: FORMAT.into(),
                    history_binding: hex(ledger.task_draft_binding()?),
                    prepared_at_unix_ns: stamp,
                    target: target.as_ref().map(|task| DraftTarget {
                        task_id: task.id,
                        revision: task.revision,
                    }),
                    source: proof,
                    draft_sha256: String::new(),
                    excerpt: excerpt.into(),
                    title: target
                        .map_or_else(|| "Continue recorded work".into(), |task| task.title),
                    continuation: String::new(),
                };
                draft.draft_sha256 = draft.binding()?;
                if let Some(path) = destination {
                    let mut bytes = serde_json::to_vec_pretty(&draft).map_err(string)?;
                    bytes.push(b'\n');
                    write_draft_file(path, &bytes)?;
                    write_json(
                        output,
                        &json!({"draft": path, "draft_sha256": draft.draft_sha256}),
                    )
                } else {
                    write_json(output, &draft)
                }
            })
        }
        Action::Approve {
            file,
            title,
            workspace,
            expected_revision,
        } => {
            let draft = Draft::load(file, ledger)?;
            let workspace = workspace_directory(workspace.as_deref())?;
            if draft.target.is_some() && (title.is_some() || workspace.is_some()) {
                return Err("Title and workspace options apply to drafts for new tasks".into());
            }
            if let Some(expected) = expected_revision {
                let current = draft
                    .target
                    .as_ref()
                    .ok_or("A draft for a new task has no expected revision")?
                    .revision;
                if *expected != current {
                    return Err(WorkspaceError::TaskRevisionConflict {
                        expected: *expected,
                        current,
                    }
                    .into());
                }
            }
            let result = evidence::with_proof(&options.receiver, &draft.source, |current, _| {
                let input = TaskDraftApproval {
                    draft_sha256: draft.draft_sha256,
                    source: ApprovedTaskSource::Python(Box::new(current)),
                    continuation: draft.continuation,
                };
                let (task, approval) = if let Some(target) = draft.target {
                    ledger
                        .approve_draft_for_existing_task(target.task_id, target.revision, &input)?
                        .ok_or(MISSING_TASK)?
                } else {
                    ledger.approve_draft_for_new_task(
                        title.as_deref().unwrap_or(&draft.title),
                        workspace.as_deref(),
                        &input,
                    )?
                };
                Ok::<_, super::TaskExecutionError>(
                    json!({"task": task_json(&task), "approval": approval}),
                )
            })?;
            write_json(output, &result)
        }
        Action::Reveal {
            task_id,
            approval_id,
        } => {
            let handoff = ledger.task_handoff(*task_id)?.ok_or(MISSING_TASK)?;
            let approval = handoff
                .continuity
                .iter()
                .find(|approval| approval.id == *approval_id)
                .ok_or("The approved continuity version is unavailable")?;
            let ApprovedTaskSource::Python(source) = &approval.source else {
                return Err("This continuity requires its native source authority".into());
            };
            evidence::with_proof(&options.receiver, source, |_, excerpt| {
                let retained = ledger.task_handoff(*task_id)?.ok_or(MISSING_TASK)?;
                if !retained.continuity.contains(approval) {
                    return Err("The approved continuity version changed".into());
                }
                write_json(
                    output,
                    &json!({"task_id": task_id, "approval": approval, "excerpt": excerpt}),
                )
            })
        }
    }
}
