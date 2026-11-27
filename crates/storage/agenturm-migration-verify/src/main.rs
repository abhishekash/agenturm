fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: agenturm-migration-verify MANIFEST.json");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: agenturm-migration-verify MANIFEST.json");
        std::process::exit(2);
    }
    match agenturm_migration_verify::verify_file(std::path::Path::new(&path)) {
        Ok(receiver) => println!(
            "{}",
            serde_json::json!({"format":"migration_candidate_verified_v1","receiver_id":receiver})
        ),
        Err(error) => {
            eprintln!("migration candidate rejected: {error}");
            std::process::exit(1);
        }
    }
}
