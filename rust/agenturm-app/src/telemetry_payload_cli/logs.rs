//! Explicit grants and exact selections for captured OTLP log text.

use super::{
    PayloadCommand, PayloadRead, arguments, open_local, print_binding, read_selection, update_grant,
};
use agenturm_kernel::{ExtensionId, ObservationPosition, ObservationQuery, ObservationStore};
use agenturm_store::{Store, TelemetryLogText, TelemetryLogTextBinding, TelemetryLogTextSelection};
use agenturm_ui::FindingSource;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

const USAGE: &str = concat!(
    "agenturm log-text <grant|revoke> --db <path> --source-id <otlp.id> --payload-scope <standalone|harness-id>\n",
    "agenturm log-text binding --source-id <otlp.id> --payload-scope <standalone|harness-id>\n",
    "agenturm log-text snapshot --db <path> --source-id <otlp.id> [--session-id <id>] [--before <json-cursor>]\n",
    "agenturm log-text preview --db <path> --selection <json-file>",
);
const PAGE_SIZE: usize = 200;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogCommand {
    Binding(TelemetryLogTextBinding),
    Grant {
        db: PathBuf,
        binding: TelemetryLogTextBinding,
        revoke: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogRead {
    Snapshot {
        source: ExtensionId,
        session: Option<String>,
        before: Option<Box<ObservationPosition>>,
    },
    Preview(PathBuf),
}

pub(crate) fn parse_log_text(args: &[String]) -> Result<PayloadCommand, String> {
    let (verb, mut flags) = arguments(args, USAGE)?;
    let command = match verb {
        "grant" | "revoke" | "binding" => {
            let source = source(&mut flags)?;
            let scope = flags.remove("--payload-scope").ok_or(USAGE)?;
            let span_binding = crate::trace_receiver::parse_payload_scope(&source, scope)?;
            let binding = TelemetryLogTextBinding::from(&span_binding);
            let grant = if verb == "binding" {
                LogCommand::Binding(binding)
            } else {
                LogCommand::Grant {
                    db: flags.remove("--db").ok_or(USAGE)?.into(),
                    binding,
                    revoke: verb == "revoke",
                }
            };
            PayloadCommand::Log(Box::new(grant))
        }
        _ => PayloadCommand::Read {
            db: flags.remove("--db").ok_or(USAGE)?.into(),
            read: PayloadRead::Log(Box::new(parse_read(verb, &mut flags)?)),
        },
    };
    if !flags.is_empty() {
        return Err(USAGE.into());
    }
    Ok(command)
}

#[cfg(target_os = "linux")]
pub(crate) fn parse_broker_log_text(args: &[String]) -> Result<PayloadRead, String> {
    let (verb, mut flags) = arguments(args, USAGE)?;
    let read = parse_read(verb, &mut flags)?;
    if !flags.is_empty() {
        return Err(USAGE.into());
    }
    Ok(PayloadRead::Log(Box::new(read)))
}

fn parse_read(verb: &str, flags: &mut BTreeMap<&str, &str>) -> Result<LogRead, String> {
    match verb {
        "preview" => Ok(LogRead::Preview(
            flags.remove("--selection").ok_or(USAGE)?.into(),
        )),
        "snapshot" => {
            let source = source(flags)?;
            let before = flags
                .remove("--before")
                .map(|cursor| {
                    let invalid = "Invalid captured log page cursor";
                    if cursor.len() > 4096 {
                        return Err(invalid);
                    }
                    let position: ObservationPosition =
                        serde_json::from_str(cursor).map_err(|_| invalid)?;
                    if position.harness_id != source.as_str() {
                        return Err(invalid);
                    }
                    Ok(Box::new(position))
                })
                .transpose()?;
            Ok(LogRead::Snapshot {
                source,
                session: flags.remove("--session-id").map(str::to_owned),
                before,
            })
        }
        _ => Err(USAGE.into()),
    }
}

fn source(flags: &mut BTreeMap<&str, &str>) -> Result<ExtensionId, String> {
    crate::trace_import::parse_source_id(flags.remove("--source-id").ok_or(USAGE)?)
}

pub(super) fn execute_local(command: &LogCommand) -> Result<(), String> {
    match command {
        LogCommand::Binding(binding) => {
            let scope = binding.grant_scope().map_err(|error| error.to_string())?;
            print_binding(&scope);
        }
        LogCommand::Grant {
            db,
            binding,
            revoke,
        } => {
            let scope = binding.grant_scope().map_err(|error| error.to_string())?;
            update_grant(db, &scope, *revoke)?;
        }
    }
    Ok(())
}

pub(super) fn execute_local_read(db: &Path, read: &LogRead) -> Result<(), String> {
    match read {
        LogRead::Snapshot {
            source,
            session,
            before,
        } => {
            let store = Store::open_read_only(db).map_err(|error| error.to_string())?;
            let identity = store
                .history_identity()
                .map_err(|error| error.to_string())?;
            print_snapshot(
                &store,
                source,
                session.as_deref(),
                before.as_deref(),
                &identity,
            )?;
        }
        LogRead::Preview(path) => {
            let selection = load_selection(path)?;
            let store = open_local(db)?;
            crate::with_private_stdout(|output| {
                store
                    .with_telemetry_log_text(&selection, |log| display(log, output))
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "Exact captured log text is unavailable".to_owned())?
            })?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn execute_broker(
    read: &LogRead,
    store: &agenturm_storage_service::NativeStore,
    identity: &str,
) -> Result<(), String> {
    match read {
        LogRead::Snapshot {
            source,
            session,
            before,
        } => print_snapshot(
            store,
            source,
            session.as_deref(),
            before.as_deref(),
            identity,
        ),
        LogRead::Preview(path) => {
            let selection = load_selection(path)?;
            crate::with_private_stdout(|output| {
                store
                    .with_telemetry_log_text(&selection, |log| display(log, output))
                    .map_err(|error| error.to_string())
            })
        }
    }
}

fn print_snapshot(
    store: &impl ObservationStore,
    source: &ExtensionId,
    session: Option<&str>,
    before: Option<&ObservationPosition>,
    history_identity: &str,
) -> Result<(), String> {
    let page = store.query_observations(&ObservationQuery {
        harness_id: Some(source.as_str().into()),
        session_id: session.map(str::to_owned),
        before: before.cloned(),
        snapshot_id: before.map(|position| position.snapshot_id),
        limit: PAGE_SIZE,
    })?;
    let selections = page
        .observations
        .iter()
        .rev()
        .filter_map(|row| {
            let FindingSource::Log(log) =
                crate::findings::log_source(row, page.snapshot_id, history_identity)?
            else {
                return None;
            };
            Some(crate::findings::log_selection(*log))
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        json!({
            "history_identity": history_identity,
            "snapshot_id": page.snapshot_id,
            "scanned_observations": page.observations.len(),
            "next_before": page.older_position(),
            "selections": selections,
        })
    );
    Ok(())
}

fn load_selection(path: &Path) -> Result<TelemetryLogTextSelection, String> {
    serde_json::from_slice(&read_selection(path)?).map_err(|_| {
        "Invalid captured log selection. Use an exact selection from log-text snapshot.".into()
    })
}

fn display(log: &TelemetryLogText, output: &mut dyn Write) -> Result<(), String> {
    serde_json::to_writer(
        &mut *output,
        &json!({
            "origin": "producer_captured_log_text",
            "text": log.text,
        }),
    )
    .map_err(|_| "Cannot write captured log text")?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|_| "Cannot write captured log text".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_log_snapshot_permits_source_scope_and_broker_rejects_local_paths() {
        let words = |input: &str| {
            input
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let PayloadCommand::Read {
            read: PayloadRead::Log(read),
            ..
        } = parse_log_text(&words("snapshot --db history --source-id otlp.fixture")).unwrap()
        else {
            panic!("expected log command");
        };
        assert!(matches!(
            *read,
            LogRead::Snapshot {
                session: None,
                before: None,
                ..
            }
        ));
        assert!(parse_log_text(&words("grant --db history --source-id otlp.fixture")).is_err());
        #[cfg(target_os = "linux")]
        {
            assert!(
                parse_broker_log_text(&words("preview --db history --selection fixture.json"))
                    .is_err()
            );
            assert!(
                parse_broker_log_text(&words(
                    "grant --source-id otlp.fixture --payload-scope standalone"
                ))
                .is_err()
            );
        }
    }
}
