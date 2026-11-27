//! Metadata-only OpenCode current SQLite schema reader.

#![forbid(unsafe_code)]

use agenturm_source_process::{Poller, Result};
use serde_json::Value;
use std::{ffi::OsStr, path::Path};

struct OpenCode<const LEGACY: bool>;

impl<const LEGACY: bool> Poller for OpenCode<LEGACY> {
    const PLUGIN_ID: &'static str = "example.rust-opencode-harness";

    fn poll(
        path: &Path,
        source_id: &str,
        cursor: &str,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<Value> {
        let mut batch = if LEGACY {
            agenturm_opencode_harness::poll_legacy(path, source_id, cursor, max_records, max_bytes)?
        } else {
            agenturm_opencode_harness::poll(path, source_id, cursor, max_records, max_bytes)?
        };
        if LEGACY {
            // The old Python admission contract uses different relationship
            // keys, one session-level lifecycle, and only the generic tool
            // fact accepts an operation ID.
            let records = batch["records"]
                .as_array_mut()
                .ok_or("missing adapter records")?;
            records.retain(|row| {
                row["kind"] != "mcp_call"
                    && (row["kind"] != "lifecycle" || row["identity"]["turn_id"].is_null())
            });
            for record in records {
                if let Some(identity) = record.get_mut("identity").and_then(Value::as_object_mut) {
                    identity.remove("turn_id");
                }
                match record["kind"].as_str() {
                    Some("check") => {
                        if let Some(identity) =
                            record.get_mut("identity").and_then(Value::as_object_mut)
                        {
                            identity.remove("operation_id");
                        }
                    }
                    Some("relationship") => {
                        if let Some(payload) = record["payload"].as_object_mut()
                            && let Some(kind) = payload.remove("relationship_kind")
                        {
                            payload.insert("kind".into(), kind);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(batch)
    }
}

fn main() {
    let legacy =
        std::env::var_os("AGENTURM_PLUGIN_LEGACY_ADMISSION").as_deref() == Some(OsStr::new("1"));
    let outcome = if legacy {
        agenturm_source_process::run::<OpenCode<true>>()
    } else {
        agenturm_source_process::run::<OpenCode<false>>()
    };
    if outcome.is_err() {
        std::process::exit(2);
    }
}
