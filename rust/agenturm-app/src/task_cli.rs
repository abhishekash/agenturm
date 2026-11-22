//! Saved task commands against the cockpit's local workspace ledger.

mod continuity;
pub(super) use continuity::TaskPromptVerifier;

use agenturm_kernel::ExtensionId;
use agenturm_store::{
    MAX_WORKSPACE_PAGE_SIZE, SavedHandoff, SavedTask, SavedTaskDecision, Store, TaskBriefUpdate,
    TaskDecisionInput, TaskDecisionKind, WorkspaceAnchor, WorkspaceError, WorkspaceGitSnapshot,
    WorkspaceLedger,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const DEFAULT_LIMIT: usize = 100;
const MAX_TEXT_INPUT: u64 = 4 * 1024;
const MAX_BRIEF_INPUT: u64 = 128 * 1024;
pub(super) const USAGE: &str = concat!(
    "usage: agenturm task list --db <history> [--all] [--limit <1..200>] [--before <id>]\n",
    "       agenturm task add --db <history> --title <title> [--notes <text>|--notes-file <path>] [--workspace <directory>]\n",
    "       agenturm task show <task-id> --db <history>\n",
    "       agenturm task <complete|reopen> <task-id> --db <history> [--expected-revision <n>]\n",
    "       agenturm task snapshot <task-id> --db <history> [--include-diff] [--expected-revision <n>]\n",
    "       agenturm task update <task-id> --db <history> [--title <title>] [--notes <text>|--notes-file <path>] [--workspace <directory>] [--expected-revision <n>]\n",
    "       agenturm task archive <task-id> --db <history> [--restore] [--expected-revision <n>]\n",
    "       agenturm task brief <task-id> --db <history> [--file <json>|--intent <text> ...] [--expected-revision <n>]\n",
    "       agenturm task decide <task-id> --db <history> --kind <accepted|rejected> --text <text>|--text-file <path> [--rationale <text>] [--scope <text>] [--supersedes <decision-id>] [--source-file <draft-json> --config <sources>] [--expected-revision <n>]\n",
    "       agenturm task decisions <task-id> --db <history>\n",
    "       agenturm task draft --db <history> --config <sources> --source-id <id> --session-id <id> --turn-id <id> [--worker-id <id>] --event-id <id> --revision <n> [--task-id <id>] [--output <new-json>]\n",
    "       agenturm task approve-draft <draft-json> --db <history> --config <sources> [--title <title>] [--workspace <directory>] [--expected-revision <n>]\n",
    "       agenturm task continuity <task-id> --db <history> [--config <sources> --approval-id <id>]\n",
    "       agenturm task handoff <task-id> --db <history> [--format <markdown|html|json>] [--output <new-directory>] [--include-decision-history] [--include-diff]\n",
    "       agenturm task <link|unlink> <task-id> --db <history> --source-id <id> --session-id <id> [--turn-id <id> [--worker-id <id>]] [--expected-revision <n>]\n",
    "       agenturm task handoffs --db <history> [--limit <1..200>] [--before <id>]\n",
    "       agenturm task export <handoff-id> --db <history>\n",
    "\nBrief fields are intent, acceptance_criteria, constraints, open_questions and next_action. Use hyphens in flags.\n",
    "Create is an alias for add. Results are JSON except task handoff, which defaults to Markdown.\n",
    "Handoff --output and export write private Markdown and HTML pairs.\n",
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskOptions {
    request: Option<(PathBuf, TaskAction)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TaskAction {
    List {
        limit: usize,
        before: Option<u64>,
        all: bool,
    },
    Add {
        title: String,
        note: Option<TextInput>,
        workspace: Option<String>,
    },
    Show(u64),
    Update {
        id: u64,
        title: Option<String>,
        note: Option<TextInput>,
        workspace: Option<String>,
        expected: Option<u64>,
    },
    Archive {
        id: u64,
        archived: bool,
        expected: Option<u64>,
    },
    Snapshot {
        id: u64,
        expected: Option<u64>,
        include_diff: bool,
    },
    Brief {
        id: u64,
        patch: BriefInput,
        expected: Option<u64>,
    },
    Decide {
        id: u64,
        kind: TaskDecisionKind,
        text: TextInput,
        rationale: String,
        scope: String,
        supersedes_id: Option<u64>,
        expected: Option<u64>,
        source: Option<continuity::DecisionSource>,
    },
    Decisions(u64),
    Draft(continuity::DraftRequest),
    ApproveDraft(continuity::ApprovalRequest),
    Continuity {
        id: u64,
        reveal: Option<(Option<PathBuf>, u64)>,
    },
    Handoff {
        id: u64,
        format: HandoffFormat,
        destination: Option<PathBuf>,
        include_history: bool,
        include_diff: bool,
    },
    SetCompleted {
        id: u64,
        completed: bool,
        expected: Option<u64>,
    },
    Link {
        id: u64,
        anchor: WorkspaceAnchor,
        add: bool,
        expected: Option<u64>,
    },
    Handoffs {
        limit: usize,
        before: Option<u64>,
    },
    Export(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TextInput {
    Value(String),
    File(PathBuf),
}

impl TextInput {
    fn load(&self) -> Result<String, String> {
        match self {
            Self::Value(text) => Ok(text.clone()),
            Self::File(path) => read_input(path, MAX_TEXT_INPUT),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum BriefInput {
    Fields(TaskBriefUpdate),
    File(PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandoffFormat {
    Markdown,
    Html,
    Json,
}

pub(super) fn parse(args: &[String]) -> Result<TaskOptions, String> {
    let Some((verb, mut args)) = args.split_first() else {
        return Err(USAGE.into());
    };
    if matches!(verb.as_str(), "--help" | "-h" | "help") && args.is_empty() {
        return Ok(TaskOptions { request: None });
    }

    let mut id = None;
    let mut draft_file = None;
    let mut flags = BTreeMap::new();
    while let Some((flag, rest)) = args.split_first() {
        if !flag.starts_with("--") {
            if verb == "approve-draft" {
                if flag.is_empty() || draft_file.replace(PathBuf::from(flag)).is_some() {
                    return Err("Supply one draft JSON path".into());
                }
            } else if id.replace(parse_id(flag)?).is_some() {
                return Err("Supply at most one task or handoff ID".into());
            }
            args = rest;
            continue;
        }
        if matches!(
            flag.as_str(),
            "--all" | "--restore" | "--json" | "--include-decision-history" | "--include-diff"
        ) {
            if flags.insert(flag.as_str(), "").is_some() {
                return Err(format!("Duplicate option {flag}"));
            }
            args = rest;
            continue;
        }
        let (value, rest) = rest
            .split_first()
            .ok_or_else(|| format!("Missing value for {flag}"))?;
        if value.starts_with("--") {
            return Err(format!("Missing value for {flag}"));
        }
        if flags.insert(flag.as_str(), value.as_str()).is_some() {
            return Err(format!("Duplicate option {flag}"));
        }
        args = rest;
    }

    let db = required_flag(&mut flags, "--db")?.into();
    let json_output = flags.remove("--json").is_some();
    let action = match (verb.as_str(), id) {
        (verb @ ("list" | "handoffs"), None) => {
            let limit = flags
                .remove("--limit")
                .map(|value| {
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|value| (1..=MAX_WORKSPACE_PAGE_SIZE).contains(value))
                        .ok_or("--limit must be an integer from 1 to 200")
                })
                .transpose()?
                .unwrap_or(DEFAULT_LIMIT);
            let before = flags.remove("--before").map(parse_id).transpose()?;
            if verb == "list" {
                TaskAction::List {
                    limit,
                    before,
                    all: flags.remove("--all").is_some(),
                }
            } else {
                TaskAction::Handoffs { limit, before }
            }
        }
        ("add" | "create", None) => TaskAction::Add {
            title: required_flag(&mut flags, "--title")?.into(),
            note: text_input(&mut flags, "--notes", "--notes-file")?,
            workspace: flags.remove("--workspace").map(str::to_owned),
        },
        ("show", Some(id)) => TaskAction::Show(id),
        ("update", Some(id)) => {
            let title = flags.remove("--title").map(str::to_owned);
            let note = text_input(&mut flags, "--notes", "--notes-file")?;
            let workspace = flags.remove("--workspace").map(str::to_owned);
            if title.is_none() && note.is_none() && workspace.is_none() {
                return Err("Choose --title, --notes, --notes-file, or --workspace".into());
            }
            TaskAction::Update {
                id,
                title,
                note,
                workspace,
                expected: expected_revision(&mut flags)?,
            }
        }
        ("archive", Some(id)) => TaskAction::Archive {
            id,
            archived: flags.remove("--restore").is_none(),
            expected: expected_revision(&mut flags)?,
        },
        ("snapshot", Some(id)) => TaskAction::Snapshot {
            id,
            expected: expected_revision(&mut flags)?,
            include_diff: flags.remove("--include-diff").is_some(),
        },
        ("brief", Some(id)) => {
            let patch = TaskBriefUpdate {
                intent: flags.remove("--intent").map(str::to_owned),
                acceptance_criteria: flags.remove("--acceptance-criteria").map(str::to_owned),
                constraints: flags.remove("--constraints").map(str::to_owned),
                open_questions: flags.remove("--open-questions").map(str::to_owned),
                next_action: flags.remove("--next-action").map(str::to_owned),
            };
            let patch = if let Some(file) = flags.remove("--file") {
                if patch != TaskBriefUpdate::default() {
                    return Err("Use --file or individual brief fields".into());
                }
                BriefInput::File(file.into())
            } else {
                BriefInput::Fields(patch)
            };
            TaskAction::Brief {
                id,
                patch,
                expected: expected_revision(&mut flags)?,
            }
        }
        ("decide", Some(id)) => {
            let kind = match required_flag(&mut flags, "--kind")? {
                "accepted" => TaskDecisionKind::Accepted,
                "rejected" => TaskDecisionKind::Rejected,
                _ => return Err("--kind must be accepted or rejected".into()),
            };
            let text = text_input(&mut flags, "--text", "--text-file")?
                .ok_or("Supply --text or --text-file")?;
            let source = flags
                .remove("--source-file")
                .map(|file| -> Result<_, String> {
                    if file.is_empty() {
                        return Err("Supply a draft JSON path for --source-file".into());
                    }
                    Ok(continuity::DecisionSource {
                        file: file.into(),
                        config: flags.remove("--config").map(PathBuf::from),
                    })
                })
                .transpose()?;
            TaskAction::Decide {
                id,
                kind,
                text,
                rationale: flags.remove("--rationale").unwrap_or_default().into(),
                scope: flags.remove("--scope").unwrap_or_default().into(),
                supersedes_id: flags.remove("--supersedes").map(parse_id).transpose()?,
                expected: expected_revision(&mut flags)?,
                source,
            }
        }
        ("decisions", Some(id)) => TaskAction::Decisions(id),
        ("draft", None) => TaskAction::Draft(continuity::DraftRequest {
            source: continuity::ExactSource {
                anchor: WorkspaceAnchor {
                    harness_id: ExtensionId::parse(required_flag(&mut flags, "--source-id")?)
                        .map_err(string)?,
                    session_id: required_flag(&mut flags, "--session-id")?.into(),
                    turn_id: Some(required_flag(&mut flags, "--turn-id")?.into()),
                    worker_id: flags.remove("--worker-id").map(str::to_owned),
                },
                event_id: required_flag(&mut flags, "--event-id")?.into(),
                revision: required_flag(&mut flags, "--revision")?
                    .parse()
                    .map_err(|_| "--revision must be an unsigned 32-bit integer")?,
            },
            config: flags.remove("--config").map(PathBuf::from),
            task_id: flags.remove("--task-id").map(parse_id).transpose()?,
            output: flags.remove("--output").map(PathBuf::from),
        }),
        ("approve-draft", None) => TaskAction::ApproveDraft(continuity::ApprovalRequest {
            file: draft_file.ok_or("Supply the draft JSON file")?,
            config: flags.remove("--config").map(PathBuf::from),
            title: flags.remove("--title").map(str::to_owned),
            workspace: flags.remove("--workspace").map(str::to_owned),
            expected_revision: expected_revision(&mut flags)?,
        }),
        ("continuity", Some(id)) => TaskAction::Continuity {
            id,
            reveal: match (flags.remove("--config"), flags.remove("--approval-id")) {
                (None, None) => None,
                (config, Some(approval)) => Some((config.map(PathBuf::from), parse_id(approval)?)),
                _ => return Err("Use --approval-id to reveal a saved excerpt".into()),
            },
        },
        ("handoff", Some(id)) => TaskAction::Handoff {
            id,
            format: match (flags.remove("--format"), json_output) {
                (None, true) | (Some("json"), _) => HandoffFormat::Json,
                (None | Some("markdown"), false) => HandoffFormat::Markdown,
                (Some("html"), false) => HandoffFormat::Html,
                _ => return Err("Choose one handoff format: markdown, html, or json".into()),
            },
            destination: flags.remove("--output").map(PathBuf::from),
            include_history: flags.remove("--include-decision-history").is_some(),
            include_diff: flags.remove("--include-diff").is_some(),
        },
        (verb @ ("complete" | "reopen"), Some(id)) => TaskAction::SetCompleted {
            id,
            completed: verb == "complete",
            expected: expected_revision(&mut flags)?,
        },
        (verb @ ("link" | "unlink"), Some(id)) => {
            let worker_id = flags.remove("--worker-id").map(str::to_owned);
            let turn_id = flags.remove("--turn-id").map(str::to_owned);
            if worker_id.is_some() && turn_id.is_none() {
                return Err("--worker-id requires --turn-id".into());
            }
            TaskAction::Link {
                id,
                anchor: WorkspaceAnchor {
                    harness_id: ExtensionId::parse(required_flag(&mut flags, "--source-id")?)
                        .map_err(|error| error.to_string())?,
                    session_id: required_flag(&mut flags, "--session-id")?.into(),
                    worker_id,
                    turn_id,
                },
                add: verb == "link",
                expected: expected_revision(&mut flags)?,
            }
        }
        ("export", Some(id)) => TaskAction::Export(id),
        _ => return Err(USAGE.into()),
    };
    if let Some(flag) = flags.keys().next() {
        return Err(format!("Unexpected option {flag} for task {verb}"));
    }
    Ok(TaskOptions {
        request: Some((db, action)),
    })
}

fn required_flag<'a>(flags: &mut BTreeMap<&str, &'a str>, name: &str) -> Result<&'a str, String> {
    flags
        .remove(name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Supply {name}"))
}

fn text_input(
    flags: &mut BTreeMap<&str, &str>,
    inline: &str,
    file: &str,
) -> Result<Option<TextInput>, String> {
    match (flags.remove(inline), flags.remove(file)) {
        (Some(_), Some(_)) => Err(format!("Use {inline} or {file}")),
        (Some(text), None) => Ok(Some(TextInput::Value(text.into()))),
        (None, Some(path)) if !path.is_empty() => Ok(Some(TextInput::File(path.into()))),
        (None, Some(_)) => Err(format!("Supply a path for {file}")),
        (None, None) => Ok(None),
    }
}

fn expected_revision(flags: &mut BTreeMap<&str, &str>) -> Result<Option<u64>, String> {
    flags
        .remove("--expected-revision")
        .map(|value| {
            parse_id(value).map_err(|_| "--expected-revision must be a positive integer".into())
        })
        .transpose()
}

fn parse_id(value: &str) -> Result<u64, String> {
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .map(|id| id as u64)
        .ok_or_else(|| "Task, handoff, and cursor IDs must be positive integers".into())
}

#[derive(Debug)]
pub struct TaskExecutionError {
    message: String,
    conflict: bool,
}

impl TaskExecutionError {
    pub fn exit_code(&self) -> u8 {
        if self.conflict { 3 } else { 2 }
    }
}

impl std::fmt::Display for TaskExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TaskExecutionError {}

impl From<String> for TaskExecutionError {
    fn from(message: String) -> Self {
        Self {
            message,
            conflict: false,
        }
    }
}

impl From<&str> for TaskExecutionError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl From<TaskExecutionError> for String {
    fn from(error: TaskExecutionError) -> Self {
        error.message
    }
}

impl From<WorkspaceError> for TaskExecutionError {
    fn from(error: WorkspaceError) -> Self {
        Self {
            conflict: matches!(error, WorkspaceError::TaskRevisionConflict { .. }),
            message: error.to_string(),
        }
    }
}

type TaskResult<T> = Result<T, TaskExecutionError>;

pub fn execute_task_stdout(options: &TaskOptions) -> TaskResult<()> {
    with_task_stdout(options, |output| execute_task(options, output))
}

pub(super) fn with_task_stdout(
    options: &TaskOptions,
    execute: impl FnOnce(&mut dyn Write) -> TaskResult<()>,
) -> TaskResult<()> {
    if matches!(
        options.request.as_ref().map(|(_, action)| action),
        Some(
            TaskAction::Draft(_)
                | TaskAction::Continuity {
                    reveal: Some(_),
                    ..
                }
        )
    ) {
        crate::with_private_stdout(execute)
    } else {
        execute(&mut std::io::stdout().lock())
    }
}

/// A sink used to publish a source-backed task draft or excerpt must
/// complete its writes and flush promptly or fail without deferred output.
pub fn execute_task(options: &TaskOptions, mut output: impl Write) -> TaskResult<()> {
    let Some((db, _)) = &options.request else {
        return Ok(output.write_all(USAGE.as_bytes()).map_err(string)?);
    };
    Store::open_read_only(db).map_err(string)?;
    let ledger = WorkspaceLedger::open(db)?;
    execute_ledger(options, &ledger, db, None, output)
}

pub(super) fn execute_ledger(
    options: &TaskOptions,
    ledger: &WorkspaceLedger,
    workspace: &Path,
    source_verifier: Option<&dyn TaskPromptVerifier>,
    mut output: impl Write,
) -> TaskResult<()> {
    let Some((_, action)) = &options.request else {
        return Ok(output.write_all(USAGE.as_bytes()).map_err(string)?);
    };
    let authority = continuity::SourceAccess {
        history: workspace,
        verifier: source_verifier,
    };
    let result = match action {
        TaskAction::List { limit, before, all } => {
            let page = ledger.list_tasks_including_archived(*limit, *before, *all)?;
            json!({
                "items": page.items.iter().map(task_json).collect::<Vec<_>>(),
                "next_before": page.next_before,
            })
        }
        TaskAction::Add {
            title,
            note,
            workspace,
        } => {
            let note = note
                .as_ref()
                .map(TextInput::load)
                .transpose()?
                .unwrap_or_default();
            let directory = workspace_directory(workspace.as_deref())?;
            task_json(&ledger.create_task_in_workspace(None, title, &note, directory.as_deref())?)
        }
        TaskAction::Show(id) => {
            let task = task(ledger, *id)?;
            let links = ledger
                .task_links(*id, MAX_WORKSPACE_PAGE_SIZE)
                .map_err(string)?;
            let workspace = ledger.task_workspace_snapshot(*id).map_err(string)?;
            json!({
                "task": task_json(&task),
                "links": links.iter().map(anchor_json).collect::<Vec<_>>(),
                "links_limit": MAX_WORKSPACE_PAGE_SIZE,
                "git_snapshot": workspace.as_ref().map(git_snapshot_json),
            })
        }
        TaskAction::Update {
            id,
            title,
            note,
            workspace,
            expected,
        } => {
            let note = note.as_ref().map(TextInput::load).transpose()?;
            let workspace = workspace_directory(workspace.as_deref())?;
            let task = ledger
                .update_task_fields(
                    *id,
                    title.as_deref(),
                    note.as_deref(),
                    workspace.as_deref(),
                    *expected,
                )?
                .ok_or(MISSING_TASK)?;
            task_json(&task)
        }
        TaskAction::Archive {
            id,
            archived,
            expected,
        } => {
            let task = ledger
                .set_task_archived(*id, *archived, *expected)?
                .ok_or(MISSING_TASK)?;
            task_json(&task)
        }
        TaskAction::Snapshot {
            id,
            expected,
            include_diff,
        } => {
            let task = task(ledger, *id)?;
            if let Some(expected) = expected
                && *expected != task.revision
            {
                return Err(WorkspaceError::TaskRevisionConflict {
                    expected: *expected,
                    current: task.revision,
                }
                .into());
            }
            let previous = ledger.task_workspace_snapshot(*id)?;
            let directory = task
                .workspace_directory
                .as_deref()
                .or_else(|| previous.as_ref().map(|item| item.repository_root.as_str()))
                .ok_or("Set a local workspace directory with task update --workspace")?;
            let (git, diff) = if *include_diff {
                let (git, diff) = super::task_git::capture_with_diff(Path::new(directory))?;
                (git, Some(diff))
            } else {
                (super::task_git::capture(Path::new(directory))?, None)
            };
            let task = ledger
                .save_task_workspace_capture_if_current(
                    *id,
                    &git,
                    diff.as_ref(),
                    Some(task.revision),
                )?
                .ok_or(MISSING_TASK)?;
            let mut summary =
                json!({ "task": task_json(&task), "git_snapshot": git_snapshot_json(&git) });
            if let Some(diff) = diff {
                summary["git_diff_bytes"] = json!(diff.byte_len());
            }
            summary
        }
        TaskAction::Brief {
            id,
            patch,
            expected,
        } => {
            let loaded;
            let patch = match patch {
                BriefInput::Fields(patch) => patch,
                BriefInput::File(path) => {
                    loaded = serde_json::from_str(&read_input(path, MAX_BRIEF_INPUT)?).map_err(
                        |_| "Brief file must be one JSON object with supported text fields",
                    )?;
                    &loaded
                }
            };
            let (task, brief) = if patch == &TaskBriefUpdate::default() {
                ledger.task_brief(*id)?
            } else {
                ledger.update_task_brief(*id, patch, *expected)?
            }
            .ok_or(MISSING_TASK)?;
            json!({ "task_id": id, "revision": task.revision, "brief": brief })
        }
        TaskAction::Decide {
            id,
            kind,
            text,
            rationale,
            scope,
            supersedes_id,
            expected,
            source,
        } => {
            let input = TaskDecisionInput {
                kind: *kind,
                text: text.load()?,
                rationale: rationale.clone(),
                scope: scope.clone(),
                supersedes_id: *supersedes_id,
            };
            let (task, decision) = if let Some(source) = source {
                continuity::add_decision(&authority, ledger, *id, *expected, &input, source)?
            } else {
                ledger
                    .add_task_decision(*id, &input, *expected)?
                    .ok_or(MISSING_TASK)?
            };
            json!({ "task_id": id, "revision": task.revision, "decision": decision })
        }
        TaskAction::Decisions(id) => {
            let (task, decisions) = ledger.task_decisions(*id)?.ok_or(MISSING_TASK)?;
            json!({ "task_id": id, "revision": task.revision, "decisions": decision_history_json(&decisions, true) })
        }
        TaskAction::Draft(request) => {
            return continuity::draft(&authority, ledger, request, &mut output);
        }
        TaskAction::ApproveDraft(request) => continuity::approve(&authority, ledger, request)?,
        TaskAction::Continuity { id, reveal } => {
            return continuity::read(
                &authority,
                ledger,
                *id,
                reveal.as_ref().map(|(config, id)| (config.as_deref(), *id)),
                &mut output,
            );
        }
        TaskAction::Handoff {
            id,
            format,
            destination,
            include_history,
            include_diff,
        } => {
            let handoff = ledger
                .task_handoff_with_git_diff(*id, *include_diff)?
                .ok_or(MISSING_TASK)?;
            if *include_diff && handoff.git_diff.is_none() {
                return Err("Capture a Git patch with task snapshot --include-diff before including it in a handoff".into());
            }
            if let Some(path) = destination {
                let published =
                    super::handoff_export::export_task(path, &handoff, *include_history)?;
                json!({ "task_id": id, "revision": handoff.task.revision,
                    "markdown": published.join("handoff.md").display().to_string(),
                    "html": published.join("handoff.html").display().to_string(), })
            } else if *format == HandoffFormat::Json {
                let mut document = json!({ "task": task_json(&handoff.task), "brief": handoff.brief,
                    "links": handoff.links.iter().map(anchor_json).collect::<Vec<_>>(),
                    "links_truncated": handoff.links_truncated,
                    "decisions": decision_history_json(&handoff.decisions, *include_history),
                    "continuity": handoff.continuity,
                    "git_snapshot": handoff.git_snapshot.as_ref().map(git_snapshot_json), });
                if let Some(diff) = handoff.git_diff {
                    document["git_diff"] = serde_json::to_value(diff).map_err(string)?;
                }
                document
            } else {
                let text = super::handoff_export::render_task(
                    &handoff,
                    *include_history,
                    *format == HandoffFormat::Html,
                );
                return Ok(output.write_all(text.as_bytes()).map_err(string)?);
            }
        }
        TaskAction::SetCompleted {
            id,
            completed,
            expected,
        } => {
            let task = ledger
                .set_task_completed_if_current(*id, *completed, *expected)?
                .ok_or(MISSING_TASK)?;
            json!({ "id": id, "completed": completed, "revision": task.revision })
        }
        TaskAction::Link {
            id,
            anchor,
            add,
            expected,
        } => {
            let (task, changed) = if *add {
                ledger.link_task_if_current(*id, anchor, *expected)
            } else {
                ledger.unlink_task_if_current(*id, anchor, *expected)
            }?
            .ok_or(MISSING_TASK)?;
            json!({ "id": id, "anchor": anchor_json(anchor), "changed": changed, "revision": task.revision })
        }
        TaskAction::Handoffs { limit, before } => {
            let page = ledger.list_handoffs(*limit, *before).map_err(string)?;
            json!({
                "items": page.items.iter().map(handoff_json).collect::<Vec<_>>(),
                "next_before": page.next_before,
            })
        }
        TaskAction::Export(id) => {
            let handoff = ledger
                .list_handoffs(1, before_id(*id))
                .map_err(string)?
                .items
                .into_iter()
                .next()
                .filter(|item| item.id == *id)
                .ok_or("That handoff is no longer available")?;
            let destination = super::handoff_export::export(workspace, &handoff)?;
            json!({
                "id": id,
                "markdown": destination.join("handoff.md").display().to_string(),
                "html": destination.join("handoff.html").display().to_string(),
            })
        }
    };
    serde_json::to_writer(&mut output, &result).map_err(string)?;
    Ok(output.write_all(b"\n").map_err(string)?)
}

const MISSING_TASK: &str = "That task is no longer available";

fn task(ledger: &WorkspaceLedger, id: u64) -> TaskResult<SavedTask> {
    Ok(ledger.task(id)?.ok_or(MISSING_TASK)?)
}

fn before_id(id: u64) -> Option<u64> {
    // With no cursor the ledger includes the largest representable SQLite ID.
    id.checked_add(1)
        .filter(|value| i64::try_from(*value).is_ok())
}

fn task_json(item: &SavedTask) -> Value {
    json!({
        "id": item.id,
        "anchor": item.anchor.as_ref().map(anchor_json),
        "title": item.title,
        "notes": item.note,
        "completed": item.completed,
        "archived": item.archived,
        "revision": item.revision,
        "workspace_directory": item.workspace_directory,
    })
}

fn git_snapshot_json(item: &WorkspaceGitSnapshot) -> Value {
    json!({
        "repository_root": item.repository_root,
        "branch": item.branch,
        "head_commit": item.head_commit,
        "staged_changes": item.staged_changes,
        "unstaged_changes": item.unstaged_changes,
        "untracked_files": item.untracked_files,
        "captured_at_unix_ns": item.captured_at_unix_ns,
    })
}

fn decision_history_json(decisions: &[SavedTaskDecision], include_history: bool) -> Vec<Value> {
    let superseded: BTreeSet<_> = decisions
        .iter()
        .filter_map(|item| item.decision.supersedes_id)
        .collect();
    decisions
        .iter()
        .map(|item| (item, !superseded.contains(&item.id)))
        .filter(|(_, current)| include_history || *current)
        .map(|(item, current)| json!({
            "id": item.id, "created_at_unix_ns": item.created_at_unix_ns, "kind": item.decision.kind,
            "text": item.decision.text, "rationale": item.decision.rationale, "scope": item.decision.scope,
            "supersedes_id": item.decision.supersedes_id, "current": current,
            "source": item.source,
        }))
        .collect()
}

fn workspace_directory(path: Option<&str>) -> Result<Option<String>, String> {
    path.map(|path| {
        if path.is_empty() {
            return Ok(String::new());
        }
        std::path::absolute(path)
            .map_err(string)?
            .into_os_string()
            .into_string()
            .map_err(|_| "Task workspace directory must be UTF-8".into())
    })
    .transpose()
}

fn read_input(path: &Path, maximum: u64) -> Result<String, String> {
    if !path.is_file() {
        return Err("Task input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(string)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(string)?;
    if bytes.len() as u64 > maximum {
        return Err(format!("Task input exceeds {maximum} bytes"));
    }
    String::from_utf8(bytes).map_err(|_| "Task input must be UTF-8".into())
}

fn handoff_json(item: &SavedHandoff) -> Value {
    json!({
        "id": item.id,
        "from": anchor_json(&item.from),
        "to": anchor_json(&item.to),
        "notes": item.note,
    })
}

fn anchor_json(anchor: &WorkspaceAnchor) -> Value {
    json!({
        "source_id": anchor.harness_id.as_str(),
        "session_id": anchor.session_id,
        "worker_id": anchor.worker_id,
        "turn_id": anchor.turn_id,
    })
}

fn string(error: impl std::fmt::Display) -> String {
    error.to_string()
}
