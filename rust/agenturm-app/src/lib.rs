//! Rust application composition boundary.
//!
//! The app exposes generic kernel extension registration, durable refresh,
//! explicit one-shot and configured multi-source usage collection, and a
//! full-screen durable-history viewer. Continuous collection is still open.

use agenturm_kernel::{
    Capability, ExtensionId, Harness, Kernel, MetricRow, Observation, ObservationKind,
    ObservationStore, PaneModel, RegistrationError, Snapshot, SourceAdapter, UiContribution,
};
use agenturm_runtime::{CODEX_PLUGIN_ID, ProcessConfig, ProcessSource, UsageBatchDecoder};
use agenturm_store::Store;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

const MAX_COLLECTION_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_CONFIGURED_SOURCES: usize = 16;

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
    Collect(ConfiguredCollectOptions),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfiguredCollectOptions {
    pub config: PathBuf,
    pub db: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionReport {
    pub summary: String,
    pub failed_sources: usize,
    pub global_failures: Vec<String>,
}

impl CollectionReport {
    pub fn has_failures(&self) -> bool {
        self.failed_sources > 0 || !self.global_failures.is_empty()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionConfigFile {
    version: u32,
    plugin_host: PathBuf,
    sources: Vec<ConfiguredSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfiguredSource {
    plugin_id: String,
    adapter: PathBuf,
    source_root: PathBuf,
    source_id: String,
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
        Some("collect") => parse_collect_options(&args[1..]).map(Command::Collect),
        Some("ingest-usage") => parse_ingest_options(&args[1..], None).map(Command::Ingest),
        Some("ingest-codex-usage") => {
            parse_ingest_options(&args[1..], Some(CODEX_PLUGIN_ID)).map(Command::Ingest)
        }
        _ => Err(usage()),
    }
}

fn parse_collect_options(args: &[String]) -> Result<ConfiguredCollectOptions, String> {
    if args.len() != 4
        || args[0] != "--config"
        || args[1].is_empty()
        || args[2] != "--db"
        || args[3].is_empty()
    {
        return Err(usage());
    }
    Ok(ConfiguredCollectOptions {
        config: PathBuf::from(&args[1]),
        db: PathBuf::from(&args[3]),
    })
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
    "usage: agenturm [--help|--version|status] | agenturm inspect --db <path> | agenturm collect --config <json-path> --db <path> | agenturm ingest-usage --plugin-id <id> --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id> | agenturm ingest-codex-usage --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>".into()
}

pub fn execute_inspect(db_path: &std::path::Path) -> Result<Snapshot, String> {
    let mut store = Store::open(db_path).map_err(|error| format!("open store failed: {error}"))?;
    let mut app = AgenturmApp::default_composition();
    Ok(app.refresh_durable(&mut store))
}

pub fn execute_collect(options: &ConfiguredCollectOptions) -> Result<CollectionReport, String> {
    let config = load_collection_config(&options.config)?;
    let mut store =
        Store::open(&options.db).map_err(|error| format!("open store failed: {error}"))?;
    let mut kernel = Kernel::new();
    let mut source_ids = Vec::with_capacity(config.sources.len());
    let mut startup_failures = BTreeMap::new();
    for source in config.sources {
        let source_id = ExtensionId::parse(source.source_id.clone())
            .map_err(|error| format!("invalid source ID: {error}"))?;
        source_ids.push(source_id.clone());
        let process_config = ProcessConfig::new(
            config.plugin_host.clone(),
            source.adapter,
            source.source_root,
            source.source_id,
            source.plugin_id,
        );
        match ProcessSource::spawn_with_decoder(process_config, UsageBatchDecoder) {
            Ok(adapter) => {
                if let Err(error) = kernel.register_source_adapter(source_id.clone(), adapter) {
                    startup_failures.insert(source_id, error.to_string());
                }
            }
            Err(error) => {
                startup_failures.insert(source_id, error.to_string());
            }
        }
    }

    let snapshot = kernel.refresh_with_store(&mut store);
    Ok(build_collection_report(
        &source_ids,
        &startup_failures,
        &snapshot,
    ))
}

fn build_collection_report(
    source_ids: &[ExtensionId],
    startup_failures: &BTreeMap<ExtensionId, String>,
    snapshot: &Snapshot,
) -> CollectionReport {
    let mut lines = Vec::with_capacity(source_ids.len());
    let mut failed_sources = 0;
    let mut global_failures = Vec::new();
    let storage_id = ExtensionId::parse("kernel.store").expect("storage extension ID is valid");
    for failure in snapshot
        .failures
        .iter()
        .filter(|failure| failure.extension_id == storage_id)
    {
        global_failures.push(format!("{:?}: {}", failure.stage, failure.message));
    }
    let source_count = source_ids.len();
    for source_id in source_ids {
        if let Some(message) = startup_failures.get(source_id) {
            failed_sources += 1;
            lines.push(format!("{}: startup failed: {message}", source_id.as_str()));
            continue;
        }
        if let Some(failure) = snapshot
            .failures
            .iter()
            .find(|failure| failure.extension_id == *source_id)
        {
            failed_sources += 1;
            lines.push(format!(
                "{}: {:?}: {}",
                source_id.as_str(),
                failure.stage,
                failure.message
            ));
            continue;
        }
        match snapshot.admissions.get(source_id) {
            Some(status) => {
                let outcome = if status.replay {
                    format!("replay confirmed ({} newly inserted)", status.inserted)
                } else {
                    format!("committed {} usage observation(s)", status.inserted)
                };
                lines.push(format!(
                    "{}: {outcome}, skipped {} non-usage fact(s), ACK sent={}",
                    source_id.as_str(),
                    status.skipped_non_usage_facts,
                    status.source_ack_sent
                ));
            }
            None => {
                failed_sources += 1;
                lines.push(format!("{}: no admission result", source_id.as_str()));
            }
        }
    }
    let global_summary = global_failures
        .iter()
        .map(|failure| format!("storage: {failure}"))
        .collect::<Vec<_>>();
    let mut all_lines = lines;
    all_lines.extend(global_summary);
    let source_failure_word = if failed_sources == 1 {
        "failure"
    } else {
        "failures"
    };
    let global_failure_word = if global_failures.len() == 1 {
        "failure"
    } else {
        "failures"
    };
    let source_word = if source_count == 1 {
        "source"
    } else {
        "sources"
    };
    CollectionReport {
        summary: format!(
            "{} {source_word} checked; {} source {source_failure_word}; {} global {global_failure_word}\n{}\n",
            source_count,
            failed_sources,
            global_failures.len(),
            all_lines.join("\n")
        ),
        failed_sources,
        global_failures,
    }
}

fn load_collection_config(path: &std::path::Path) -> Result<ValidatedCollectionConfig, String> {
    let file =
        File::open(path).map_err(|error| format!("open collection config failed: {error}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_COLLECTION_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read collection config failed: {error}"))?;
    if bytes.len() as u64 > MAX_COLLECTION_CONFIG_BYTES {
        return Err("collection config exceeds the 256 KiB limit".into());
    }
    let mut config: CollectionConfigFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid collection config: {error}"))?;
    if config.version != 1 {
        return Err("unsupported collection config version".into());
    }
    if config.sources.is_empty() || config.sources.len() > MAX_CONFIGURED_SOURCES {
        return Err(format!(
            "collection config must define 1 to {MAX_CONFIGURED_SOURCES} sources"
        ));
    }

    let base = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let base = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("resolve collection config directory failed: {error}"))?
            .join(base)
    };
    let resolve = |value: PathBuf| {
        if value.is_absolute() {
            value
        } else {
            base.join(value)
        }
    };
    if config.plugin_host.as_os_str().is_empty() {
        return Err("plugin host path must not be empty".into());
    }
    config.plugin_host = resolve(config.plugin_host);
    if !valid_config_path(&config.plugin_host) || !config.plugin_host.is_absolute() {
        return Err(
            "plugin host path must be nonempty, bounded, and absolute after resolution".into(),
        );
    }

    let mut seen_source_ids = BTreeSet::new();
    let mut seen_collection_targets = BTreeSet::new();
    for source in &mut config.sources {
        ExtensionId::parse(source.source_id.clone())
            .map_err(|error| format!("invalid source ID: {error}"))?;
        ExtensionId::parse(source.plugin_id.clone())
            .map_err(|_| "invalid plugin ID in collection config".to_owned())?;
        if !seen_source_ids.insert(source.source_id.clone()) {
            return Err(format!("duplicate source ID: {}", source.source_id));
        }
        if source.adapter.as_os_str().is_empty() || source.source_root.as_os_str().is_empty() {
            return Err("adapter and source-root paths must not be empty".into());
        }
        source.adapter = resolve(std::mem::take(&mut source.adapter));
        source.source_root = resolve(std::mem::take(&mut source.source_root));
        if !valid_config_path(&source.adapter)
            || !valid_config_path(&source.source_root)
            || !source.adapter.is_absolute()
            || !source.source_root.is_absolute()
        {
            return Err(
                "adapter and source-root paths must be nonempty, bounded, and absolute after resolution"
                    .into(),
            );
        }
        let target_identity = (
            normalized_path_identity(&source.adapter),
            normalized_path_identity(&source.source_root),
        );
        if !seen_collection_targets.insert(target_identity) {
            return Err("the same adapter and source-root pair cannot be configured twice".into());
        }
    }
    Ok(ValidatedCollectionConfig {
        plugin_host: config.plugin_host,
        sources: config.sources,
    })
}

fn valid_config_path(path: &std::path::Path) -> bool {
    !path.as_os_str().is_empty() && path.to_string_lossy().len() <= 4_096
}

fn normalized_path_identity(path: &std::path::Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                normalized.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

#[derive(Debug)]
struct ValidatedCollectionConfig {
    plugin_host: PathBuf,
    sources: Vec<ConfiguredSource>,
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
            "       agenturm collect --config <json-path> --db <path>\n",
            "       agenturm ingest-usage --plugin-id <id> --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>\n",
            "       agenturm ingest-codex-usage --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>\n",
            "\n",
            "This explicit projection stores token counters and skips other supported facts.\n",
            "Use `agenturm inspect --db <path>` to view committed history in the full-screen interface.\n",
            "Collection config is versioned JSON; relative executable and source paths resolve beside the config.\n",
        )
        .into(),
        Command::Version => format!("agenturm {}\n", env!("CARGO_PKG_VERSION")),
        Command::Status => concat!(
            "Agenturm Rust kernel: ready\n",
            "Registered harnesses: 0\n",
            "Registered capabilities: 0\n",
            "Registered UI contributions: 0\n",
            "Production storage service: not connected (collection is one-shot)\n",
            "One-shot registered-plugin usage projection: available\n",
            "Configured multi-source collection: available (one-shot)\n",
        )
        .into(),
        Command::Inspect(_) => String::new(),
        Command::Ingest(_) => String::new(),
        Command::Collect(_) => String::new(),
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
        assert_eq!(
            parse_command(&[
                "collect".into(),
                "--config".into(),
                "sources.json".into(),
                "--db".into(),
                "state.sqlite".into(),
            ])
            .unwrap(),
            Command::Collect(ConfiguredCollectOptions {
                config: PathBuf::from("sources.json"),
                db: PathBuf::from("state.sqlite"),
            })
        );
        assert!(
            parse_command(&["collect".into(), "--config".into(), "sources.json".into()]).is_err()
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
        assert!(status.contains("Production storage service: not connected"));
        assert!(status.contains("One-shot registered-plugin usage projection: available"));
        assert!(status.contains("Configured multi-source collection: available (one-shot)"));
    }

    #[test]
    fn global_store_failures_make_collection_report_fail() {
        let mut snapshot = Snapshot::default();
        snapshot.failures.push(agenturm_kernel::ExtensionFailure {
            extension_id: ExtensionId::parse("kernel.store").unwrap(),
            stage: agenturm_kernel::ExtensionStage::AdmissionRead,
            message: "injected durable history read failure".into(),
        });
        let report = build_collection_report(&[], &BTreeMap::new(), &snapshot);
        assert_eq!(report.failed_sources, 0);
        assert_eq!(report.global_failures.len(), 1);
        assert!(
            report
                .summary
                .contains("0 sources checked; 0 source failures; 1 global failure")
        );
        assert!(report.summary.contains("storage: AdmissionRead"));
        assert!(report.has_failures());
    }

    #[test]
    fn one_source_poll_failure_does_not_hide_another_admission() {
        let failed_source = ExtensionId::parse("harness.failed").unwrap();
        let healthy_source = ExtensionId::parse("harness.healthy").unwrap();
        let mut snapshot = Snapshot::default();
        snapshot.failures.push(agenturm_kernel::ExtensionFailure {
            extension_id: failed_source.clone(),
            stage: agenturm_kernel::ExtensionStage::AdmissionPoll,
            message: "injected source poll failure".into(),
        });
        snapshot.admissions.insert(
            healthy_source.clone(),
            agenturm_kernel::AdmissionStatus {
                inserted: 2,
                replay: false,
                skipped_non_usage_facts: 0,
                source_ack_sent: true,
            },
        );
        let report = build_collection_report(
            &[failed_source, healthy_source],
            &BTreeMap::new(),
            &snapshot,
        );
        assert!(report.summary.contains("harness.failed: AdmissionPoll"));
        assert!(
            report
                .summary
                .contains("harness.healthy: committed 2 usage")
        );
        assert!(report.summary.contains("ACK sent=true"));
        assert_eq!(report.failed_sources, 1);
        assert!(report.has_failures());
    }

    #[test]
    fn collection_config_resolves_relative_paths_and_rejects_duplicate_sources() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-app-config-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("sources.json");
        std::fs::write(
            &config_path,
            r#"{"version":1,"plugin_host":"bin/agenturm-plugin-host","sources":[{"plugin_id":"example.rust-pi-harness","adapter":"bin/agenturm-pi-harness","source_root":"data/pi.jsonl","source_id":"harness.pi"},{"plugin_id":"example.rust-codex-harness","adapter":"bin/agenturm-codex-harness","source_root":"data/codex.jsonl","source_id":"harness.codex"}]}"#,
        )
        .unwrap();
        let config = load_collection_config(&config_path).unwrap();
        assert_eq!(config.sources.len(), 2);
        assert_eq!(
            config.plugin_host,
            directory.join("bin/agenturm-plugin-host")
        );
        assert_eq!(
            config.sources[0].adapter,
            directory.join("bin/agenturm-pi-harness")
        );
        assert_eq!(config.sources[1].source_id, "harness.codex");

        let relative_path = PathBuf::from(format!(
            ".agenturm-config-relative-{}-{nonce}.json",
            std::process::id()
        ));
        std::fs::write(
            &relative_path,
            r#"{"version":1,"plugin_host":"host","sources":[{"plugin_id":"example.harness","adapter":"adapter","source_root":"root","source_id":"harness.relative"}]}"#,
        )
        .unwrap();
        let relative_config = load_collection_config(&relative_path).unwrap();
        assert!(relative_config.plugin_host.is_absolute());
        assert!(relative_config.sources[0].source_root.is_absolute());
        std::fs::remove_file(relative_path).unwrap();

        std::fs::write(
            &config_path,
            r#"{"version":1,"plugin_host":"host","sources":[{"plugin_id":"example.harness","adapter":"adapter","source_root":"root","source_id":"harness.same"},{"plugin_id":"example.harness","adapter":"adapter","source_root":"root","source_id":"harness.same"}]}"#,
        )
        .unwrap();
        assert!(
            load_collection_config(&config_path)
                .unwrap_err()
                .contains("duplicate source ID")
        );

        std::fs::write(
            &config_path,
            r#"{"version":1,"plugin_host":"host","sources":[{"plugin_id":"example.harness","adapter":"sub/../adapter","source_root":"root","source_id":"harness.one"},{"plugin_id":"example.harness","adapter":"adapter","source_root":"root","source_id":"harness.two"}]}"#,
        )
        .unwrap();
        assert!(
            load_collection_config(&config_path)
                .unwrap_err()
                .contains("cannot be configured twice")
        );

        std::fs::write(
            &config_path,
            r#"{"version":1,"plugin_host":"host","sources":[{"plugin_id":"example.harness","adapter":"","source_root":"root","source_id":"harness.one"}]}"#,
        )
        .unwrap();
        assert!(
            load_collection_config(&config_path)
                .unwrap_err()
                .contains("must not be empty")
        );

        std::fs::write(
            &config_path,
            r#"{"version":1,"plugin_host":"","sources":[{"plugin_id":"example.harness","adapter":"adapter","source_root":"root","source_id":"harness.one"}]}"#,
        )
        .unwrap();
        assert!(
            load_collection_config(&config_path)
                .unwrap_err()
                .contains("plugin host path must not be empty")
        );

        let sources = (0..MAX_CONFIGURED_SOURCES)
            .map(|index| {
                serde_json::json!({
                    "plugin_id": "example.harness",
                    "adapter": format!("adapter-{index}"),
                    "source_root": format!("root-{index}"),
                    "source_id": format!("harness.source-{index}")
                })
            })
            .collect::<Vec<_>>();
        let at_limit = serde_json::json!({
            "version": 1,
            "plugin_host": "host",
            "sources": sources
        });
        std::fs::write(&config_path, serde_json::to_vec(&at_limit).unwrap()).unwrap();
        assert_eq!(
            load_collection_config(&config_path).unwrap().sources.len(),
            16
        );

        let over_limit = serde_json::json!({
            "version": 1,
            "plugin_host": "host",
            "sources": (0..=MAX_CONFIGURED_SOURCES)
                .map(|index| serde_json::json!({
                    "plugin_id": "example.harness",
                    "adapter": format!("adapter-{index}"),
                    "source_root": format!("root-{index}"),
                    "source_id": format!("harness.source-{index}")
                }))
                .collect::<Vec<_>>()
        });
        std::fs::write(&config_path, serde_json::to_vec(&over_limit).unwrap()).unwrap();
        assert!(
            load_collection_config(&config_path)
                .unwrap_err()
                .contains("1 to 16 sources")
        );

        std::fs::write(
            &config_path,
            vec![b' '; MAX_COLLECTION_CONFIG_BYTES as usize + 1],
        )
        .unwrap();
        assert!(
            load_collection_config(&config_path)
                .unwrap_err()
                .contains("256 KiB limit")
        );
        std::fs::remove_dir_all(directory).unwrap();
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
