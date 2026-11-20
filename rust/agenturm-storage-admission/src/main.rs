//! Explicit scratch-fixture CLI; not included in the receiver launcher.

use std::io::{self, Read};

fn main() {
    let mut input = String::new();
    if let Err(error) = io::stdin().take(1024 * 1024 + 1).read_to_string(&mut input) {
        eprintln!("scratch admission rejected: {error}");
        std::process::exit(1);
    }
    match agenturm_storage_admission::apply_json(&input) {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("scratch admission rejected: {error}");
            std::process::exit(1);
        }
    }
}
