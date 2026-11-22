//! Opt-in, metadata-only OpenCode current SQLite schema reader.

#![forbid(unsafe_code)]

use agenturm_source_process::{Poller, Result};
use serde_json::Value;
use std::path::Path;

struct OpenCode;

impl Poller for OpenCode {
    const PLUGIN_ID: &'static str = "example.rust-opencode-harness";

    fn poll(
        path: &Path,
        source_id: &str,
        cursor: &str,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<Value> {
        agenturm_opencode_harness::poll(path, source_id, cursor, max_records, max_bytes)
    }
}

fn main() {
    if agenturm_source_process::run::<OpenCode>().is_err() {
        std::process::exit(2);
    }
}
