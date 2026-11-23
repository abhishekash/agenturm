//! Read-only attention snapshots and bounded change notifications.

use crate::{WatchSignalRegistrations, cockpit_workflows::pending_requests_with_seen};
use agenturm_kernel::ObservationStore;
use agenturm_store::{Store, WorkspaceLedger};
use serde::Serialize;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const MAX_POLLS: usize = 10_000;
const USAGE: &str = concat!(
    "usage: agenturm attention status --db <path>\n",
    "       agenturm attention watch --db <path> [--once | --max-polls <1..10000>] ",
    "[--interval-ms <100..60000>]\n",
    "       agenturm attention <status|watch> --endpoint http://<loopback-IP:port> ",
    "--token-file <path> [--receiver-protocol python-v0.2.8] [watch options]\n",
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttentionOptions {
    Status {
        db: PathBuf,
    },
    Watch {
        db: PathBuf,
        interval: Duration,
        max_polls: usize,
    },
    #[cfg(target_os = "linux")]
    Remote {
        receiver: crate::RemoteHistoryOptions,
        watch: Option<(Duration, usize)>,
    },
}

pub(super) fn parse(args: &[String]) -> Result<AttentionOptions, String> {
    #[cfg(target_os = "linux")]
    if args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--endpoint" | "--token-file" | "--receiver-protocol"
        )
    }) {
        return parse_remote(args);
    }
    match args
        .split_first()
        .map(|(verb, values)| (verb.as_str(), values))
    {
        Some(("status", [flag, path])) if flag == "--db" && valid_value(path) => {
            Ok(AttentionOptions::Status { db: path.into() })
        }
        Some(("watch", values)) => {
            let (db, interval, max_polls) = parse_watch(values, true)?;
            Ok(AttentionOptions::Watch {
                db: db.ok_or(USAGE)?,
                interval,
                max_polls,
            })
        }
        _ => Err(USAGE.into()),
    }
}

fn parse_watch(
    mut args: &[String],
    local: bool,
) -> Result<(Option<PathBuf>, Duration, usize), String> {
    let (mut db, mut interval, mut max_polls) = (None, None, None);
    while let Some((flag, rest)) = args.split_first() {
        args = rest;
        if flag == "--once" && max_polls.is_none() {
            max_polls = Some(1);
            continue;
        }
        let (value, rest) = args
            .split_first()
            .filter(|(value, _)| valid_value(value))
            .ok_or(USAGE)?;
        args = rest;
        match flag.as_str() {
            "--db" if local && db.is_none() => db = Some(PathBuf::from(value)),
            "--interval-ms" if interval.is_none() => {
                interval = Some(Duration::from_millis(
                    value
                        .parse()
                        .ok()
                        .filter(|ms| (100..=60_000).contains(ms))
                        .ok_or(USAGE)?,
                ));
            }
            "--max-polls" if max_polls.is_none() => {
                max_polls = Some(
                    value
                        .parse()
                        .ok()
                        .filter(|polls| (1..=MAX_POLLS).contains(polls))
                        .ok_or(USAGE)?,
                );
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok((
        db,
        interval.unwrap_or(Duration::from_secs(2)),
        max_polls.unwrap_or(MAX_POLLS),
    ))
}

#[cfg(target_os = "linux")]
fn parse_remote(args: &[String]) -> Result<AttentionOptions, String> {
    let (verb, mut args) = args.split_first().ok_or(USAGE)?;
    let mut receiver = Vec::new();
    let mut polling = Vec::new();
    while let Some((flag, rest)) = args.split_first() {
        if flag == "--once" {
            polling.push(flag.clone());
            args = rest;
            continue;
        }
        let (value, trailer) = rest
            .split_first()
            .filter(|(value, _)| valid_value(value))
            .ok_or(USAGE)?;
        if matches!(
            flag.as_str(),
            "--endpoint" | "--token-file" | "--receiver-protocol"
        ) {
            receiver.extend([flag.clone(), value.clone()]);
        } else {
            polling.extend([flag.clone(), value.clone()]);
        }
        args = trailer;
    }
    let watch = match verb.as_str() {
        "status" if polling.is_empty() => None,
        "watch" => {
            let (_, interval, limit) = parse_watch(&polling, false)?;
            Some((interval, limit))
        }
        _ => return Err(USAGE.into()),
    };
    Ok(AttentionOptions::Remote {
        receiver: crate::remote_history::parse_attach(&receiver)?,
        watch,
    })
}

fn valid_value(value: &str) -> bool {
    !value.is_empty() && !value.starts_with("--")
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct Snapshot {
    partial: bool,
    pending: Vec<Pending>,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct Pending {
    source_id: String,
    session_id: String,
    worker_id: Option<String>,
    turn_id: Option<String>,
    event_id: String,
    revision: u32,
    observed_at_unix_ns: String,
    reason_code: String,
    seen: bool,
}

#[derive(Serialize)]
struct Notification<'a, T> {
    schema_version: u8,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    snapshot: &'a T,
}

pub fn execute_attention(options: &AttentionOptions, mut output: impl Write) -> Result<(), String> {
    match options {
        AttentionOptions::Status { db } => emit(&mut output, "snapshot", &read_snapshot(db)?)
            .map_err(|error| format!("write attention snapshot failed: {error}")),
        AttentionOptions::Watch {
            db,
            interval,
            max_polls,
        } => watch(*interval, *max_polls, output, || read_snapshot(db)),
        #[cfg(target_os = "linux")]
        AttentionOptions::Remote {
            receiver,
            watch: polling,
        } => {
            if receiver.is_python() {
                return legacy_attention(receiver, *polling, output);
            }
            use crate::HistoryReader;
            let (store, marker) = crate::remote_history::open_workspace(receiver)?;
            let ledger = store.workspace_ledger(&marker)?;
            let read = || snapshot_from(&store, &ledger);
            if let Some((interval, limit)) = polling {
                watch(*interval, *limit, output, read)
            } else {
                emit(&mut output, "snapshot", &read()?)
                    .map_err(|error| format!("write attention snapshot failed: {error}"))
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn legacy_attention(
    receiver: &crate::RemoteHistoryOptions,
    polling: Option<(Duration, usize)>,
    mut output: impl Write,
) -> Result<(), String> {
    let mut read = crate::remote_history::legacy::attention_reader(receiver)?;
    if let Some((interval, limit)) = polling.filter(|(_, limit)| *limit > 1) {
        return watch(interval, limit, output, read);
    }
    let snapshot = read()?;
    emit(&mut output, "snapshot", &snapshot)
        .map_err(|error| format!("write attention snapshot failed: {error}"))?;
    if polling.is_some() && !snapshot.ready {
        return Err("Python receiver request inventory is incomplete".into());
    }
    Ok(())
}

fn read_snapshot(db: &Path) -> Result<Snapshot, String> {
    let ledger = WorkspaceLedger::open(db)
        .map_err(|error| format!("open workflow state failed: {error}"))?;
    let store =
        Store::open_read_only(db).map_err(|error| format!("open history failed: {error}"))?;
    snapshot_from(&store, &ledger)
}

fn snapshot_from(
    store: &impl ObservationStore,
    ledger: &WorkspaceLedger,
) -> Result<Snapshot, String> {
    let (requests, partial) = pending_requests_with_seen(store, ledger)?;
    let pending = requests
        .into_iter()
        .map(|row| Pending {
            source_id: row.owner.source,
            session_id: row.owner.session,
            worker_id: row.owner.worker,
            turn_id: row.owner.turn,
            event_id: row.event_id,
            revision: row.revision,
            observed_at_unix_ns: row.timestamp.to_string(),
            reason_code: row.reason,
            seen: row.seen,
        })
        .collect();
    Ok(Snapshot { partial, pending })
}

fn emit(writer: &mut impl Write, kind: &'static str, snapshot: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(
        &mut *writer,
        &Notification {
            schema_version: 1,
            kind,
            snapshot,
        },
    )?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn watch<T: PartialEq + Serialize>(
    interval: Duration,
    max_polls: usize,
    mut output: impl Write,
    mut read_snapshot: impl FnMut() -> Result<T, String>,
) -> Result<(), String> {
    if !(1..=MAX_POLLS).contains(&max_polls)
        || !(Duration::from_millis(100)..=Duration::from_secs(60)).contains(&interval)
    {
        return Err(USAGE.into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = WatchSignalRegistrations::new(Arc::clone(&stop))?;
    let mut previous = None;
    for poll in 0..max_polls {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let snapshot = read_snapshot()?;
        if previous.as_ref() != Some(&snapshot) {
            let kind = if previous.is_none() {
                "snapshot"
            } else {
                "change"
            };
            match emit(&mut output, kind, &snapshot) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => break,
                Err(error) => return Err(format!("write attention notification failed: {error}")),
            }
            previous = Some(snapshot);
        }
        if poll + 1 < max_polls {
            let deadline = Instant::now() + interval;
            while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                thread::sleep(
                    Duration::from_millis(25)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
    }
    Ok(())
}
