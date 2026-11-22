//! Saved task commands against the cockpit's local workspace ledger.

use agenturm_kernel::ExtensionId;
use agenturm_store::{
    MAX_WORKSPACE_PAGE_SIZE, SavedHandoff, SavedTask, Store, WorkspaceAnchor, WorkspaceLedger,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

const DEFAULT_LIMIT: usize = 100;
pub(super) const USAGE: &str = concat!(
    "usage: agenturm task list --db <history> [--limit <1..200>] [--before <id>]\n",
    "       agenturm task add --db <history> --title <title> [--notes <text>]\n",
    "       agenturm task <show|complete|reopen> <task-id> --db <history>\n",
    "       agenturm task <link|unlink> <task-id> --db <history> --source-id <id> --session-id <id> [--turn-id <id> [--worker-id <id>]]\n",
    "       agenturm task handoffs --db <history> [--limit <1..200>] [--before <id>]\n",
    "       agenturm task export <handoff-id> --db <history>\n",
    "\nResults are JSON. Export writes private Markdown and HTML beside the history database.\n",
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
    },
    Add {
        title: String,
        note: String,
    },
    Show(u64),
    SetCompleted {
        id: u64,
        completed: bool,
    },
    Link {
        id: u64,
        anchor: WorkspaceAnchor,
        add: bool,
    },
    Handoffs {
        limit: usize,
        before: Option<u64>,
    },
    Export(u64),
}

pub(super) fn parse(args: &[String]) -> Result<TaskOptions, String> {
    let Some((verb, mut args)) = args.split_first() else {
        return Err(USAGE.into());
    };
    if matches!(verb.as_str(), "--help" | "-h" | "help") && args.is_empty() {
        return Ok(TaskOptions { request: None });
    }

    let mut id = None;
    let mut flags = BTreeMap::new();
    while let Some((flag, rest)) = args.split_first() {
        if !flag.starts_with("--") {
            if id.replace(parse_id(flag)?).is_some() {
                return Err("Supply at most one task or handoff ID".into());
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
                TaskAction::List { limit, before }
            } else {
                TaskAction::Handoffs { limit, before }
            }
        }
        ("add", None) => TaskAction::Add {
            title: required_flag(&mut flags, "--title")?.into(),
            note: flags.remove("--notes").unwrap_or_default().into(),
        },
        ("show", Some(id)) => TaskAction::Show(id),
        (verb @ ("complete" | "reopen"), Some(id)) => TaskAction::SetCompleted {
            id,
            completed: verb == "complete",
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

fn parse_id(value: &str) -> Result<u64, String> {
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .map(|id| id as u64)
        .ok_or_else(|| "Task, handoff, and cursor IDs must be positive integers".into())
}

pub fn execute_task(options: &TaskOptions, mut output: impl Write) -> Result<(), String> {
    let Some((db, action)) = &options.request else {
        return output.write_all(USAGE.as_bytes()).map_err(string);
    };
    Store::open_read_only(db).map_err(string)?;
    let ledger = WorkspaceLedger::open(db).map_err(string)?;
    let result = match action {
        TaskAction::List { limit, before } => {
            let page = ledger.list_tasks(*limit, *before).map_err(string)?;
            json!({
                "items": page.items.iter().map(task_json).collect::<Vec<_>>(),
                "next_before": page.next_before,
            })
        }
        TaskAction::Add { title, note } => {
            task_json(&ledger.create_task(None, title, note).map_err(string)?)
        }
        TaskAction::Show(id) => {
            let task = task(&ledger, *id)?;
            let links = ledger
                .task_links(*id, MAX_WORKSPACE_PAGE_SIZE)
                .map_err(string)?;
            let workspace = ledger.task_workspace_snapshot(*id).map_err(string)?;
            json!({
                "task": task_json(&task),
                "links": links.iter().map(anchor_json).collect::<Vec<_>>(),
                "links_limit": MAX_WORKSPACE_PAGE_SIZE,
                "git_snapshot": workspace.map(|item| json!({
                    "repository_root": item.repository_root,
                    "branch": item.branch,
                    "head_commit": item.head_commit,
                    "staged_changes": item.staged_changes,
                    "unstaged_changes": item.unstaged_changes,
                    "untracked_files": item.untracked_files,
                    "captured_at_unix_ns": item.captured_at_unix_ns,
                })),
            })
        }
        TaskAction::SetCompleted { id, completed } => {
            if !ledger.set_task_completed(*id, *completed).map_err(string)? {
                return Err("That task is no longer available".into());
            }
            json!({ "id": id, "completed": completed })
        }
        TaskAction::Link { id, anchor, add } => {
            task(&ledger, *id)?;
            let changed = if *add {
                ledger.link_task(*id, anchor)
            } else {
                ledger.unlink_task(*id, anchor)
            }
            .map_err(string)?;
            json!({ "id": id, "anchor": anchor_json(anchor), "changed": changed })
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
            let destination = super::handoff_export::export(db, &handoff)?;
            json!({
                "id": id,
                "markdown": destination.join("handoff.md").display().to_string(),
                "html": destination.join("handoff.html").display().to_string(),
            })
        }
    };
    serde_json::to_writer(&mut output, &result).map_err(string)?;
    output.write_all(b"\n").map_err(string)
}

fn task(ledger: &WorkspaceLedger, id: u64) -> Result<SavedTask, String> {
    ledger
        .list_tasks(1, before_id(id))
        .map_err(string)?
        .items
        .into_iter()
        .next()
        .filter(|item| item.id == id)
        .ok_or_else(|| "That task is no longer available".into())
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
    })
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
