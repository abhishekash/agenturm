//! Diagnose one exact source query from an isolated Store backup.

#![forbid(unsafe_code)]

use agenturm_backup_query::{MAX_QUERY_BYTES, query_json};
use std::io::{self, Read};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    io::stdin()
        .take((MAX_QUERY_BYTES + 1) as u64)
        .read_to_end(&mut input)?;
    let reply = query_json(std::str::from_utf8(&input)?)?;
    serde_json::to_writer(io::stdout().lock(), &reply)?;
    Ok(())
}

fn main() {
    if run().is_err() {
        eprintln!("invalid or unreadable backup query");
        std::process::exit(2);
    }
}
