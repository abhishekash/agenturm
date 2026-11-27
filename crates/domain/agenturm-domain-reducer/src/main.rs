//! Read one bounded fixture from stdin and write its pure reduction to stdout.

#![forbid(unsafe_code)]

use agenturm_domain_reducer::{MAX_FIXTURE_BYTES, reduce_json};
use std::io::{self, Read};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    io::stdin()
        .take((MAX_FIXTURE_BYTES + 1) as u64)
        .read_to_end(&mut input)?;
    let input = std::str::from_utf8(&input)?;
    let output = reduce_json(input)?;
    serde_json::to_writer(io::stdout().lock(), &output)?;
    Ok(())
}

fn main() {
    if run().is_err() {
        eprintln!("invalid or unreadable domain fixture");
        std::process::exit(2);
    }
}
