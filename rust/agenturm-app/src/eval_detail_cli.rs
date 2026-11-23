//! Explicit grant and disclosure of captured evaluation text.

use agenturm_kernel::ExtensionId;
use agenturm_store::{EvalDetailSelection, EvalResultDetail, Store, eval_detail_grant_scope};
use serde::Serialize;
use serde_json::json;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub(super) const USAGE: &str = concat!(
    "agenturm eval-detail binding --source-id <otlp.id>\n",
    "agenturm eval-detail <grant|revoke> --db <path> --source-id <otlp.id>\n",
    "agenturm eval-detail preview --db <path> --selection <json-file>",
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvalDetailOptions {
    Binding(ExtensionId),
    Grant {
        db: PathBuf,
        source: ExtensionId,
        revoke: bool,
    },
    Preview {
        db: PathBuf,
        selection: PathBuf,
    },
}

pub(super) fn parse(args: &[String]) -> Result<EvalDetailOptions, String> {
    match args {
        [verb, flag, source] if verb == "binding" && flag == "--source-id" => Ok(
            EvalDetailOptions::Binding(crate::trace_import::parse_source_id(source)?),
        ),
        [verb, flags @ ..] if matches!(verb.as_str(), "grant" | "revoke") => {
            let (db, source) =
                crate::parse_db_and_option(flags, "--source-id").map_err(|_| USAGE.to_owned())?;
            Ok(EvalDetailOptions::Grant {
                db,
                source: crate::trace_import::parse_source_id(source)?,
                revoke: verb == "revoke",
            })
        }
        [verb, flags @ ..] if verb == "preview" => {
            let (db, selection) =
                crate::parse_db_and_option(flags, "--selection").map_err(|_| USAGE.to_owned())?;
            Ok(EvalDetailOptions::Preview {
                db,
                selection: selection.into(),
            })
        }
        _ => Err(USAGE.into()),
    }
}

fn open_private_store(db: &Path) -> Result<Store, String> {
    // Private reads and approvals attach to an existing history only.
    Store::open_read_only(db).map_err(|error| error.to_string())?;
    Store::open_collector(db).map_err(|error| error.to_string())
}

pub fn execute_eval_detail_stdout(options: &EvalDetailOptions) -> Result<(), String> {
    match options {
        EvalDetailOptions::Binding(source) => {
            let scope = eval_detail_grant_scope(source).map_err(|error| error.to_string())?;
            println!(
                "{}",
                json!({ "source_id": scope.source_id(), "kind": scope.kind(),
                "source_fingerprint": scope.source_fingerprint() })
            );
        }
        EvalDetailOptions::Grant { db, source, revoke } => {
            let scope = eval_detail_grant_scope(source).map_err(|error| error.to_string())?;
            let mut store = open_private_store(db)?;
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
        EvalDetailOptions::Preview { db, selection } => {
            let selection = load_selection(selection)?;
            let store = open_private_store(db)?;
            crate::with_private_stdout(|output| {
                store
                    .with_eval_result_detail(&selection, |detail| display(detail, output))
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "Exact retained evaluation detail is unavailable".to_owned())?
            })?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn parse_broker(args: &[String]) -> Result<PathBuf, String> {
    match args {
        [verb, flag, path]
            if verb == "preview"
                && flag == "--selection"
                && !path.is_empty()
                && !path.starts_with("--") =>
        {
            Ok(path.into())
        }
        _ => Err(
            "agenturm broker eval-detail preview --broker-config <json> --selection <json-file>"
                .into(),
        ),
    }
}

#[cfg(target_os = "linux")]
pub(super) fn execute_broker(
    path: &Path,
    store: agenturm_storage_service::NativeStore,
) -> Result<(), String> {
    let selection = load_selection(path)?;
    let identity = store
        .history_identity()
        .map_err(|error| error.to_string())?;
    let store = store
        .with_workspace_identity(&identity)
        .map_err(|error| error.to_string())?;
    crate::with_private_stdout(|output| {
        store
            .with_eval_result_detail(&selection, |detail| display(detail, output))
            .map_err(|error| error.to_string())
    })
}

fn load_selection(path: &Path) -> Result<EvalDetailSelection, String> {
    const MAX_SELECTION_BYTES: u64 = 4096;
    let mut bytes = Vec::new();
    crate::trace_import::open_regular_export(path, "evaluation detail selection")?
        .take(MAX_SELECTION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read evaluation detail selection")?;
    if bytes.len() as u64 > MAX_SELECTION_BYTES {
        return Err("Evaluation detail selection exceeds 4 KiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid evaluation detail selection".into())
}

fn display(detail: &EvalResultDetail, output: &mut dyn Write) -> Result<(), String> {
    #[derive(Serialize)]
    struct Preview<'a> {
        provenance: &'static str,
        detail: &'a EvalResultDetail,
    }
    serde_json::to_writer(
        &mut *output,
        &Preview {
            provenance: "imported evaluation text",
            detail,
        },
    )
    .map_err(|_| "Cannot write evaluation detail")?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|_| "Cannot write evaluation detail".into())
}

pub(super) fn modal_selection(detail: agenturm_ui::EvaluationDetailEntry) -> EvalDetailSelection {
    EvalDetailSelection {
        trace_source_id: detail.source,
        trace_id: detail.trace_id,
        revision: detail.revision,
        receipt_id: detail.receipt_id,
        snapshot_id: detail.snapshot_id,
    }
}

pub(super) fn deliver_modal(
    detail: &EvalResultDetail,
    output: &mut dyn Write,
) -> Result<(), String> {
    use std::fmt::Write as _;

    let mut text = String::new();
    if let Some(error) = &detail.error {
        let _ = writeln!(text, "Evaluation error\n{error}");
    }
    for score in &detail.scores {
        let _ = writeln!(
            text,
            "Score {} detail\n{}",
            score.score_index + 1,
            score.detail
        );
    }
    serde_json::to_writer(&mut *output, &text).map_err(|_| "Cannot encode evaluation detail")?;
    output
        .flush()
        .map_err(|_| "Cannot deliver evaluation detail".into())
}
