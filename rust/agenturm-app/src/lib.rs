//! Rust application composition boundary.
//!
//! The app exposes generic kernel extension registration, durable refresh,
//! explicit one-shot and configured multi-source usage collection, and a
//! full-screen durable-history viewer. Installed service management remains open.

use agenturm_kernel::{
    Capability, ExtensionId, Harness, Kernel, MAX_DURABLE_SNAPSHOT_OBSERVATIONS, MetricRow,
    Observation, ObservationKind, ObservationPosition, ObservationQuery, ObservationStore,
    PaneModel, RegistrationError, Snapshot, SourceAdapter, UiContribution,
};
use agenturm_runtime::{CODEX_PLUGIN_ID, ProcessConfig, ProcessSource, UsageBatchDecoder};
#[cfg(target_os = "linux")]
use agenturm_service::{
    ServiceSpec, Systemctl, install_user_service, status_user_service, uninstall_user_service,
};
use agenturm_store::Store;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const MAX_COLLECTION_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_CONFIGURED_SOURCES: usize = 16;
const INSPECT_PAGE_SIZE: usize = 500;

#[cfg(target_os = "linux")]
const SERVICE_STATUS_LINE: &str =
    "Managed user service: Linux systemd install/status/uninstall available\n";
#[cfg(not(target_os = "linux"))]
const SERVICE_STATUS_LINE: &str = "Managed user service: systemd management unavailable\n";

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

    pub fn refresh_history_query(
        &mut self,
        store: &impl ObservationStore,
        query: ObservationQuery,
    ) -> Snapshot {
        self.kernel.refresh_query_with_store(store, query)
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
    Query(HistoryQueryOptions),
    Ingest(IngestOptions),
    Collect(ConfiguredCollectOptions),
    Service(ServiceCommand),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceCommand {
    Install {
        config: PathBuf,
        db: PathBuf,
        interval_ms: u64,
    },
    Uninstall,
    Status,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfiguredCollectOptions {
    pub config: PathBuf,
    pub db: PathBuf,
    pub watch_interval: Option<Duration>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WatchReport {
    pub cycles: usize,
    pub had_failures: bool,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryQueryOptions {
    pub db: PathBuf,
    pub harness_id: Option<String>,
    pub session_id: Option<String>,
    pub before: Option<ObservationPosition>,
    pub snapshot_id: Option<u64>,
    pub limit: usize,
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
        Some("query") => parse_query_options(&args[1..]).map(Command::Query),
        Some("collect") => parse_collect_options(&args[1..]).map(Command::Collect),
        Some("service") => parse_service_options(&args[1..]).map(Command::Service),
        Some("ingest-usage") => parse_ingest_options(&args[1..], None).map(Command::Ingest),
        Some("ingest-codex-usage") => {
            parse_ingest_options(&args[1..], Some(CODEX_PLUGIN_ID)).map(Command::Ingest)
        }
        _ => Err(usage()),
    }
}

pub fn execute_service(action: &ServiceCommand) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        execute_systemd_service(action)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = action;
        Err("Agenturm systemd user service management currently supports Linux only".into())
    }
}

#[cfg(target_os = "linux")]
fn execute_systemd_service(action: &ServiceCommand) -> Result<String, String> {
    let unit_directory = systemd_user_unit_directory()?;
    let mut manager = Systemctl;
    match action {
        ServiceCommand::Install {
            config,
            db,
            interval_ms,
        } => {
            let config = std::fs::canonicalize(config)
                .map_err(|error| format!("resolve collection config failed: {error}"))?;
            let validated = load_collection_config(&config)?;
            let executable = std::env::current_exe()
                .map_err(|error| format!("resolve Agenturm executable failed: {error}"))?;
            let database = absolute_database_path(db)?;
            validate_service_database_isolation(&database, &validated.sources)?;
            let database = prepare_database_path(&database)?;
            let spec = ServiceSpec {
                executable,
                collection_config: config,
                database,
                interval_ms: *interval_ms,
            };
            let path = install_user_service(&spec, &unit_directory, &mut manager)?;
            Ok(format!(
                "Installed and started Agenturm user service at {}.\n",
                path.display()
            ))
        }
        ServiceCommand::Uninstall => {
            uninstall_user_service(&unit_directory, &mut manager)?;
            Ok("Stopped and uninstalled Agenturm user service.\n".into())
        }
        ServiceCommand::Status => status_user_service(&mut manager),
    }
}

#[cfg(target_os = "linux")]
fn validate_service_database_isolation(
    database: &std::path::Path,
    sources: &[ConfiguredSource],
) -> Result<(), String> {
    let database_directory = database
        .parent()
        .ok_or_else(|| "database path must have a parent directory".to_owned())?;
    let database_directory = canonicalize_with_missing_suffix(database_directory)?;
    for source in sources {
        let source_root = canonicalize_with_missing_suffix(&source.source_root)?;
        if database_directory.starts_with(&source_root)
            || source_root.starts_with(&database_directory)
        {
            return Err(
                "database directory and configured source roots must not overlap; the service grants its database directory write access".into(),
            );
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn systemd_user_unit_directory() -> Result<PathBuf, String> {
    let config_home = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("XDG_CONFIG_HOME must be an absolute path".into());
            }
            path
        }
        None => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| "HOME is unavailable; set XDG_CONFIG_HOME".to_owned())?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(config_home.join("systemd").join("user"))
}

#[cfg(target_os = "linux")]
fn absolute_database_path(path: &std::path::Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("resolve working directory failed: {error}"))?
            .join(path)
    };
    if absolute.exists() {
        if !absolute.is_file() {
            return Err("database path is not a regular file".into());
        }
        std::fs::canonicalize(absolute)
            .map_err(|error| format!("resolve database file failed: {error}"))
    } else {
        canonicalize_with_missing_suffix(&absolute)
    }
}

#[cfg(target_os = "linux")]
fn prepare_database_path(path: &std::path::Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| "database path must name a file".to_owned())?;
    let parent = path
        .parent()
        .ok_or_else(|| "database path must have a parent directory".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create database directory failed: {error}"))?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|error| format!("resolve database directory failed: {error}"))?;
    let prepared = parent.join(name);
    if prepared.exists() {
        if !prepared.is_file() {
            return Err("database path is not a regular file".into());
        }
        std::fs::canonicalize(prepared)
            .map_err(|error| format!("resolve database file failed: {error}"))
    } else {
        Ok(prepared)
    }
}

fn parse_service_options(args: &[String]) -> Result<ServiceCommand, String> {
    match args.first().map(String::as_str) {
        Some("install") => {
            let mut config = None;
            let mut db = None;
            let mut interval_ms = None;
            let mut index = 1;
            while index < args.len() {
                let key = args[index].as_str();
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .ok_or_else(usage)?;
                let slot = match key {
                    "--config" => &mut config,
                    "--db" => &mut db,
                    "--interval-ms" => &mut interval_ms,
                    _ => return Err(usage()),
                };
                if slot.replace(value.clone()).is_some() {
                    return Err(format!("duplicate option {key}\n{}", usage()));
                }
                index += 2;
            }
            let interval_ms = interval_ms
                .map(|value| value.parse::<u64>().map_err(|_| usage()))
                .transpose()?
                .unwrap_or(2_000);
            if !(100..=60_000).contains(&interval_ms) {
                return Err("service interval must be between 100 and 60000 milliseconds".into());
            }
            Ok(ServiceCommand::Install {
                config: PathBuf::from(config.ok_or_else(usage)?),
                db: PathBuf::from(db.ok_or_else(usage)?),
                interval_ms,
            })
        }
        Some("uninstall") if args.len() == 1 => Ok(ServiceCommand::Uninstall),
        Some("status") if args.len() == 1 => Ok(ServiceCommand::Status),
        _ => Err(usage()),
    }
}

fn parse_collect_options(args: &[String]) -> Result<ConfiguredCollectOptions, String> {
    let mut config = None;
    let mut db = None;
    let mut watch = false;
    let mut interval_ms = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" | "--db" | "--interval-ms" => {
                let key = args[index].as_str();
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(usage)?;
                if value.starts_with("--") {
                    return Err(usage());
                }
                let slot = match key {
                    "--config" => &mut config,
                    "--db" => &mut db,
                    "--interval-ms" => &mut interval_ms,
                    _ => unreachable!(),
                };
                if slot.replace(value.clone()).is_some() {
                    return Err(format!("duplicate option {key}\n{}", usage()));
                }
                index += 2;
            }
            "--watch" if !watch => {
                watch = true;
                index += 1;
            }
            _ => return Err(usage()),
        }
    }
    if interval_ms.is_some() && !watch {
        return Err("--interval-ms requires --watch".into());
    }
    let watch_interval = if watch {
        let interval_ms = interval_ms
            .map(|value| value.parse::<u64>().map_err(|_| usage()))
            .transpose()?
            .unwrap_or(2_000);
        if !(100..=60_000).contains(&interval_ms) {
            return Err("watch interval must be between 100 and 60000 milliseconds".into());
        }
        Some(Duration::from_millis(interval_ms))
    } else {
        None
    };
    Ok(ConfiguredCollectOptions {
        config: PathBuf::from(config.ok_or_else(usage)?),
        db: PathBuf::from(db.ok_or_else(usage)?),
        watch_interval,
    })
}

fn parse_inspect_options(args: &[String]) -> Result<PathBuf, String> {
    if args.len() == 2 && args[0] == "--db" && !args[1].is_empty() && !args[1].starts_with("--") {
        Ok(PathBuf::from(&args[1]))
    } else {
        Err(usage())
    }
}

fn parse_query_options(args: &[String]) -> Result<HistoryQueryOptions, String> {
    let mut db = None;
    let mut harness_id = None;
    let mut session_id = None;
    let mut before_timestamp = None;
    let mut before_harness = None;
    let mut before_event = None;
    let mut before_revision = None;
    let mut before_snapshot_id = None;
    let mut limit = None;
    let mut index = 0;
    while index < args.len() {
        let key = &args[index];
        let value = args.get(index + 1).ok_or_else(usage)?;
        if value.starts_with("--") || value.is_empty() {
            return Err(usage());
        }
        let slot = match key.as_str() {
            "--db" => &mut db,
            "--harness-id" => &mut harness_id,
            "--session-id" => &mut session_id,
            "--before-timestamp-ns" => &mut before_timestamp,
            "--before-harness-id" => &mut before_harness,
            "--before-event-id" => &mut before_event,
            "--before-revision" => &mut before_revision,
            "--snapshot-id" => &mut before_snapshot_id,
            "--limit" => &mut limit,
            _ => return Err(usage()),
        };
        if slot.replace(value.clone()).is_some() {
            return Err(format!("duplicate option {key}\n{}", usage()));
        }
        index += 2;
    }
    let before_parts = [
        before_timestamp.as_ref(),
        before_harness.as_ref(),
        before_event.as_ref(),
        before_revision.as_ref(),
    ];
    let snapshot_id = before_snapshot_id
        .as_deref()
        .map(|value| value.parse::<u64>().map_err(|_| usage()))
        .transpose()?;
    let before = if before_parts.iter().all(Option::is_none) {
        None
    } else if before_parts.iter().all(Option::is_some) {
        let snapshot_id = snapshot_id.ok_or_else(usage)?;
        Some(ObservationPosition {
            observed_at_unix_ns: before_timestamp
                .as_deref()
                .and_then(|value| value.parse().ok())
                .ok_or_else(usage)?,
            harness_id: before_harness.ok_or_else(usage)?,
            event_id: before_event.ok_or_else(usage)?,
            revision: before_revision
                .as_deref()
                .and_then(|value| value.parse().ok())
                .ok_or_else(usage)?,
            snapshot_id,
        })
    } else {
        return Err(usage());
    };
    let limit = limit
        .map(|value| value.parse::<usize>().map_err(|_| usage()))
        .transpose()?
        .unwrap_or(500);
    if limit == 0 || limit > MAX_DURABLE_SNAPSHOT_OBSERVATIONS {
        return Err(usage());
    }
    Ok(HistoryQueryOptions {
        db: PathBuf::from(db.ok_or_else(usage)?),
        harness_id,
        session_id,
        before,
        snapshot_id,
        limit,
    })
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
    "usage: agenturm [--help|--version|status] | agenturm inspect --db <path> | agenturm query --db <path> [--harness-id <id>] [--session-id <id>] [--limit <1..16384>] [--snapshot-id <id>] [--before-timestamp-ns <ns> --before-harness-id <id> --before-event-id <id> --before-revision <n>] | agenturm collect --config <json-path> --db <path> [--watch [--interval-ms <100..60000>]] | agenturm service install --config <json-path> --db <path> [--interval-ms <100..60000>] | agenturm service status | agenturm service uninstall | agenturm ingest-usage --plugin-id <id> --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id> | agenturm ingest-codex-usage --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>".into()
}

pub fn execute_inspect(db_path: &std::path::Path) -> Result<Snapshot, String> {
    let store = Store::open(db_path).map_err(|error| format!("open store failed: {error}"))?;
    let mut app = AgenturmApp::default_composition();
    Ok(app.refresh_history_query(&store, ObservationQuery::latest(INSPECT_PAGE_SIZE)))
}

pub fn execute_inspect_ui(db_path: &std::path::Path) -> Result<(), String> {
    let store = Store::open(db_path).map_err(|error| format!("open store failed: {error}"))?;
    let mut app = AgenturmApp::default_composition();
    let snapshot = app.refresh_history_query(&store, ObservationQuery::latest(INSPECT_PAGE_SIZE));
    agenturm_ui::run_with_history(snapshot, |before| {
        Ok(Some(app.refresh_history_query(
            &store,
            ObservationQuery {
                harness_id: None,
                session_id: None,
                before: Some(before.clone()),
                snapshot_id: Some(before.snapshot_id),
                limit: INSPECT_PAGE_SIZE,
            },
        )))
    })
    .map_err(|error| format!("Agenturm UI failed: {error}"))
}

/// Executes one bounded read-only history query and returns a stable cursor
/// that can be passed back as `--before-*` to fetch the next older page.
pub fn execute_query(options: &HistoryQueryOptions) -> Result<String, String> {
    let store = Store::open_read_only(&options.db)
        .map_err(|error| format!("open existing store failed: {error}"))?;
    let query = ObservationQuery {
        harness_id: options.harness_id.clone(),
        session_id: options.session_id.clone(),
        before: options.before.clone(),
        snapshot_id: options.snapshot_id,
        limit: options.limit,
    };
    let page = store
        .query_observations(&query)
        .map_err(|error| format!("query store failed: {error}"))?;
    let next_before = page
        .has_more
        .then(|| {
            page.observations.first().map(|row| ObservationPosition {
                observed_at_unix_ns: row.observed_at_unix_ns,
                harness_id: row.harness_id.as_str().to_owned(),
                event_id: row.event_id.clone(),
                revision: row.revision,
                snapshot_id: page.snapshot_id,
            })
        })
        .flatten();
    let next_before = next_before.map(|position| {
        serde_json::json!({
            "observed_at_unix_ns":position.observed_at_unix_ns.to_string(),
            "harness_id":position.harness_id,
            "event_id":position.event_id,
            "revision":position.revision,
            "snapshot_id":position.snapshot_id.to_string(),
        })
    });
    let observations = page
        .observations
        .iter()
        .map(|row| {
            let kind = match &row.kind {
                ObservationKind::SessionStarted => serde_json::json!({"kind":"session_started"}),
                ObservationKind::SessionEnded => serde_json::json!({"kind":"session_ended"}),
                ObservationKind::Activity { category } => {
                    serde_json::json!({"kind":"activity", "category":category})
                }
                ObservationKind::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    total_tokens,
                } => serde_json::json!({
                    "kind":"usage", "input_tokens":input_tokens,
                    "output_tokens":output_tokens,
                    "cache_read_tokens":cache_read_tokens,
                    "cache_write_tokens":cache_write_tokens,
                    "total_tokens":total_tokens,
                }),
            };
            serde_json::json!({
                "harness_id":row.harness_id.as_str(),
                "event_id":row.event_id,
                "revision":row.revision,
                "observed_at_unix_ns":row.observed_at_unix_ns.to_string(),
                "identity": {
                    "session_id":row.identity.session_id,
                    "worker_id":row.identity.worker_id,
                    "turn_id":row.identity.turn_id,
                    "operation_id":row.identity.operation_id,
                },
                "observation":kind,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&serde_json::json!({
        "observations":observations,
        "has_more":page.has_more,
        "next_before":next_before,
    }))
    .map_err(|_| "serialize history query failed".into())
}

pub fn execute_collect(options: &ConfiguredCollectOptions) -> Result<CollectionReport, String> {
    let mut collector = ConfiguredCollector::open(options)?;
    Ok(collector.refresh())
}

pub fn execute_watch(
    options: &ConfiguredCollectOptions,
    interval: Duration,
) -> Result<WatchReport, String> {
    if interval < Duration::from_millis(100) || interval > Duration::from_secs(60) {
        return Err("watch interval must be between 100 milliseconds and 60 seconds".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = WatchSignalRegistrations::new(Arc::clone(&stop))?;
    let mut collector = ConfiguredCollector::open_with_stop(options, Some(&stop))?;

    let mut cycles = 0usize;
    let mut had_failures = false;
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut first_cycle = true;
    loop {
        if !first_cycle && stop.load(Ordering::Relaxed) {
            break;
        }
        first_cycle = false;
        let report = collector.refresh();
        had_failures |= report.has_failures();
        writeln!(output, "{}", report.summary.trim_end())
            .map_err(|error| format!("write collection report failed: {error}"))?;
        output
            .flush()
            .map_err(|error| format!("flush collection report failed: {error}"))?;
        cycles += 1;

        if stop.load(Ordering::Relaxed) {
            break;
        }

        let deadline = Instant::now() + interval;
        while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
            thread::sleep(
                Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
    drop(output);
    Ok(WatchReport {
        cycles,
        had_failures,
    })
}

struct WatchSignalRegistrations(Vec<signal_hook::SigId>);

impl WatchSignalRegistrations {
    fn new(stop: Arc<AtomicBool>) -> Result<Self, String> {
        let interrupt = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&stop))
            .map_err(|error| format!("register interrupt handler failed: {error}"))?;
        let mut registrations = Self(vec![interrupt]);
        #[cfg(unix)]
        {
            let terminate = signal_hook::flag::register(signal_hook::consts::SIGTERM, stop)
                .map_err(|error| format!("register termination handler failed: {error}"))?;
            registrations.0.push(terminate);
        }
        Ok(registrations)
    }
}

impl Drop for WatchSignalRegistrations {
    fn drop(&mut self) {
        for registration in self.0.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

pub struct ConfiguredCollector {
    kernel: Kernel,
    store: Store,
    source_ids: Vec<ExtensionId>,
    startup_failures: BTreeMap<ExtensionId, String>,
    pending_sources: BTreeMap<ExtensionId, (ProcessConfig, Instant)>,
    stop_flag: Option<Arc<AtomicBool>>,
}

impl ConfiguredCollector {
    pub fn open(options: &ConfiguredCollectOptions) -> Result<Self, String> {
        Self::open_with_stop(options, None)
    }

    fn open_with_stop(
        options: &ConfiguredCollectOptions,
        stop: Option<&Arc<AtomicBool>>,
    ) -> Result<Self, String> {
        let config = load_collection_config(&options.config)?;
        let store =
            Store::open(&options.db).map_err(|error| format!("open store failed: {error}"))?;
        let mut kernel = Kernel::new();
        let mut source_ids = Vec::with_capacity(config.sources.len());
        let mut startup_failures = BTreeMap::new();
        let mut pending_sources = BTreeMap::new();
        for source in config.sources {
            let source_id = ExtensionId::parse(source.source_id.clone())
                .map_err(|error| format!("invalid source ID: {error}"))?;
            source_ids.push(source_id.clone());
            if stop.is_some_and(|stop| stop.load(Ordering::Relaxed)) {
                startup_failures.insert(
                    source_id,
                    "startup skipped because shutdown was requested".into(),
                );
                continue;
            }
            let mut process_config = ProcessConfig::new(
                config.plugin_host.clone(),
                source.adapter,
                source.source_root,
                source.source_id,
                source.plugin_id,
            );
            if let Some(stop) = stop {
                process_config = process_config.with_stop_flag(Arc::clone(stop));
            }
            let retry_config = process_config.clone();
            match ProcessSource::spawn_with_decoder(process_config, UsageBatchDecoder) {
                Ok(adapter) => {
                    if let Err(error) = kernel.register_source_adapter(source_id.clone(), adapter) {
                        startup_failures.insert(source_id, error.to_string());
                    }
                }
                Err(error) => {
                    startup_failures.insert(source_id.clone(), error.to_string());
                    if stop.is_some() {
                        pending_sources.insert(
                            source_id,
                            (retry_config, Instant::now() + Duration::from_secs(1)),
                        );
                    }
                }
            }
        }

        Ok(Self {
            kernel,
            store,
            source_ids,
            startup_failures,
            pending_sources,
            stop_flag: stop.cloned(),
        })
    }

    pub fn refresh(&mut self) -> CollectionReport {
        self.retry_pending_sources();
        let snapshot = self.kernel.refresh_with_store(&mut self.store);
        let shutdown_requested = self
            .stop_flag
            .as_ref()
            .is_some_and(|stop| stop.load(Ordering::Relaxed));
        build_collection_report(
            &self.source_ids,
            &self.startup_failures,
            &snapshot,
            shutdown_requested,
        )
    }

    fn retry_pending_sources(&mut self) {
        let Some(stop) = &self.stop_flag else {
            return;
        };
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let now = Instant::now();
        let due = self
            .pending_sources
            .iter()
            .filter(|(_, (_, retry_at))| *retry_at <= now)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in due {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let Some((config, _)) = self.pending_sources.get(&id).cloned() else {
                continue;
            };
            match ProcessSource::spawn_with_decoder(config, UsageBatchDecoder) {
                Ok(adapter) => match self.kernel.register_source_adapter(id.clone(), adapter) {
                    Ok(()) => {
                        self.pending_sources.remove(&id);
                        self.startup_failures.remove(&id);
                    }
                    Err(error) => {
                        self.pending_sources.remove(&id);
                        self.startup_failures.insert(id, error.to_string());
                    }
                },
                Err(error) => {
                    if let Some((_, retry_at)) = self.pending_sources.get_mut(&id) {
                        *retry_at = Instant::now() + Duration::from_secs(1);
                    }
                    self.startup_failures.insert(id, error.to_string());
                }
            }
        }
    }
}

fn build_collection_report(
    source_ids: &[ExtensionId],
    startup_failures: &BTreeMap<ExtensionId, String>,
    snapshot: &Snapshot,
    shutdown_requested: bool,
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
            if shutdown_requested
                && (message == "source operation cancelled"
                    || message == "startup skipped because shutdown was requested")
            {
                lines.push(format!(
                    "{}: startup cancelled by shutdown",
                    source_id.as_str()
                ));
                continue;
            }
            failed_sources += 1;
            lines.push(format!("{}: startup failed: {message}", source_id.as_str()));
            continue;
        }
        if let Some(failure) = snapshot
            .failures
            .iter()
            .find(|failure| failure.extension_id == *source_id)
        {
            if shutdown_requested && failure.message == "source operation cancelled" {
                lines.push(format!(
                    "{}: poll cancelled by shutdown",
                    source_id.as_str()
                ));
                continue;
            }
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
                if shutdown_requested {
                    lines.push(format!(
                        "{}: poll cancelled by shutdown",
                        source_id.as_str()
                    ));
                    continue;
                }
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
    lexical_normalize_path(path)
}

fn lexical_normalize_path(path: &std::path::Path) -> PathBuf {
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

#[cfg(target_os = "linux")]
fn canonicalize_with_missing_suffix(path: &std::path::Path) -> Result<PathBuf, String> {
    let mut cursor = path.to_path_buf();
    let mut missing_suffix = Vec::new();
    let mut followed_symlinks = BTreeSet::new();
    loop {
        match std::fs::canonicalize(&cursor) {
            Ok(mut resolved) => {
                for component in missing_suffix.iter().rev() {
                    resolved.push(component);
                }
                return Ok(lexical_normalize_path(&resolved));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::symlink_metadata(&cursor) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        let link = lexical_normalize_path(&cursor);
                        if !followed_symlinks.insert(link.clone()) {
                            return Err(format!(
                                "cannot resolve cyclic symlink in path identity for {}",
                                path.display()
                            ));
                        }
                        let target = std::fs::read_link(&cursor).map_err(|error| {
                            format!("cannot resolve symlink {}: {error}", cursor.display())
                        })?;
                        cursor = if target.is_absolute() {
                            target
                        } else {
                            cursor
                                .parent()
                                .ok_or_else(|| {
                                    format!("cannot resolve symlink {}", cursor.display())
                                })?
                                .join(target)
                        };
                        cursor = lexical_normalize_path(&cursor);
                        continue;
                    }
                    Ok(_) => {}
                    Err(metadata_error)
                        if metadata_error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(metadata_error) => {
                        return Err(format!(
                            "cannot inspect path identity for {}: {metadata_error}",
                            cursor.display()
                        ));
                    }
                }
                let component = cursor.file_name().ok_or_else(|| {
                    format!("cannot resolve path identity for {}", path.display())
                })?;
                missing_suffix.push(component.to_os_string());
                cursor = cursor
                    .parent()
                    .ok_or_else(|| format!("cannot resolve path identity for {}", path.display()))?
                    .to_path_buf();
            }
            Err(error) => {
                return Err(format!(
                    "cannot resolve path identity for {}: {error}",
                    path.display()
                ));
            }
        }
    }
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
            "       agenturm query --db <path> [--harness-id <id>] [--session-id <id>] [--limit <1..16384>] [--snapshot-id <id>] [--before-timestamp-ns <ns> --before-harness-id <id> --before-event-id <id> --before-revision <n>]\n",
            "       agenturm collect --config <json-path> --db <path>\n",
            "       agenturm collect --config <json-path> --db <path> --watch [--interval-ms <100..60000>]\n",
            "       agenturm ingest-usage --plugin-id <id> --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>\n",
            "       agenturm ingest-codex-usage --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>\n",
            "\n",
            "This explicit projection stores token counters and skips other supported facts.\n",
            "Use `agenturm inspect --db <path>` to view committed history in the full-screen interface.\n",
            "Collection config is versioned JSON; relative executable and source paths resolve beside the config.\n",
            "On Linux, install the managed foreground collector with `agenturm service install`.\n",
        )
        .into(),
        Command::Version => format!("agenturm {}\n", env!("CARGO_PKG_VERSION")),
        Command::Status => format!(
            concat!(
                "Agenturm Rust kernel: ready\n",
                "Registered harnesses: 0\n",
                "Registered capabilities: 0\n",
                "Registered UI contributions: 0\n",
                "Production storage service: not connected (no separate storage identity)\n",
                "Registered-plugin usage projection: available\n",
                "Configured multi-source collection: one-shot and foreground watch available\n",
                "{}"
            ),
            SERVICE_STATUS_LINE
        ),
        Command::Inspect(_) => String::new(),
        Command::Query(_) => String::new(),
        Command::Ingest(_) => String::new(),
        Command::Collect(_) => String::new(),
        Command::Service(_) => String::new(),
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
            parse_command(&[
                "service".into(),
                "install".into(),
                "--config".into(),
                "sources.json".into(),
                "--db".into(),
                "state.sqlite".into(),
            ])
            .unwrap(),
            Command::Service(ServiceCommand::Install {
                config: PathBuf::from("sources.json"),
                db: PathBuf::from("state.sqlite"),
                interval_ms: 2_000,
            })
        );
        assert_eq!(
            parse_command(&[
                "service".into(),
                "install".into(),
                "--config".into(),
                "sources.json".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--interval-ms".into(),
                "500".into(),
            ])
            .unwrap(),
            Command::Service(ServiceCommand::Install {
                config: PathBuf::from("sources.json"),
                db: PathBuf::from("state.sqlite"),
                interval_ms: 500,
            })
        );
        assert!(
            parse_command(&[
                "service".into(),
                "install".into(),
                "--config".into(),
                "sources.json".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--interval-ms".into(),
                "99".into(),
            ])
            .is_err()
        );
        assert_eq!(
            parse_command(&["inspect".into(), "--db".into(), "./state.sqlite".into()]).unwrap(),
            Command::Inspect(PathBuf::from("./state.sqlite"))
        );
        assert_eq!(
            parse_command(&["query".into(), "--db".into(), "./state.sqlite".into()]).unwrap(),
            Command::Query(HistoryQueryOptions {
                db: PathBuf::from("./state.sqlite"),
                harness_id: None,
                session_id: None,
                before: None,
                snapshot_id: None,
                limit: 500,
            })
        );
        assert_eq!(
            parse_command(&[
                "query".into(),
                "--db".into(),
                "./state.sqlite".into(),
                "--snapshot-id".into(),
                "12".into(),
            ])
            .unwrap(),
            Command::Query(HistoryQueryOptions {
                db: PathBuf::from("./state.sqlite"),
                harness_id: None,
                session_id: None,
                before: None,
                snapshot_id: Some(12),
                limit: 500,
            })
        );
        assert_eq!(
            parse_command(&[
                "query".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--harness-id".into(),
                "harness.codex".into(),
                "--session-id".into(),
                "session-1".into(),
                "--limit".into(),
                "10".into(),
                "--before-timestamp-ns".into(),
                "42".into(),
                "--before-harness-id".into(),
                "harness.codex".into(),
                "--before-event-id".into(),
                "event-1".into(),
                "--before-revision".into(),
                "0".into(),
                "--snapshot-id".into(),
                "1".into(),
            ])
            .unwrap(),
            Command::Query(HistoryQueryOptions {
                db: PathBuf::from("state.sqlite"),
                harness_id: Some("harness.codex".into()),
                session_id: Some("session-1".into()),
                before: Some(ObservationPosition {
                    observed_at_unix_ns: 42,
                    harness_id: "harness.codex".into(),
                    event_id: "event-1".into(),
                    revision: 0,
                    snapshot_id: 1,
                }),
                snapshot_id: Some(1),
                limit: 10,
            })
        );
        assert!(
            parse_command(&[
                "query".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--before-timestamp-ns".into(),
                "42".into(),
            ])
            .is_err()
        );
        assert!(
            parse_command(&[
                "query".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--limit".into(),
                "16385".into(),
            ])
            .is_err()
        );
        assert!(
            parse_command(&[
                "query".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--session-id".into(),
                "session 1".into(),
            ])
            .is_ok()
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
                watch_interval: None,
            })
        );
        assert!(
            parse_command(&["collect".into(), "--config".into(), "sources.json".into()]).is_err()
        );
        assert_eq!(
            parse_command(&[
                "collect".into(),
                "--watch".into(),
                "--interval-ms".into(),
                "500".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--config".into(),
                "sources.json".into(),
            ])
            .unwrap(),
            Command::Collect(ConfiguredCollectOptions {
                config: PathBuf::from("sources.json"),
                db: PathBuf::from("state.sqlite"),
                watch_interval: Some(Duration::from_millis(500)),
            })
        );
        assert!(
            parse_command(&[
                "collect".into(),
                "--config".into(),
                "sources.json".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--interval-ms".into(),
                "500".into(),
            ])
            .is_err()
        );
        assert!(
            parse_command(&[
                "collect".into(),
                "--config".into(),
                "sources.json".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--watch".into(),
                "--interval-ms".into(),
                "99".into(),
            ])
            .is_err()
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
    fn history_query_reopens_store_and_returns_a_reusable_older_cursor() {
        use agenturm_store::{
            ObservationBatch, ObservationIdentity as StoredIdentity, ObservationKind as StoredKind,
            StoredObservation,
        };
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agenturm-query-app-{}-{id}.sqlite",
            std::process::id()
        ));
        let mut store = Store::open(&path).unwrap();
        let make_event = |event_id: &str, timestamp| StoredObservation {
            event_id: event_id.into(),
            revision: 0,
            identity: StoredIdentity {
                session_id: "session-a".into(),
                worker_id: None,
                turn_id: None,
                operation_id: None,
            },
            observed_at_unix_ns: timestamp,
            kind: StoredKind::Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                cache_read_tokens: None,
                cache_write_tokens: None,
                total_tokens: Some(15),
            },
        };
        store
            .commit_batch(&ObservationBatch {
                request_id: "query-app-request".into(),
                source_id: "harness.codex".into(),
                expected_cursor: None,
                next_cursor: "cursor-1".into(),
                observations: vec![
                    make_event("event-1", 10),
                    make_event("event-2", 20),
                    make_event("event-3", 30),
                ],
            })
            .unwrap();
        drop(store);

        let first = execute_query(&HistoryQueryOptions {
            db: path.clone(),
            harness_id: Some("harness.codex".into()),
            session_id: Some("session-a".into()),
            before: None,
            snapshot_id: None,
            limit: 2,
        })
        .unwrap();
        let first: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(first["has_more"], true);
        assert_eq!(first["observations"][0]["event_id"], "event-2");
        assert_eq!(first["observations"][1]["event_id"], "event-3");

        let cursor = &first["next_before"];
        let older = execute_query(&HistoryQueryOptions {
            db: path.clone(),
            harness_id: Some("harness.codex".into()),
            session_id: Some("session-a".into()),
            before: Some(ObservationPosition {
                observed_at_unix_ns: cursor["observed_at_unix_ns"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
                harness_id: cursor["harness_id"].as_str().unwrap().into(),
                event_id: cursor["event_id"].as_str().unwrap().into(),
                revision: cursor["revision"].as_u64().unwrap() as u32,
                snapshot_id: cursor["snapshot_id"].as_str().unwrap().parse().unwrap(),
            }),
            snapshot_id: Some(cursor["snapshot_id"].as_str().unwrap().parse().unwrap()),
            limit: 2,
        })
        .unwrap();
        let older: serde_json::Value = serde_json::from_str(&older).unwrap();
        assert_eq!(older["has_more"], false);
        assert!(older["next_before"].is_null());
        assert_eq!(older["observations"][0]["event_id"], "event-1");
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn history_query_does_not_create_a_missing_database() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agenturm-query-missing-{}-{nonce}.sqlite",
            std::process::id()
        ));
        assert!(!path.exists());
        let result = execute_query(&HistoryQueryOptions {
            db: path.clone(),
            harness_id: None,
            session_id: None,
            before: None,
            snapshot_id: None,
            limit: 1,
        });
        assert!(result.is_err());
        assert!(!path.exists());

        std::fs::write(&path, []).unwrap();
        let result = execute_query(&HistoryQueryOptions {
            db: path.clone(),
            harness_id: None,
            session_id: None,
            before: None,
            snapshot_id: None,
            limit: 1,
        });
        assert!(result.is_err());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        for suffix in ["-wal", "-shm"] {
            assert!(!std::path::PathBuf::from(format!("{}{suffix}", path.display())).exists());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn status_output_does_not_claim_unwired_components() {
        let status = render(Command::Status);
        assert!(status.contains("Registered harnesses: 0"));
        assert!(status.contains("Production storage service: not connected"));
        assert!(status.contains("Registered-plugin usage projection: available"));
        assert!(status.contains(
            "Configured multi-source collection: one-shot and foreground watch available"
        ));
        assert!(
            status
                .contains("Managed user service: Linux systemd install/status/uninstall available")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn service_database_overlap_resolves_symlinks_before_missing_source_suffixes() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "agenturm-service-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let database_directory = root.join("database");
        std::fs::create_dir_all(&database_directory).unwrap();
        let source_alias = root.join("source-alias");
        symlink(&database_directory, &source_alias).unwrap();
        let source = ConfiguredSource {
            plugin_id: "example.rust-pi-harness".into(),
            adapter: PathBuf::from("/bin/true"),
            source_root: source_alias.join("new-source-root"),
            source_id: "harness.pi".into(),
        };
        assert!(
            validate_service_database_isolation(
                &database_directory.join("agenturm.sqlite"),
                std::slice::from_ref(&source)
            )
            .is_err()
        );
        let dangling_alias = root.join("dangling-source-alias");
        symlink(
            database_directory.join("future-source-root"),
            &dangling_alias,
        )
        .unwrap();
        let dangling_source = ConfiguredSource {
            source_root: dangling_alias,
            ..source
        };
        assert!(
            validate_service_database_isolation(
                &database_directory.join("agenturm.sqlite"),
                &[dangling_source]
            )
            .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn global_store_failures_make_collection_report_fail() {
        let mut snapshot = Snapshot::default();
        snapshot.failures.push(agenturm_kernel::ExtensionFailure {
            extension_id: ExtensionId::parse("kernel.store").unwrap(),
            stage: agenturm_kernel::ExtensionStage::AdmissionRead,
            message: "injected durable history read failure".into(),
        });
        let report = build_collection_report(&[], &BTreeMap::new(), &snapshot, false);
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
            false,
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
    fn shutdown_cancellation_is_reported_without_becoming_a_source_failure() {
        let source_id = ExtensionId::parse("harness.cancelled").unwrap();
        let mut snapshot = Snapshot::default();
        snapshot.failures.push(agenturm_kernel::ExtensionFailure {
            extension_id: source_id.clone(),
            stage: agenturm_kernel::ExtensionStage::AdmissionPoll,
            message: "source operation cancelled".into(),
        });
        let report = build_collection_report(&[source_id], &BTreeMap::new(), &snapshot, true);
        assert!(report.summary.contains("poll cancelled by shutdown"));
        assert_eq!(report.failed_sources, 0);
        assert!(!report.has_failures());
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
