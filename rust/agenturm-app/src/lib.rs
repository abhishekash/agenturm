//! Rust application composition boundary.
//!
//! This is intentionally an early shell: the product UI and production
//! harness/storage composition are not wired yet. Bundled and independently
//! installed extensions will be registered here through the kernel ports.

use agenturm_kernel::{Kernel, Snapshot};
use agenturm_runtime::{ProcessConfig, ProcessSource, ingest_once};
use agenturm_store::{CommitOutcome, Store};
use std::path::PathBuf;

pub struct AgenturmApp {
    kernel: Kernel,
}

impl AgenturmApp {
    pub fn new(kernel: Kernel) -> Self {
        Self { kernel }
    }

    pub fn empty_composition() -> Self {
        Self::new(Kernel::new())
    }

    pub fn refresh(&mut self) -> Snapshot {
        self.kernel.refresh()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Help,
    Version,
    Status,
    Ingest(IngestOptions),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngestOptions {
    pub plugin_host: PathBuf,
    pub adapter: PathBuf,
    pub source_root: PathBuf,
    pub db: PathBuf,
    pub source_id: String,
}

pub fn parse_command(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None => Ok(Command::Help),
        Some("--help" | "-h" | "help") if args.len() == 1 => Ok(Command::Help),
        Some("--version" | "-V" | "version") if args.len() == 1 => Ok(Command::Version),
        Some("status") if args.len() == 1 => Ok(Command::Status),
        Some("ingest-codex-usage") => parse_ingest_options(&args[1..]).map(Command::Ingest),
        _ => Err(usage()),
    }
}

fn parse_ingest_options(args: &[String]) -> Result<IngestOptions, String> {
    let mut plugin_host = None;
    let mut adapter = None;
    let mut source_root = None;
    let mut db = None;
    let mut source_id = None;
    let mut index = 0;
    while index < args.len() {
        let key = &args[index];
        let value = args.get(index + 1).ok_or_else(usage)?;
        if value.starts_with("--") {
            return Err(usage());
        }
        let slot = match key.as_str() {
            "--plugin-host" => &mut plugin_host,
            "--adapter" => &mut adapter,
            "--source-root" => &mut source_root,
            "--db" => &mut db,
            "--source-id" => &mut source_id,
            _ => return Err(usage()),
        };
        if slot.replace(value.clone()).is_some() {
            return Err(format!("duplicate option {key}\n{}", usage()));
        }
        index += 2;
    }
    Ok(IngestOptions {
        plugin_host: PathBuf::from(plugin_host.ok_or_else(usage)?),
        adapter: PathBuf::from(adapter.ok_or_else(usage)?),
        source_root: PathBuf::from(source_root.ok_or_else(usage)?),
        db: PathBuf::from(db.ok_or_else(usage)?),
        source_id: source_id.ok_or_else(usage)?,
    })
}

fn usage() -> String {
    "usage: agenturm [--help|--version|status] | agenturm ingest-codex-usage --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>".into()
}

pub fn execute_ingest(options: &IngestOptions) -> Result<String, String> {
    let config = ProcessConfig::codex(
        options.plugin_host.clone(),
        options.adapter.clone(),
        options.source_root.clone(),
        options.source_id.clone(),
    );
    let mut source = ProcessSource::spawn(config).map_err(|error| error.to_string())?;
    let mut store =
        Store::open(&options.db).map_err(|error| format!("open store failed: {error}"))?;
    let result = ingest_once(&options.source_id, &mut source, &mut store)
        .map_err(|error| error.to_string())?;
    let summary = match result.commit {
        CommitOutcome::Committed { inserted } => {
            format!("Committed {inserted} usage observation(s)")
        }
        CommitOutcome::Replay => "Batch was already committed".into(),
    };
    Ok(format!(
        "{summary}; sent source ACK. Skipped {} non-usage fact(s) by design.\n",
        result.skipped_non_usage_facts
    ))
}

pub fn render(command: Command) -> String {
    match command {
        Command::Help => concat!(
            "Agenturm · Rust application (early composition shell)\n",
            "\n",
            "Usage: agenturm [--help|--version|status]\n",
            "       agenturm ingest-codex-usage --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>\n",
            "\n",
            "This explicit projection stores token counters and skips all other valid Codex facts.\n",
            "The full-screen interface and persistent application configuration are not yet available.\n",
        )
        .into(),
        Command::Version => format!("agenturm {}\n", env!("CARGO_PKG_VERSION")),
        Command::Status => concat!(
            "Agenturm Rust kernel: ready\n",
            "Registered harnesses: 0\n",
            "Registered capabilities: 0\n",
            "Registered UI contributions: 0\n",
            "Production storage: not connected (no persistent app configuration)\n",
            "Explicit one-shot Codex usage projection: available\n",
        )
        .into(),
        Command::Ingest(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_application_refresh_is_explicitly_empty() {
        let mut app = AgenturmApp::empty_composition();
        let snapshot = app.refresh();
        assert!(snapshot.observations.is_empty());
        assert!(snapshot.metrics.is_empty());
        assert!(snapshot.panes.is_empty());
        assert!(snapshot.failures.is_empty());
    }

    #[test]
    fn command_parser_has_stable_help_version_and_status_commands() {
        assert_eq!(parse_command(&[]).unwrap(), Command::Help);
        assert_eq!(
            parse_command(&["--version".into()]).unwrap(),
            Command::Version
        );
        assert_eq!(parse_command(&["status".into()]).unwrap(), Command::Status);
        assert!(parse_command(&["unknown".into()]).is_err());
    }

    #[test]
    fn ingest_requires_every_explicit_source_and_storage_path() {
        let args = [
            "ingest-codex-usage",
            "--plugin-host",
            "/bin/agenturm-plugin-host",
            "--adapter",
            "/bin/agenturm-codex-harness",
            "--source-root",
            "/tmp/codex.jsonl",
            "--db",
            "/tmp/agenturm.sqlite",
            "--source-id",
            "harness.codex",
        ]
        .map(str::to_owned);
        assert!(matches!(parse_command(&args), Ok(Command::Ingest(_))));
        assert!(parse_command(&args[..args.len() - 2]).is_err());
        let mut duplicate = args.to_vec();
        duplicate.extend(["--db".into(), "/tmp/other.sqlite".into()]);
        assert!(parse_command(&duplicate).is_err());
    }

    #[test]
    fn status_output_does_not_claim_unwired_components() {
        let status = render(Command::Status);
        assert!(status.contains("Registered harnesses: 0"));
        assert!(status.contains("Production storage: not connected"));
        assert!(status.contains("Explicit one-shot Codex usage projection: available"));
    }
}
