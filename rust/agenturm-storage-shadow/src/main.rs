//! Inspect one isolated Store backup and write a diagnostic certificate.

#![forbid(unsafe_code)]

use agenturm_storage_shadow::{MAX_REQUEST_BYTES, inspect_json};
use std::io::{self, Read};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    io::stdin()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut input)?;
    let request = std::str::from_utf8(&input)?;
    let reply = inspect_json(request)?;
    serde_json::to_writer(io::stdout().lock(), &reply)?;
    Ok(())
}

fn main() {
    if run().is_err() {
        eprintln!("invalid or unreadable shadow Store");
        std::process::exit(2);
    }
}
