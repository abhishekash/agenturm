//! Rust application composition boundary.
//!
//! The app exposes generic kernel extension registration, durable refresh,
//! an explicit registered-plugin usage command, and a full-screen durable-history viewer.
//! Persistent source configuration and multi-harness composition are still
//! open.

use agenturm_kernel::{
    Capability, ExtensionId, Harness, Kernel, MetricRow, Observation, ObservationKind,
    ObservationStore, PaneModel, RegistrationError, Snapshot, SourceAdapter, UiContribution,
};
use agenturm_runtime::{CODEX_PLUGIN_ID, ProcessConfig, ProcessSource, UsageBatchDecoder};
use agenturm_store::Store;
use std::collections::BTreeSet;
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

    pub fn default_composition() -> Self {
        let mut app = Self::empty_composition();
        app.register_capability(
            ExtensionId::parse("capability.overview").expect("built-in extension ID is valid"),
            OverviewCapability,
        )
        .expect("built-in capability registers once");
        app.register_ui_contribution(
            ExtensionId::parse("pane.overview").expect("built-in extension ID is valid"),
            OverviewPane,
        )
        .expect("built-in pane registers once");
        app
    }

    pub fn refresh(&mut self) -> Snapshot {
        self.kernel.refresh()
    }

    pub fn refresh_durable(&mut self, store: &mut impl ObservationStore) -> Snapshot {
        self.kernel.refresh_with_store(store)
    }

    pub fn register_harness(
        &mut self,
        id: ExtensionId,
        harness: impl Harness + 'static,
    ) -> Result<(), RegistrationError> {
        self.kernel.register_harness(id, harness)
    }

    pub fn register_source_adapter(
        &mut self,
        id: ExtensionId,
        source: impl SourceAdapter + 'static,
    ) -> Result<(), RegistrationError> {
        self.kernel.register_source_adapter(id, source)
    }

    pub fn register_capability(
        &mut self,
        id: ExtensionId,
        capability: impl Capability + 'static,
    ) -> Result<(), RegistrationError> {
        self.kernel.register_capability(id, capability)
    }

    pub fn register_ui_contribution(
        &mut self,
        id: ExtensionId,
        contribution: impl UiContribution + 'static,
    ) -> Result<(), RegistrationError> {
        self.kernel.register_ui_contribution(id, contribution)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Help,
    Version,
    Status,
    Inspect(PathBuf),
    Ingest(IngestOptions),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngestOptions {
    pub plugin_id: String,
    pub plugin_host: PathBuf,
    pub adapter: PathBuf,
    pub source_root: PathBuf,
    pub db: PathBuf,
    pub source_id: String,
}

struct OverviewCapability;

impl Capability for OverviewCapability {
    fn evaluate(&self, observations: &[Observation]) -> Result<Vec<MetricRow>, String> {
        let mut usage_events = 0usize;
        let mut reported_total_events = 0usize;
        let mut reported_total_tokens = 0u64;
        for row in observations {
            if let ObservationKind::Usage { total_tokens, .. } = &row.kind {
                usage_events += 1;
                if let Some(total) = total_tokens {
                    reported_total_events += 1;
                    reported_total_tokens = reported_total_tokens
                        .checked_add(*total)
                        .ok_or_else(|| "reported token total exceeds u64".to_owned())?;
                }
            }
        }
        Ok(vec![
            MetricRow {
                label: "Usage events with reported totals".into(),
                value: format!("{reported_total_events} / {usage_events}"),
            },
            MetricRow {
                label: "Reported total tokens (partial)".into(),
                value: reported_total_tokens.to_string(),
            },
        ])
    }
}

struct OverviewPane;

impl UiContribution for OverviewPane {
    fn build_pane(&self, snapshot: &Snapshot) -> Result<PaneModel, String> {
        Ok(PaneModel {
            title: "Overview".into(),
            rows: overview_rows(&snapshot.observations),
        })
    }
}

fn overview_rows(observations: &[Observation]) -> Vec<MetricRow> {
    let sessions = observations
        .iter()
        .map(|row| (row.harness_id.as_str(), row.identity.session_id.as_str()))
        .collect::<BTreeSet<_>>()
        .len();
    let sources = observations
        .iter()
        .map(|row| row.harness_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let activities = observations
        .iter()
        .filter(|row| matches!(row.kind, ObservationKind::Activity { .. }))
        .count();
    let usages = observations
        .iter()
        .filter(|row| matches!(row.kind, ObservationKind::Usage { .. }))
        .count();
    vec![
        MetricRow {
            label: "Sessions observed".into(),
            value: sessions.to_string(),
        },
        MetricRow {
            label: "Sources with history".into(),
            value: sources.to_string(),
        },
        MetricRow {
            label: "Activity events".into(),
            value: activities.to_string(),
        },
        MetricRow {
            label: "Usage events".into(),
            value: usages.to_string(),
        },
    ]
}

pub fn parse_command(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None => Ok(Command::Help),
        Some("--help" | "-h" | "help") if args.len() == 1 => Ok(Command::Help),
        Some("--version" | "-V" | "version") if args.len() == 1 => Ok(Command::Version),
        Some("status") if args.len() == 1 => Ok(Command::Status),
        Some("inspect") => parse_inspect_options(&args[1..]).map(Command::Inspect),
        Some("ingest-usage") => parse_ingest_options(&args[1..], None).map(Command::Ingest),
        Some("ingest-codex-usage") => {
            parse_ingest_options(&args[1..], Some(CODEX_PLUGIN_ID)).map(Command::Ingest)
        }
        _ => Err(usage()),
    }
}

fn parse_inspect_options(args: &[String]) -> Result<PathBuf, String> {
    if args.len() == 2 && args[0] == "--db" && !args[1].is_empty() && !args[1].starts_with("--") {
        Ok(PathBuf::from(&args[1]))
    } else {
        Err(usage())
    }
}

fn parse_ingest_options(
    args: &[String],
    default_plugin_id: Option<&str>,
) -> Result<IngestOptions, String> {
    let mut plugin_id = default_plugin_id.map(str::to_owned);
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
            "--plugin-id" => &mut plugin_id,
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
        plugin_id: plugin_id.ok_or_else(usage)?,
        plugin_host: PathBuf::from(plugin_host.ok_or_else(usage)?),
        adapter: PathBuf::from(adapter.ok_or_else(usage)?),
        source_root: PathBuf::from(source_root.ok_or_else(usage)?),
        db: PathBuf::from(db.ok_or_else(usage)?),
        source_id: source_id.ok_or_else(usage)?,
    })
}

fn usage() -> String {
    "usage: agenturm [--help|--version|status] | agenturm inspect --db <path> | agenturm ingest-usage --plugin-id <id> --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id> | agenturm ingest-codex-usage --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>".into()
}

pub fn execute_inspect(db_path: &std::path::Path) -> Result<Snapshot, String> {
    let mut store = Store::open(db_path).map_err(|error| format!("open store failed: {error}"))?;
    let mut app = AgenturmApp::default_composition();
    Ok(app.refresh_durable(&mut store))
}

pub fn execute_ingest(options: &IngestOptions) -> Result<String, String> {
    let source_id = ExtensionId::parse(options.source_id.clone())
        .map_err(|error| format!("invalid source ID: {error}"))?;
    let config = ProcessConfig::new(
        options.plugin_host.clone(),
        options.adapter.clone(),
        options.source_root.clone(),
        options.source_id.clone(),
        options.plugin_id.clone(),
    );
    let source = ProcessSource::spawn_with_decoder(config, UsageBatchDecoder)
        .map_err(|error| error.to_string())?;
    let mut store =
        Store::open(&options.db).map_err(|error| format!("open store failed: {error}"))?;
    let mut kernel = Kernel::new();
    kernel
        .register_source_adapter(source_id.clone(), source)
        .map_err(|error| format!("register source adapter failed: {error}"))?;
    let snapshot = kernel.refresh_with_store(&mut store);
    if let Some(failure) = snapshot.failures.first() {
        if failure.stage == agenturm_kernel::ExtensionStage::AdmissionRead
            && snapshot
                .admissions
                .get(&source_id)
                .is_some_and(|status| status.source_ack_sent)
        {
            return Err(format!(
                "source ACK was sent after durable admission, but refreshing stored history failed; retrying the read is safe: {}",
                failure.message
            ));
        }
        return Err(
            if failure.stage == agenturm_kernel::ExtensionStage::AdmissionAck {
                format!(
                    "observations were committed durably, but source ACK failed; retry is safe: {}",
                    failure.message
                )
            } else {
                format!("{:?}: {}", failure.stage, failure.message)
            },
        );
    }
    let status = snapshot
        .admissions
        .get(&source_id)
        .ok_or_else(|| "kernel did not return a source admission result".to_owned())?;
    let summary = if status.replay {
        "Batch was already committed".into()
    } else {
        format!("Committed {} usage observation(s)", status.inserted)
    };
    Ok(format!(
        "{summary}; source ACK sent. Skipped {} non-usage fact(s) by design.\n",
        status.skipped_non_usage_facts
    ))
}

pub fn render(command: Command) -> String {
    match command {
        Command::Help => concat!(
            "Agenturm · Rust application (early composition shell)\n",
            "\n",
            "Usage: agenturm [--help|--version|status]\n",
            "       agenturm inspect --db <path>\n",
            "       agenturm ingest-usage --plugin-id <id> --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>\n",
            "       agenturm ingest-codex-usage --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>\n",
            "\n",
            "This explicit projection stores token counters and skips other supported facts.\n",
            "Use `agenturm inspect --db <path>` to view committed history in the full-screen interface.\n",
            "Persistent source configuration is not yet available.\n",
        )
        .into(),
        Command::Version => format!("agenturm {}\n", env!("CARGO_PKG_VERSION")),
        Command::Status => concat!(
            "Agenturm Rust kernel: ready\n",
            "Registered harnesses: 0\n",
            "Registered capabilities: 0\n",
            "Registered UI contributions: 0\n",
            "Production storage: not connected (no persistent app configuration)\n",
            "One-shot registered-plugin usage projection: available\n",
        )
        .into(),
        Command::Inspect(_) => String::new(),
        Command::Ingest(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixtureHarness;

    impl Harness for FixtureHarness {
        fn poll(&mut self) -> Result<Vec<agenturm_kernel::RawObservation>, String> {
            Ok(vec![agenturm_kernel::RawObservation {
                event_id: "fixture:event-1".into(),
                revision: 0,
                identity: agenturm_kernel::ObservationIdentity {
                    session_id: "session-1".into(),
                    worker_id: None,
                    turn_id: None,
                    operation_id: None,
                },
                observed_at_unix_ns: 1,
                kind: agenturm_kernel::ObservationKind::Activity {
                    category: "tool".into(),
                },
            }])
        }
    }

    struct ActivityCount;

    impl Capability for ActivityCount {
        fn evaluate(
            &self,
            observations: &[agenturm_kernel::Observation],
        ) -> Result<Vec<agenturm_kernel::MetricRow>, String> {
            Ok(vec![agenturm_kernel::MetricRow {
                label: "Activity events".into(),
                value: observations.len().to_string(),
            }])
        }
    }

    struct ActivityPane;

    impl UiContribution for ActivityPane {
        fn build_pane(&self, snapshot: &Snapshot) -> Result<agenturm_kernel::PaneModel, String> {
            Ok(agenturm_kernel::PaneModel {
                title: "Activity".into(),
                rows: vec![agenturm_kernel::MetricRow {
                    label: "Events".into(),
                    value: snapshot.observations.len().to_string(),
                }],
            })
        }
    }

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
        assert_eq!(
            parse_command(&["inspect".into(), "--db".into(), "./state.sqlite".into()]).unwrap(),
            Command::Inspect(PathBuf::from("./state.sqlite"))
        );
        assert!(parse_command(&["inspect".into()]).is_err());
        assert!(parse_command(&["inspect".into(), "--db".into(), String::new()]).is_err());
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
        let Command::Ingest(codex) = parse_command(&args).unwrap() else {
            panic!("Codex usage command must parse as ingestion");
        };
        assert_eq!(codex.plugin_id, CODEX_PLUGIN_ID);
        assert!(parse_command(&args[..args.len() - 2]).is_err());
        let mut duplicate = args.to_vec();
        duplicate.extend(["--db".into(), "/tmp/other.sqlite".into()]);
        assert!(parse_command(&duplicate).is_err());

        let generic = [
            "ingest-usage",
            "--plugin-id",
            "example.rust-pi-harness",
            "--plugin-host",
            "/bin/agenturm-plugin-host",
            "--adapter",
            "/bin/agenturm-pi-harness",
            "--source-root",
            "/tmp/pi.jsonl",
            "--db",
            "/tmp/agenturm.sqlite",
            "--source-id",
            "harness.pi",
        ]
        .map(str::to_owned);
        let Command::Ingest(pi) = parse_command(&generic).unwrap() else {
            panic!("generic usage command must parse as ingestion");
        };
        assert_eq!(pi.plugin_id, "example.rust-pi-harness");
        assert!(parse_command(&generic[..2]).is_err());
    }

    #[test]
    fn status_output_does_not_claim_unwired_components() {
        let status = render(Command::Status);
        assert!(status.contains("Registered harnesses: 0"));
        assert!(status.contains("Production storage: not connected"));
        assert!(status.contains("One-shot registered-plugin usage projection: available"));
    }

    #[test]
    fn app_composes_harness_capability_and_generic_pane_without_kernel_access() {
        let mut app = AgenturmApp::empty_composition();
        app.register_harness(
            ExtensionId::parse("harness.fixture").unwrap(),
            FixtureHarness,
        )
        .unwrap();
        app.register_capability(
            ExtensionId::parse("capability.activity-count").unwrap(),
            ActivityCount,
        )
        .unwrap();
        app.register_ui_contribution(ExtensionId::parse("pane.activity").unwrap(), ActivityPane)
            .unwrap();

        let snapshot = app.refresh();

        assert_eq!(snapshot.observations.len(), 1);
        assert_eq!(
            snapshot.metrics[&ExtensionId::parse("capability.activity-count").unwrap()][0].value,
            "1"
        );
        assert_eq!(
            snapshot.panes[&ExtensionId::parse("pane.activity").unwrap()].rows[0].value,
            "1"
        );
    }

    #[test]
    fn default_composition_registers_a_data_only_overview() {
        let mut app = AgenturmApp::default_composition();
        let snapshot = app.refresh();
        let pane = &snapshot.panes[&ExtensionId::parse("pane.overview").unwrap()];
        assert_eq!(pane.title, "Overview");
        assert_eq!(pane.rows[0].value, "0");
        let metrics = &snapshot.metrics[&ExtensionId::parse("capability.overview").unwrap()];
        assert_eq!(metrics[0].label, "Usage events with reported totals");
        assert_eq!(metrics[0].value, "0 / 0");
    }
}
