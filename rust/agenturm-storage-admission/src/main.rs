//! Explicit scratch/clone fixture CLI; not included in the receiver launcher.

use std::env;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::Path;

fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() == 2 && args[0] == OsStr::new("--v2-scratch-path") {
        let mut input = Vec::new();
        let result = io::stdin()
            .take((agenturm_storage_service_contract::MAX_FRAME_BYTES + 1) as u64)
            .read_to_end(&mut input)
            .map_err(|error| error.into())
            .and_then(|_| agenturm_storage_admission::apply_v2_json(Path::new(&args[1]), &input));
        match result {
            Ok(output) => {
                io::stdout().write_all(&output).expect("write v2 reply");
                println!();
            }
            Err(error) => {
                eprintln!("scratch admission rejected: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() == 2 && args[0] == OsStr::new("--v2-activation-clone-path") {
        let mut input = Vec::new();
        let result = io::stdin()
            .take((agenturm_storage_service_contract::MAX_FRAME_BYTES + 1) as u64)
            .read_to_end(&mut input)
            .map_err(|error| error.into())
            .and_then(|_| {
                agenturm_storage_admission::apply_activation_clone_v2_json(
                    Path::new(&args[1]),
                    &input,
                )
            });
        match result {
            Ok(output) => {
                io::stdout().write_all(&output).expect("write v2 reply");
                println!();
            }
            Err(error) => {
                eprintln!("activation-clone admission rejected: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() == 2 && args[0] == OsStr::new("--v2-mixed-clone-batch") {
        let mut input = Vec::new();
        let result = io::stdin()
            .take((agenturm_storage_service_contract::MAX_FRAME_BYTES + 1) as u64)
            .read_to_end(&mut input)
            .map_err(|error| error.into())
            .and_then(|_| {
                agenturm_storage_admission::apply_activation_clone_mixed_batch_v2_json(
                    Path::new(&args[1]),
                    &input,
                )
            });
        match result {
            Ok(output) => {
                io::stdout().write_all(&output).expect("write v2 reply");
                println!();
            }
            Err(error) => {
                eprintln!("activation-clone admission rejected: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() == 2 && args[0] == OsStr::new("--v2-clone-policy-path") {
        let mut input = Vec::new();
        let result = io::stdin()
            .take((agenturm_storage_service_contract::MAX_FRAME_BYTES + 1) as u64)
            .read_to_end(&mut input)
            .map_err(|error| error.into())
            .and_then(|_| {
                agenturm_storage_admission::apply_activation_clone_policy_v2_json(
                    Path::new(&args[1]),
                    &input,
                )
            });
        match result {
            Ok(output) => {
                io::stdout().write_all(&output).expect("write v2 reply");
                println!();
            }
            Err(error) => {
                eprintln!("activation-clone policy rejected: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() == 2 && args[0] == OsStr::new("--v2-clone-diagnostic-path") {
        let mut input = Vec::new();
        let result = io::stdin()
            .take((agenturm_storage_service_contract::MAX_FRAME_BYTES + 1) as u64)
            .read_to_end(&mut input)
            .map_err(|error| error.into())
            .and_then(|_| {
                agenturm_storage_admission::read_activation_clone_diagnostic_v2_json(
                    Path::new(&args[1]),
                    &input,
                )
            });
        match result {
            Ok(output) => {
                io::stdout().write_all(&output).expect("write v2 reply");
                println!();
            }
            Err(error) => {
                eprintln!("activation-clone diagnostic read rejected: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if !args.is_empty() {
        eprintln!("admission rejected: use an explicit v2 scratch or clone fixture option");
        std::process::exit(1);
    }
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
