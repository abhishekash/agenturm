//! Explicit local and broker views of producer-captured telemetry text.

use agenturm_kernel::{ExtensionId, ObservationQuery, ObservationStore};
use agenturm_store::{Store, TelemetryBinding, TelemetryPayload, TelemetryPayloadSelection};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const USAGE: &str = concat!(
    "agenturm trace-payload <grant|revoke> --db <path> --source-id <otlp.id> --payload-scope <standalone|harness-id>\n",
    "agenturm trace-payload binding --source-id <otlp.id> --payload-scope <standalone|harness-id>\n",
    "agenturm trace-payload snapshot --db <path> --source-id <otlp.id> --session-id <id>\n",
    "agenturm trace-payload preview --db <path> --selection <json-file>",
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadCommand {
    Binding(TelemetryBinding),
    Grant {
        db: PathBuf,
        binding: TelemetryBinding,
        revoke: bool,
    },
    Read {
        db: PathBuf,
        read: PayloadRead,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadRead {
    Snapshot {
        source: ExtensionId,
        session: String,
    },
    Preview(PathBuf),
}

pub(super) fn parse(args: &[String]) -> Result<PayloadCommand, String> {
    let (verb, mut flags) = arguments(args)?;
    let command = match verb {
        "grant" | "revoke" | "binding" => {
            let source =
                super::trace_import::parse_source_id(required(&mut flags, "--source-id")?)?;
            let scope = required(&mut flags, "--payload-scope")?;
            let binding = super::trace_receiver::parse_payload_scope(&source, scope)?;
            if verb == "binding" {
                PayloadCommand::Binding(binding)
            } else {
                PayloadCommand::Grant {
                    db: required(&mut flags, "--db")?.into(),
                    binding,
                    revoke: verb == "revoke",
                }
            }
        }
        _ => PayloadCommand::Read {
            db: required(&mut flags, "--db")?.into(),
            read: parse_read(verb, &mut flags)?,
        },
    };
    if !flags.is_empty() {
        return Err(USAGE.into());
    }
    Ok(command)
}

#[cfg(target_os = "linux")]
pub(super) fn parse_broker(args: &[String]) -> Result<PayloadRead, String> {
    let (verb, mut flags) = arguments(args)?;
    let read = parse_read(verb, &mut flags);
    if !flags.is_empty() {
        return Err(USAGE.into());
    }
    read
}

fn arguments(args: &[String]) -> Result<(&str, BTreeMap<&str, &str>), String> {
    let (verb, rest) = args.split_first().ok_or(USAGE)?;
    let (pairs, remainder) = rest.as_chunks::<2>();
    let mut flags = BTreeMap::new();
    for [flag, value] in pairs {
        if value.is_empty()
            || value.starts_with("--")
            || flags.insert(flag.as_str(), value.as_str()).is_some()
        {
            return Err(USAGE.into());
        }
    }
    if !remainder.is_empty() {
        return Err(USAGE.into());
    }
    Ok((verb, flags))
}

fn required<'a>(flags: &mut BTreeMap<&str, &'a str>, flag: &str) -> Result<&'a str, String> {
    flags.remove(flag).ok_or_else(|| USAGE.into())
}

fn parse_read(verb: &str, flags: &mut BTreeMap<&str, &str>) -> Result<PayloadRead, String> {
    match verb {
        "preview" => Ok(PayloadRead::Preview(required(flags, "--selection")?.into())),
        "snapshot" => Ok(PayloadRead::Snapshot {
            source: super::trace_import::parse_source_id(required(flags, "--source-id")?)?,
            session: required(flags, "--session-id")?.into(),
        }),
        _ => Err(USAGE.into()),
    }
}

pub fn execute_payload_stdout(options: &PayloadCommand) -> Result<(), String> {
    match options {
        PayloadCommand::Binding(binding) => {
            let scope = binding.grant_scope().map_err(|error| error.to_string())?;
            println!(
                "{}",
                json!({
                    "source_id": scope.source_id(), "kind": scope.kind(),
                    "source_fingerprint": scope.source_fingerprint(),
                })
            );
        }
        PayloadCommand::Grant {
            db,
            binding,
            revoke,
        } => {
            let scope = binding.grant_scope().map_err(|error| error.to_string())?;
            let mut store = open_local(db)?;
            if *revoke {
                let changed = store
                    .revoke_bound_content(&scope)
                    .map_err(|error| error.to_string())?;
                println!("{}", json!({ "revoked": changed }));
            } else {
                let grant = store
                    .grant_content(&scope)
                    .map_err(|error| error.to_string())?;
                println!("{}", json!({ "epoch": grant.epoch() }));
            }
        }
        PayloadCommand::Read {
            db,
            read: PayloadRead::Snapshot { source, session },
        } => {
            let store = Store::open_read_only(db).map_err(|error| error.to_string())?;
            let identity = store.history_identity().map_err(|_| {
                "Telemetry history identity is unavailable. Upgrade the history with the Rust writer, then refresh the selection."
            })?;
            println!("{}", snapshot(&store, source, session, &identity)?);
        }
        PayloadCommand::Read {
            db,
            read: PayloadRead::Preview(path),
        } => {
            let selection = load_selection(path)?;
            let store = open_local(db)?;
            super::with_private_stdout(|output| {
                store
                    .with_telemetry_payload(&selection, |payload| display(payload, output))
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "Exact retained telemetry payload is unavailable".to_owned())?
            })?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn execute_broker(
    options: &PayloadRead,
    unbound: agenturm_storage_service::NativeStore,
) -> Result<(), String> {
    let identity = unbound
        .history_identity()
        .map_err(|error| error.to_string())?;
    let store = unbound
        .with_workspace_identity(&identity)
        .map_err(|error| error.to_string())?;
    match options {
        PayloadRead::Snapshot { source, session } => {
            println!("{}", snapshot(&store, source, session, &identity)?);
            Ok(())
        }
        PayloadRead::Preview(path) => {
            let selection = load_selection(path)?;
            super::with_private_stdout(|output| {
                store
                    .with_telemetry_payload(&selection, |payload| display(payload, output))
                    .map_err(|error| error.to_string())
            })
        }
    }
}

fn snapshot(
    store: &impl ObservationStore,
    source: &ExtensionId,
    session: &str,
    history_identity: &str,
) -> Result<String, String> {
    let page = store.query_observations(&ObservationQuery {
        harness_id: Some(source.as_str().to_owned()),
        session_id: Some(session.to_owned()),
        ..ObservationQuery::latest(1)
    })?;
    if page.observations.is_empty() {
        return Err("The selected telemetry session has no retained span metadata".into());
    }
    Ok(json!({
        "history_identity": history_identity,
        "snapshot_id": page.snapshot_id.to_string(),
    })
    .to_string())
}

fn open_local(db: &Path) -> Result<Store, String> {
    if !db.is_file() {
        return Err("The telemetry history is unavailable".into());
    }
    Store::open_collector(db).map_err(|error| error.to_string())
}

fn load_selection(path: &Path) -> Result<TelemetryPayloadSelection, String> {
    let file = super::trace_import::open_regular_export(path, "telemetry payload selection")?;
    let mut input = Vec::new();
    file.take(4097)
        .read_to_end(&mut input)
        .map_err(|_| "Cannot read telemetry payload selection")?;
    if input.len() > 4096 {
        return Err("Telemetry payload selection exceeds 4 KiB".into());
    }
    serde_json::from_slice(&input).map_err(|_| {
        "Invalid telemetry payload selection. Refresh it with trace-payload snapshot and include the returned history_identity and snapshot_id.".into()
    })
}

pub(super) fn modal_selection(
    span: agenturm_ui::TelemetrySpanEntry,
) -> Result<TelemetryPayloadSelection, String> {
    let native_source_id = span
        .native_source_id
        .as_deref()
        .map(ExtensionId::parse)
        .transpose()
        .map_err(|_| "Invalid native telemetry source")?;
    Ok(TelemetryPayloadSelection {
        history_identity: span.history_identity,
        binding: TelemetryBinding {
            source_id: span.source_id,
            native_source_id,
        },
        trace_id: span.trace_id,
        span_id: span.span_id,
        session_id: span.session_id,
        turn_id: span.turn_id,
        worker_id: span.worker_id,
        snapshot_id: span.snapshot_id,
    })
}

pub(super) fn deliver_modal(
    payload: &TelemetryPayload,
    delivery: &mut agenturm_ui::PromptDelivery,
) -> Result<(), String> {
    let mut text = String::new();
    if let Some(input) = &payload.input {
        text.push_str("Input\n");
        text.push_str(input);
    }
    if let Some(output) = &payload.output {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("Output\n");
        text.push_str(output);
    }
    serde_json::to_writer(&mut *delivery, &text)
        .map_err(|_| "Captured span text display closed")?;
    delivery
        .flush()
        .map_err(|_| "Captured span text display closed".into())
}

fn display(payload: &TelemetryPayload, output: &mut dyn Write) -> Result<(), String> {
    serde_json::to_writer(
        &mut *output,
        &json!({
            "origin": "producer_captured_telemetry",
            "payload": payload,
        }),
    )
    .map_err(|_| "Cannot deliver telemetry payload")?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|_| "Cannot deliver telemetry payload".into())
}
