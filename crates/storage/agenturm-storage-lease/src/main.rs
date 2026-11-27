//! Tiny test probe. This binary is not installed by the receiver launcher.

use agenturm_storage_lease::WriterLease;
use std::io::{self, Read, Write};
use std::path::Path;

fn main() {
    // The probe is an internal test tool, not a storage service entrypoint.
    if std::env::var_os("AGENTURM_STORAGE_LEASE_PROBE").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        std::process::exit(2);
    }
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(path) = args.next() else {
        std::process::exit(2);
    };
    if args.next().is_some() {
        std::process::exit(2);
    }
    let lease = match WriterLease::acquire(Path::new(&path)) {
        Ok(lease) => lease,
        Err(_) => std::process::exit(3),
    };
    if lease.verify_admission().is_err() {
        std::process::exit(3);
    }
    println!("ready");
    if io::stdout().flush().is_err() {
        std::process::exit(3);
    }
    // The parent retains a pipe to keep the lock alive. EOF releases it.
    let mut end = [0_u8; 1];
    while let Ok(1) = io::stdin().read(&mut end) {}
}
