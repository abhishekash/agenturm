//! Read-only attention snapshots and bounded change notifications.

mod desktop;

use crate::{WatchSignalRegistrations, cockpit_workflows::pending_requests_with_seen};
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
    "[--interval-ms <100..60000>] [--desktop]\n",
    "       agenturm attention native <bind|send|hook|retry> (explicit receiver binding)\n",
    "       agenturm attention <status|watch> --endpoint http://<loopback-IP:port> ",
    "--token-file <path> [--receiver-protocol python-v0.2.8] [watch options]\n",
    "Desktop delivery over HTTP requires explicit --receiver-protocol python-v0.2.8.\n",
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttentionOptions {
    #[cfg(target_os = "linux")]
    Native(Box<crate::attention_native::NativeOptions>),
    Status {
        db: PathBuf,
    },
    Watch {
        db: PathBuf,
        interval: Duration,
        max_polls: usize,
        desktop: bool,
    },
    #[cfg(target_os = "linux")]
    Remote {
        receiver: crate::RemoteHistoryOptions,
        watch: Option<(Duration, usize)>,
        desktop: bool,
    },
}

pub(super) fn parse(args: &[String]) -> Result<AttentionOptions, String> {
    #[cfg(target_os = "linux")]
    if let Some((verb, values)) = args.split_first()
        && verb == "native"
    {
        return crate::attention_native::parse(values)
            .map(|options| AttentionOptions::Native(Box::new(options)));
    }
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
            let (db, interval, max_polls, desktop) = parse_watch(values, true)?;
            Ok(AttentionOptions::Watch {
                db: db.ok_or(USAGE)?,
                interval,
                max_polls,
                desktop,
            })
        }
        _ => Err(USAGE.into()),
    }
}

fn parse_watch(
    mut args: &[String],
    local: bool,
) -> Result<(Option<PathBuf>, Duration, usize, bool), String> {
    let (mut db, mut interval, mut max_polls) = (None, None, None);
    let mut desktop = false;
    while let Some((flag, rest)) = args.split_first() {
        args = rest;
        if flag == "--once" && max_polls.is_none() {
            max_polls = Some(1);
            continue;
        }
        if flag == "--desktop" && !desktop {
            desktop = true;
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
        desktop,
    ))
}

#[cfg(target_os = "linux")]
fn parse_remote(args: &[String]) -> Result<AttentionOptions, String> {
    let (verb, mut args) = args.split_first().ok_or(USAGE)?;
    let mut receiver = Vec::new();
    let mut polling = Vec::new();
    while let Some((flag, rest)) = args.split_first() {
        if matches!(flag.as_str(), "--once" | "--desktop") {
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
    let (watch, desktop) = match verb.as_str() {
        "status" if polling.is_empty() => (None, false),
        "watch" => {
            let (_, interval, limit, desktop) = parse_watch(&polling, false)?;
            (Some((interval, limit)), desktop)
        }
        _ => return Err(USAGE.into()),
    };
    let receiver = crate::remote_history::parse_attach(&receiver)?;
    if desktop && !receiver.is_python() {
        return Err("Desktop attention over HTTP requires an explicit Python v0.2.8 receiver with a comparable clock".into());
    }
    Ok(AttentionOptions::Remote {
        receiver,
        watch,
        desktop,
    })
}

fn valid_value(value: &str) -> bool {
    !value.is_empty() && !value.starts_with("--")
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct Snapshot {
    partial: bool,
    pending: Vec<Pending>,
    #[serde(skip_serializing_if = "Option::is_none")]
    desktop_delivery: Option<desktop::Delivery>,
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
        #[cfg(target_os = "linux")]
        AttentionOptions::Native(options) => crate::attention_native::execute(options),
        AttentionOptions::Status { db } => emit(&mut output, "snapshot", &read_snapshot(db)?)
            .map_err(|error| format!("write attention snapshot failed: {error}")),
        AttentionOptions::Watch {
            db,
            interval,
            max_polls,
            desktop,
        } => {
            if *desktop {
                let ledger = WorkspaceLedger::open(db)
                    .map_err(|error| format!("open workflow state failed: {error}"))?;
                let store = open_history(db)?;
                watch(*interval, *max_polls, output, |stop| {
                    desktop::snapshot(&store, &ledger, stop, || {
                        let current = open_history(db)?;
                        desktop::current_position(&current)
                    })
                })
            } else {
                watch(*interval, *max_polls, output, |_| read_snapshot(db))
            }
        }
        #[cfg(target_os = "linux")]
        AttentionOptions::Remote {
            receiver,
            watch: polling,
            desktop: send_desktop,
        } => {
            if receiver.is_python() {
                if *send_desktop && let Some((interval, limit)) = polling {
                    return desktop::watch_legacy(receiver, *interval, *limit, output);
                }
                return legacy_attention(receiver, *polling, output);
            }
            use crate::HistoryReader;
            let (store, marker) = crate::remote_history::open_workspace(receiver)?;
            let ledger = store.workspace_ledger(&marker)?;
            let read = || snapshot_from(&store, &ledger);
            if let Some((interval, limit)) = polling {
                watch(*interval, *limit, output, |_| read())
            } else {
                emit(&mut output, "snapshot", &read()?)
                    .map_err(|error| format!("write attention snapshot failed: {error}"))
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn execute_broker(
    options: &AttentionOptions,
    store: &agenturm_storage_service::NativeStore,
    workspace: &Path,
    mut output: impl Write,
) -> Result<(), String> {
    use crate::HistoryReader;
    let ledger = store.workspace_ledger(workspace)?;
    match options {
        AttentionOptions::Native(_) => Err("Native delivery uses explicit receiver bindings. Use attention native without the broker prefix.".into()),
        AttentionOptions::Status { .. } => {
            emit(&mut output, "snapshot", &snapshot_from(store, &ledger)?)
                .map_err(|error| format!("write attention snapshot failed: {error}"))
        }
        AttentionOptions::Watch {
            interval,
            max_polls,
            desktop,
            ..
        } => watch(*interval, *max_polls, output, |stop| {
            if *desktop {
                desktop::snapshot(store, &ledger, stop, || desktop::current_position(store))
            } else {
                snapshot_from(store, &ledger)
            }
        }),
        AttentionOptions::Remote { .. } => {
            Err("Broker attention uses its configured native history".into())
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
        return watch(interval, limit, output, |_| read());
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
    let store = open_history(db)?;
    snapshot_from(&store, &ledger)
}

fn open_history(db: &Path) -> Result<Store, String> {
    Store::open_read_only(db).map_err(|error| format!("open history failed: {error}"))
}

fn snapshot_from(
    store: &impl crate::HistoryReader,
    ledger: &WorkspaceLedger,
) -> Result<Snapshot, String> {
    let (requests, partial) = pending_requests_with_seen(store, ledger)?;
    Ok(snapshot_report(requests, partial, None))
}

fn snapshot_report(
    requests: Vec<agenturm_ui::RequestEntry>,
    partial: bool,
    desktop_delivery: Option<desktop::Delivery>,
) -> Snapshot {
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
    Snapshot {
        partial,
        pending,
        desktop_delivery,
    }
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
    mut read_snapshot: impl FnMut(&AtomicBool) -> Result<T, String>,
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
        let snapshot = read_snapshot(&stop)?;
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
