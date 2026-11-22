//! Explicit native broker commands. The broker owns history paths.

use super::*;
use agenturm_storage_service::NativeStore;
use serde::Deserialize;
use std::path::{Path, PathBuf};

const USAGE: &str =
    "agenturm broker <collect|query|review|inspect> --broker-config <json> [command options]";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokerOptions {
    config: PathBuf,
    command: Box<Command>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerConfig {
    version: u16,
    socket: PathBuf,
    receiver_id: String,
    service_uid: u32,
    service_gid: u32,
}

pub(super) fn parse(args: &[String]) -> Result<BrokerOptions, String> {
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h" | "help") {
        return Ok(BrokerOptions {
            config: PathBuf::new(),
            command: Box::new(Command::Help),
        });
    }
    let mut command = Vec::new();
    let mut config = None;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--broker-config" {
            let value = args
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or(USAGE)?;
            if config.replace(PathBuf::from(value)).is_some() {
                return Err(USAGE.into());
            }
            index += 2;
        } else {
            command.push(argument.clone());
            index += 1;
        }
    }
    // Existing parsers require a local path, which is irrelevant for broker
    // operations. Never accept a caller-supplied history database path.
    if command.iter().any(|argument| argument == "--db") {
        return Err("Broker commands use the service-owned history and accept no --db".into());
    }
    if !matches!(
        command.first().map(String::as_str),
        Some("collect" | "query" | "review" | "inspect")
    ) {
        return Err(USAGE.into());
    }
    command.extend(["--db".into(), "broker-history".into()]);
    let command = parse_command(&command)?;
    Ok(BrokerOptions {
        config: config.ok_or(USAGE)?,
        command: Box::new(command),
    })
}

fn connect(path: &Path) -> Result<NativeStore, String> {
    let file = File::open(path).map_err(|error| format!("Open broker configuration: {error}"))?;
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("Broker configuration exceeds 64 KiB".into());
    }
    let config: BrokerConfig =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid broker configuration")?;
    if config.version != 1 || !config.socket.is_absolute() {
        return Err("Broker configuration requires version 1 and an absolute socket path".into());
    }
    NativeStore::new(
        config.socket,
        config.receiver_id,
        config.service_uid,
        config.service_gid,
    )
    .map_err(|error| error.to_string())
}

pub fn execute(options: &BrokerOptions) -> Result<(), String> {
    if matches!(options.command.as_ref(), Command::Help) {
        println!(
            "{USAGE}\nThe broker configuration pins its socket, receiver and service UID/GID.\nUse collect --config <sources.json> [--watch], query paging options, or inspect."
        );
        return Ok(());
    }
    let store = connect(&options.config)?;
    match options.command.as_ref() {
        Command::Collect(options) => {
            let config = load_collection_config(&options.config)?;
            let stop = Arc::new(AtomicBool::new(false));
            let signals = options
                .watch_interval
                .map(|_| WatchSignalRegistrations::new(Arc::clone(&stop)))
                .transpose()?;
            let mut collector = ConfiguredCollector::with_store(
                config,
                options.db.clone(),
                store,
                options.watch_interval.map(|_| &stop),
            )?;
            collector.validate_local_database = false;
            let had_failures = if let Some(interval) = options.watch_interval {
                watch_collector(collector, interval, &stop)?.had_failures
            } else {
                let report = collector.refresh();
                println!("{}", report.summary.trim_end());
                report.has_failures()
            };
            drop(signals);
            if had_failures {
                return Err("Broker collection reported source or storage failures".into());
            }
        }
        Command::Query(query) => println!("{}", query_from_store(&store, query, None)?),
        Command::Review {
            query,
            turn_id,
            worker_id,
        } => println!(
            "{}",
            query_from_store(&store, query, Some((turn_id, worker_id.as_deref())))?
        ),
        Command::Inspect(options) => {
            let mut app = AgenturmApp::default_composition();
            install_process_extensions(&mut app, &options.extensions)?;
            agenturm_ui::run_workspace(move |request| {
                match request {
                    WorkspaceRequest::Sessions(before) => {
                        let mut query = SessionQuery::latest(200);
                        query.before = before.map(|cursor| SessionPosition {
                            observed_at_unix_ns: cursor.updated_at_unix_ns,
                            harness_id: cursor.harness_id,
                            session_id: cursor.session_id,
                            snapshot_id: cursor.snapshot_id,
                        });
                        query.snapshot_id = query.before.as_ref().map(|cursor| cursor.snapshot_id);
                        let page = store.query_sessions(&query).map_err(|error| error.to_string())?;
                        let sessions = page.sessions.into_iter().map(|session| {
                            let (state, reason, turn_id) = session.lifecycle.map_or(
                                (None, None, None),
                                |life| (Some(life.state), life.reason_code, life.turn_id),
                            );
                            RecordedSession {
                                harness_id: session.harness_id.as_str().into(),
                                session_id: session.session_id,
                                updated_at_unix_ns: session.observed_at_unix_ns,
                                snapshot_id: page.snapshot_id,
                                state, reason, turn_id,
                                parent_session_id: session.parent_session_id,
                                relationship_kind: session.relationship_kind,
                                review_version: ReviewVersion {
                                    receipt_sequence: session.review_version.receipt_sequence,
                                    retained_observations: session.review_version.retained_observations,
                                },
                                reviewed: false,
                            }
                        }).collect();
                        Ok(WorkspaceResponse::Sessions(SessionCatalog {
                            sessions,
                            older: page.next_before.map(|cursor| SessionCursor {
                                harness_id: cursor.harness_id,
                                session_id: cursor.session_id,
                                updated_at_unix_ns: cursor.observed_at_unix_ns,
                                snapshot_id: cursor.snapshot_id,
                            }),
                            notice: Some("Broker history. Local tasks and private-content actions require the local cockpit.".into()),
                        }))
                    }
                    WorkspaceRequest::History { selection, before } => {
                        let mut page = load_workspace_history(&mut app, &store, selection, before)?;
                        page.operation_details.clear();
                        page.review_pane = None;
                        Ok(WorkspaceResponse::History(page))
                    }
                    WorkspaceRequest::Work(_) | WorkspaceRequest::MarkReviewed(_) => {
                        Err("This broker inspector supports recorded history and turn navigation".into())
                    }
                }
            }).map_err(|error| error.to_string())?;
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &str) -> Result<BrokerOptions, String> {
        parse(
            &words
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn broker_commands_bind_the_endpoint_and_keep_history_paths_service_owned() {
        for command in [
            "query",
            "query --limit 1 --harness-id harness.codex",
            "review --harness-id harness.codex --session-id session --turn-id turn",
            "inspect",
            "collect --config sources.json",
            "collect --config sources.json --watch",
        ] {
            assert!(parse_words(&format!("{command} --broker-config broker.json")).is_ok());
        }
        for command in [
            "query",
            "query --broker-config",
            "query --broker-config one --broker-config two",
            "query --broker-config one --db private.sqlite",
            "backup --broker-config one",
            "review --broker-config one",
        ] {
            assert!(parse_words(command).is_err());
        }
    }
}
