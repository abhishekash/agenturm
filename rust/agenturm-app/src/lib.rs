//! Rust application composition boundary.
//!
//! The app exposes generic kernel extension registration, durable refresh,
//! explicit one-shot and configured multi-source usage collection, and a
//! full-screen durable-history viewer and Linux user service management.

#[cfg(target_os = "linux")]
mod adapter_install;
mod attention;
#[cfg(target_os = "linux")]
mod broker_cli;
mod cockpit_host;
mod cockpit_startup;
mod cockpit_workflows;
mod content_preview;
mod discovery;
mod eval_import;
mod handoff_export;
mod inspector_activity;
mod investigate_cli;
#[cfg(target_os = "linux")]
mod new_window;
mod private_output;
#[cfg(target_os = "linux")]
mod remote_history;
mod retention_cli;
mod review_receipt;
mod session_evidence;
mod span_jsonl_import;
mod summary_cli;
mod task_cli;
mod task_git;
mod telemetry_payload_cli;
mod trace_import;
mod trace_receiver;
mod trace_run;
mod workflow_route;

#[cfg(target_os = "linux")]
pub use broker_cli::{BrokerExecutionError, execute as execute_broker};

#[cfg(target_os = "linux")]
pub use adapter_install::execute as execute_adapter_install;

pub use attention::{AttentionOptions, execute_attention};
pub use content_preview::{ContentOptions, execute_content, execute_content_stdout};
pub use discovery::{discover, execute_discover};
pub use eval_import::{EvalOptions, execute_evals};
pub use investigate_cli::{InvestigationOptions, execute_investigation};
#[cfg(target_os = "linux")]
pub use new_window::{WindowOptions, execute as execute_new_window};
pub use private_output::with_private_stdout;
#[cfg(target_os = "linux")]
pub use remote_history::{RemoteHistoryOptions, execute_remote_cockpit};
pub use retention_cli::{RetentionOptions, execute_retention};
pub use review_receipt::ReviewOutput;
pub use span_jsonl_import::{SpanJsonlImportOptions, execute_span_jsonl_import};
pub use summary_cli::{SummaryOptions, execute_summary};
pub use task_cli::{TaskOptions, execute_task, execute_task_stdout};
pub use telemetry_payload_cli::{PayloadCommand, execute_payload_stdout};
pub use trace_import::{TraceImportOptions, execute_trace_import};
pub use trace_receiver::{TraceReceiverOptions, execute_trace_receiver};
pub use trace_run::{TraceRunOptions, execute_trace_run};

use agenturm_kernel::ReadSnapshot;
use agenturm_kernel::{
    Capability, ExtensionId, ExtensionPlugin, Harness, Kernel, MAX_DURABLE_SNAPSHOT_OBSERVATIONS,
    MetricRow, Observation, ObservationExtension, ObservationKind, ObservationPosition,
    ObservationQuery, ObservationStore, PaneModel, RegistrationError, Snapshot, SourceAdapter,
    UiContribution,
};
use agenturm_runtime::{
    CODEX_PLUGIN_ID, ExtensionDataGrant, ObservationFormat, ProcessConfig,
    ProcessObservationExtension, ProcessObservationExtensionConfig, ProcessSource,
    verify_extension_artifact,
};
#[cfg(target_os = "linux")]
use agenturm_service::{
    ServiceSpec, Systemctl, install_user_service, status_user_service, uninstall_user_service,
};
use agenturm_store::{
    ResolvedTraceReview, ReviewKey, ReviewLedger, ReviewVersion as StoredReviewVersion,
    SessionPosition, SessionQuery, Store, TraceReviewQuery,
};
use agenturm_ui::{
    HistorySelection, RecordedSession, ReviewVersion, SessionCatalog, SessionCursor, SessionView,
    WorkspaceRequest, WorkspaceResponse, WorkspaceView,
};
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
const MAX_PLUGIN_CATALOG_BYTES: u64 = 64 * 1024;
const MAX_PROCESS_EXTENSION_CATALOG_ENTRIES: usize = 64;
const MAX_ENABLED_PROCESS_EXTENSIONS: usize = 4;
const MAX_PROCESS_EXTENSION_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CONFIGURED_SOURCES: usize = 16;
const FOLLOW_INTERVAL: Duration = Duration::from_secs(5);
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

    pub fn register_observation_extension(
        &mut self,
        id: ExtensionId,
        extension: impl ObservationExtension + 'static,
    ) -> Result<(), RegistrationError> {
        self.kernel.register_observation_extension(id, extension)
    }

    pub fn register_ui_contribution(
        &mut self,
        id: ExtensionId,
        contribution: impl UiContribution + 'static,
    ) -> Result<(), RegistrationError> {
        self.kernel.register_ui_contribution(id, contribution)
    }

    /// Installs a build-time extension package atomically. The package's
    /// contribution code remains trusted in-process Rust code.
    pub fn install_plugin(
        &mut self,
        plugin: impl ExtensionPlugin + 'static,
    ) -> Result<(), RegistrationError> {
        self.kernel.register_plugin(plugin)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Help,
    Version,
    Status,
    Attention(AttentionOptions),
    Task(TaskOptions),
    Inspect(InspectOptions),
    Cockpit {
        db: Option<PathBuf>,
        extensions: ResolvedExtensionCatalog,
    },
    #[cfg(target_os = "linux")]
    Window(WindowOptions),
    #[cfg(target_os = "linux")]
    RemoteCockpit(RemoteHistoryOptions),
    Query(HistoryQueryOptions),
    #[cfg(target_os = "linux")]
    Broker(broker_cli::BrokerOptions),
    #[cfg(target_os = "linux")]
    InstallAdapter(adapter_install::Options),
    Review {
        query: HistoryQueryOptions,
        selection: ReviewSelection,
        output: ReviewOutput,
    },
    Investigate(InvestigationOptions),
    Backup {
        db: PathBuf,
        output: PathBuf,
    },
    BackupWorkspace {
        db: PathBuf,
        output: PathBuf,
    },
    RestoreWorkspace {
        bundle: PathBuf,
        output: PathBuf,
    },
    Prune {
        db: PathBuf,
        before_unix_ns: u64,
    },
    Content(ContentOptions),
    TracePayload(PayloadCommand),
    Retention(RetentionOptions),
    Summary(SummaryOptions),
    Ingest(IngestOptions),
    ImportTraces(TraceImportOptions),
    ImportJsonl(SpanJsonlImportOptions),
    Evals(EvalOptions),
    ServeTraces(TraceReceiverOptions),
    Run(TraceRunOptions),
    Collect(ConfiguredCollectOptions),
    Discover {
        sessions_per_harness: usize,
        follow: bool,
    },
    Service(ServiceCommand),
}

/// Read-only inspector options, including explicitly selected process
/// extensions resolved from a user-managed catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectOptions {
    pub db: PathBuf,
    pub extensions: ResolvedExtensionCatalog,
}

/// Selected extension entries with canonical executable paths. The runtime
/// owns SHA-256 verification and process startup.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolvedExtensionCatalog {
    pub extensions: Vec<ResolvedProcessExtension>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedProcessExtension {
    pub plugin_id: ExtensionId,
    pub executable: PathBuf,
    pub sha256: String,
    pub protocol: u32,
    pub features: BTreeSet<String>,
    pub grants: BTreeSet<String>,
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
    #[serde(default)]
    plugin_catalog: Option<PathBuf>,
    #[serde(default)]
    sources: Vec<CollectionSourceInput>,
    #[serde(default)]
    follow: Option<discovery::FollowDiscovery>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct CollectionSourceInput {
    plugin_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adapter: Option<PathBuf>,
    source_root: PathBuf,
    source_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterCatalogFile {
    version: u32,
    plugins: Vec<AdapterCatalogEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterCatalogEntry {
    plugin_id: String,
    adapter: PathBuf,
    sha256: String,
    protocol: u32,
    features: Vec<String>,
    #[serde(default = "default_observation_format")]
    observation_format: String,
}

#[derive(Debug)]
struct ValidatedAdapter {
    adapter: PathBuf,
    sha256: String,
    features: Vec<String>,
    observation_format: ObservationFormat,
}

fn default_observation_format() -> String {
    ObservationFormat::default().to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessExtensionCatalogFile {
    version: u32,
    extensions: Vec<ProcessExtensionCatalogEntry>,
    #[serde(default)]
    enabled: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessExtensionCatalogEntry {
    plugin_id: String,
    executable: PathBuf,
    sha256: String,
    protocol: u32,
    features: Vec<String>,
    grants: Vec<String>,
}

#[derive(Clone, Debug)]
struct ConfiguredSource {
    plugin_id: String,
    adapter: PathBuf,
    source_root: PathBuf,
    source_id: String,
    artifact_sha256: Option<String>,
    features: Option<Vec<String>>,
    observation_format: ObservationFormat,
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
    pub snapshot_id: Option<ReadSnapshot>,
    pub limit: usize,
}

impl HistoryQueryOptions {
    fn observation_query(&self) -> ObservationQuery {
        ObservationQuery {
            harness_id: self.harness_id.clone(),
            session_id: self.session_id.clone(),
            before: self.before.clone(),
            snapshot_id: self.snapshot_id,
            limit: self.limit,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewSelection {
    anchor: ReviewAnchor,
    linked_traces: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ReviewAnchor {
    Turn {
        turn_id: String,
        worker_id: Option<String>,
    },
    Trace {
        trace_id: String,
        trace_source_id: String,
        expected_worker_id: Option<String>,
    },
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
        .filter_map(|row| {
            row.identity
                .session_id
                .as_deref()
                .map(|session| (row.harness_id.as_str(), session))
        })
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
        .filter(|row| {
            matches!(
                row.kind,
                ObservationKind::Usage { .. } | ObservationKind::CreditUsage { .. }
            )
        })
        .count();
    let tools = observations
        .iter()
        .filter(|row| matches!(row.kind, ObservationKind::Tool { .. }))
        .count();
    let lifecycle = observations
        .iter()
        .filter(|row| matches!(row.kind, ObservationKind::Lifecycle { .. }))
        .count();
    let checks = observations
        .iter()
        .filter(|row| matches!(row.kind, ObservationKind::Check { .. }))
        .count();
    let messages = observations
        .iter()
        .filter(|row| matches!(row.kind, ObservationKind::Message { .. }))
        .count();
    let diagnostics = observations
        .iter()
        .filter(|row| matches!(row.kind, ObservationKind::Diagnostic { .. }))
        .count();
    let lifecycle_evidence = observations.iter().filter_map(|row| match &row.kind {
        ObservationKind::Lifecycle { state, reason_code } => {
            Some((row, state.as_str(), reason_code.as_deref()))
        }
        _ => None,
    });
    let latest_lifecycle = lifecycle_evidence
        .clone()
        .max_by_key(|(row, ..)| evidence_order(row));
    let latest_diagnostic = observations
        .iter()
        .filter_map(|row| match &row.kind {
            ObservationKind::Diagnostic { code, severity } => Some((row, code, severity)),
            _ => None,
        })
        .max_by_key(|(row, ..)| evidence_order(row))
        .map(|(row, code, severity)| {
            let scope = match row.identity.scope {
                agenturm_kernel::ObservationScope::Source => "source".to_owned(),
                agenturm_kernel::ObservationScope::Session => format!(
                    "session {}",
                    row.identity.session_id.as_deref().unwrap_or("unknown")
                ),
            };
            format!(
                "{code} · {severity} · {} · {scope}",
                row.harness_id.as_str()
            )
        })
        .unwrap_or_else(|| "none recorded".into());
    let mut rows = vec![
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
        MetricRow {
            label: "Tool observations".into(),
            value: tools.to_string(),
        },
        MetricRow {
            label: "Lifecycle observations".into(),
            value: lifecycle.to_string(),
        },
        MetricRow {
            label: "Check observations".into(),
            value: checks.to_string(),
        },
        MetricRow {
            label: "Message role observations".into(),
            value: messages.to_string(),
        },
        MetricRow {
            label: "Diagnostic observations".into(),
            value: diagnostics.to_string(),
        },
    ];
    if let Some((row, state, reason)) = latest_lifecycle {
        rows.extend(lifecycle_evidence_rows(
            "Latest lifecycle evidence",
            "Lifecycle turn",
            row,
            state,
            reason,
        ));
        if !is_recorded_turn_end(row, state)
            && let Some((row, state, reason)) = lifecycle_evidence
                .filter(|(row, state, _)| is_recorded_turn_end(row, state))
                .max_by_key(|(row, ..)| evidence_order(row))
        {
            rows.extend(lifecycle_evidence_rows(
                "Latest recorded turn end",
                "Ended turn",
                row,
                state,
                reason,
            ));
        }
    } else {
        rows.push(MetricRow {
            label: "Latest lifecycle evidence".into(),
            value: "none recorded".into(),
        });
    }
    if let Some((row, repository, number)) = observations
        .iter()
        .filter_map(|row| match &row.kind {
            ObservationKind::PullRequestRaised { repository, number } => {
                Some((row, repository, number))
            }
            _ => None,
        })
        .max_by_key(|(row, ..)| evidence_order(row))
    {
        rows.push(MetricRow {
            label: "Latest raised PR".into(),
            value: format!(
                "{repository}#{number} · {} · turn {}",
                row.harness_id.as_str(),
                row.identity.turn_id.as_deref().unwrap_or("not recorded")
            ),
        });
    }
    if let Some((row, relationship_kind, parent_session_id)) = observations
        .iter()
        .filter_map(|row| match &row.kind {
            ObservationKind::Relationship {
                relationship_kind,
                parent_session_id,
            } => Some((row, relationship_kind, parent_session_id)),
            _ => None,
        })
        .max_by_key(|(row, ..)| evidence_order(row))
    {
        rows.extend([
            MetricRow {
                label: "Latest session relationship".into(),
                value: format!(
                    "{} · {} · child {}",
                    relationship_kind.as_str(),
                    row.harness_id.as_str(),
                    row.identity.session_id.as_deref().unwrap_or("unknown"),
                ),
            },
            MetricRow {
                label: "Parent session".into(),
                value: parent_session_id.clone(),
            },
        ]);
    }
    if let Some((row, nanocredits)) = observations
        .iter()
        .filter_map(|row| match row.kind {
            ObservationKind::CreditUsage { nanocredits } => Some((row, nanocredits)),
            _ => None,
        })
        .max_by_key(|(row, ..)| evidence_order(row))
    {
        rows.push(MetricRow {
            label: "Latest credit usage".into(),
            value: format!(
                "{} credits · {} · session {}",
                agenturm_kernel::format_credits(nanocredits),
                row.harness_id.as_str(),
                row.identity.session_id.as_deref().unwrap_or("unknown"),
            ),
        });
    }
    rows.push(MetricRow {
        label: "Latest diagnostic".into(),
        value: latest_diagnostic,
    });
    rows
}

fn evidence_order(row: &Observation) -> (u64, &str, &str, u32) {
    (
        row.observed_at_unix_ns,
        row.harness_id.as_str(),
        row.event_id.as_str(),
        row.revision,
    )
}

fn is_recorded_turn_end(row: &Observation, state: &str) -> bool {
    row.identity.turn_id.is_some() && matches!(state, "completed" | "interrupted" | "failed")
}

fn lifecycle_evidence_rows(
    label: &str,
    turn_label: &str,
    row: &Observation,
    state: &str,
    reason: Option<&str>,
) -> [MetricRow; 2] {
    [
        MetricRow {
            label: label.into(),
            value: format!(
                "reason={} · state={} · {}/{}",
                reason.unwrap_or("not-recorded"),
                state,
                row.harness_id.as_str(),
                row.identity.session_id.as_deref().unwrap_or("source")
            ),
        },
        MetricRow {
            label: turn_label.into(),
            value: row
                .identity
                .turn_id
                .as_deref()
                .unwrap_or("not recorded")
                .to_owned(),
        },
    ]
}

pub fn parse_os_command(args: &[std::ffi::OsString]) -> Result<Command, String> {
    #[cfg(target_os = "linux")]
    if new_window::handles(args) {
        return new_window::parse(args).map(Command::Window);
    }
    if args.first().is_some_and(|arg| arg == "run") {
        return trace_run::parse_trace_run(&args[1..]).map(Command::Run);
    }
    #[cfg(target_os = "linux")]
    if args.first().is_some_and(|arg| arg == "broker") {
        return broker_cli::parse_os(&args[1..]).map(Command::Broker);
    }
    let args = args
        .iter()
        .map(|arg| {
            arg.to_str()
                .map(str::to_owned)
                .ok_or("Agenturm options must be UTF-8")
        })
        .collect::<Result<Vec<_>, _>>()?;
    parse_command(&args)
}

pub fn parse_command(args: &[String]) -> Result<Command, String> {
    #[cfg(target_os = "linux")]
    if new_window::handles(args) {
        let original = args
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        return new_window::parse(&original).map(Command::Window);
    }
    if args.first().is_some_and(|arg| arg == "--once") {
        let mut explicit = vec!["cockpit".to_owned()];
        explicit.extend_from_slice(args);
        return parse_command(&explicit);
    }
    if args.first().is_some_and(|arg| arg == "cockpit")
        && args[1..].iter().any(|arg| arg == "--once")
    {
        return summary_cli::parse(&args[1..]).map(Command::Summary);
    }
    match args.first().map(String::as_str) {
        None => Ok(Command::Help),
        Some("--help" | "-h" | "help") if args.len() == 1 => Ok(Command::Help),
        Some("--version" | "-V" | "version") if args.len() == 1 => Ok(Command::Version),
        Some("status") if args.len() == 1 => Ok(Command::Status),
        #[cfg(target_os = "linux")]
        Some("broker") => broker_cli::parse(&args[1..]).map(Command::Broker),
        #[cfg(target_os = "linux")]
        Some("adapter") => adapter_install::parse(&args[1..]).map(Command::InstallAdapter),
        Some("attention") => attention::parse(&args[1..]).map(Command::Attention),
        Some("task") => task_cli::parse(&args[1..]).map(Command::Task),
        Some("cockpit") if args.len() == 1 => Ok(Command::Cockpit {
            db: None,
            extensions: ResolvedExtensionCatalog::default(),
        }),
        #[cfg(target_os = "linux")]
        Some("cockpit")
            if args[1..]
                .iter()
                .any(|arg| matches!(arg.as_str(), "--endpoint" | "--token-file")) =>
        {
            remote_history::parse_attach(&args[1..]).map(Command::RemoteCockpit)
        }
        Some("cockpit") => parse_inspect_options(&args[1..]).map(|options| Command::Cockpit {
            db: Some(options.db),
            extensions: options.extensions,
        }),
        Some("inspect") => parse_inspect_options(&args[1..]).map(Command::Inspect),
        Some("query") => parse_query_options(&args[1..], false),
        Some("review") => parse_query_options(&args[1..], true),
        Some("investigate") => investigate_cli::parse(&args[1..], false).map(Command::Investigate),
        Some("backup") => {
            if let Ok((db, output)) = parse_db_and_option(&args[1..], "--bundle") {
                return Ok(Command::BackupWorkspace {
                    db,
                    output: output.into(),
                });
            }
            let (db, output) = parse_db_and_option(&args[1..], "--output")?;
            Ok(Command::Backup {
                db,
                output: output.into(),
            })
        }
        Some("restore") => {
            let (bundle, output) = parse_two_options(&args[1..], "--bundle", "--output")?;
            Ok(Command::RestoreWorkspace {
                bundle: bundle.into(),
                output: output.into(),
            })
        }
        Some("prune") => {
            let (db, cutoff) = parse_db_and_option(&args[1..], "--before-unix-ns")?;
            let before_unix_ns = cutoff
                .parse::<u64>()
                .map_err(|_| "--before-unix-ns requires a Unix nanosecond timestamp".to_owned())?;
            Ok(Command::Prune { db, before_unix_ns })
        }
        Some("retention") => retention_cli::parse(&args[1..], false).map(Command::Retention),
        Some("content") => content_preview::parse(&args[1..]).map(Command::Content),
        Some("trace-payload") => {
            telemetry_payload_cli::parse(&args[1..]).map(Command::TracePayload)
        }
        Some("discover") => parse_discover_options(&args[1..]),
        Some("collect") => parse_collect_options(&args[1..]).map(Command::Collect),
        Some("service") => parse_service_options(&args[1..]).map(Command::Service),
        Some("ingest-usage") => parse_ingest_options(&args[1..], None).map(Command::Ingest),
        Some("serve-traces") => trace_receiver::parse(&args[1..]).map(Command::ServeTraces),
        Some("run") => trace_run::parse_trace_run(
            &args[1..]
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>(),
        )
        .map(Command::Run),
        Some("import-traces") => trace_import::parse(&args[1..]).map(Command::ImportTraces),
        Some("import-jsonl") => span_jsonl_import::parse(&args[1..]).map(Command::ImportJsonl),
        Some("import-evals") => eval_import::parse(&args[1..], true).map(Command::Evals),
        Some("evals") => eval_import::parse(&args[1..], false).map(Command::Evals),
        Some("ingest-codex-usage") => {
            parse_ingest_options(&args[1..], Some(CODEX_PLUGIN_ID)).map(Command::Ingest)
        }
        _ => Err(usage()),
    }
}

fn parse_discover_options(mut args: &[String]) -> Result<Command, String> {
    let mut follow = false;
    let mut sessions_per_harness = None;
    while let Some((flag, rest)) = args.split_first() {
        args = rest;
        match flag.as_str() {
            "--follow" if !follow => follow = true,
            "--sessions-per-harness" if sessions_per_harness.is_none() => {
                let (limit, rest) = args.split_first().ok_or_else(usage)?;
                sessions_per_harness = Some(
                    limit
                        .parse::<usize>()
                        .ok()
                        .filter(|limit| (1..=discovery::MAX_SESSIONS_PER_HARNESS).contains(limit))
                        .ok_or_else(usage)?,
                );
                args = rest;
            }
            _ => return Err(usage()),
        }
    }
    Ok(Command::Discover {
        sessions_per_harness: sessions_per_harness
            .unwrap_or(discovery::DEFAULT_SESSIONS_PER_HARNESS),
        follow,
    })
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
            if let Some(follow) = &validated.follow {
                validate_source_root_isolation(&database, follow.selection.roots())?;
            }
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
    validate_source_root_isolation(
        database,
        sources.iter().map(|source| source.source_root.as_path()),
    )
}

#[cfg(target_os = "linux")]
fn validate_source_root_isolation<'a>(
    database: &std::path::Path,
    roots: impl IntoIterator<Item = &'a std::path::Path>,
) -> Result<(), String> {
    let database_directory = database
        .parent()
        .ok_or_else(|| "database path must have a parent directory".to_owned())?;
    let database_directory = canonicalize_with_missing_suffix(database_directory)?;
    for root in roots {
        let source_root = canonicalize_with_missing_suffix(root)?;
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

fn parse_inspect_options(args: &[String]) -> Result<InspectOptions, String> {
    let mut db = None;
    let mut extension_catalog = None;
    let mut selected_extensions = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let key = args[index].as_str();
        match key {
            "--db" | "--extension-catalog" | "--extension" => {
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .ok_or_else(usage)?;
                match key {
                    "--db" => {
                        if db.replace(PathBuf::from(value)).is_some() {
                            return Err("duplicate option --db\n".to_owned() + &usage());
                        }
                    }
                    "--extension-catalog" => {
                        if extension_catalog.replace(PathBuf::from(value)).is_some() {
                            return Err(
                                "duplicate option --extension-catalog\n".to_owned() + &usage()
                            );
                        }
                    }
                    "--extension" => {
                        if selected_extensions.len() == MAX_ENABLED_PROCESS_EXTENSIONS {
                            return Err(format!(
                                "at most {MAX_ENABLED_PROCESS_EXTENSIONS} extensions may be selected"
                            ));
                        }
                        selected_extensions.push(value.clone());
                    }
                    _ => unreachable!(),
                }
                index += 2;
            }
            _ => return Err(usage()),
        }
    }

    let extensions = match (extension_catalog, selected_extensions.is_empty()) {
        (None, true) => ResolvedExtensionCatalog::default(),
        (None, false) => {
            return Err("--extension requires --extension-catalog".into());
        }
        (Some(_), true) => {
            return Err("--extension-catalog requires at least one --extension".into());
        }
        (Some(catalog), false) => load_process_extension_catalog(&catalog, &selected_extensions)?,
    };
    Ok(InspectOptions {
        db: db.ok_or_else(usage)?,
        extensions,
    })
}

fn load_process_extension_catalog(
    path: &std::path::Path,
    selected_ids: &[String],
) -> Result<ResolvedExtensionCatalog, String> {
    load_process_extension_selection(path, Some(selected_ids))
}

fn load_process_extension_selection(
    path: &std::path::Path,
    explicit_ids: Option<&[String]>,
) -> Result<ResolvedExtensionCatalog, String> {
    let catalog_path = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(error) if explicit_ids.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ResolvedExtensionCatalog::default());
        }
        Err(_) => return Err("cannot resolve extension catalog path".into()),
    };
    let metadata = std::fs::metadata(&catalog_path)
        .map_err(|_| "cannot inspect extension catalog".to_owned())?;
    if !metadata.is_file() {
        return Err("extension catalog must be a regular file".into());
    }
    let file = File::open(&catalog_path).map_err(|_| "cannot open extension catalog".to_owned())?;
    let mut bytes = Vec::new();
    file.take(MAX_PLUGIN_CATALOG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read extension catalog".to_owned())?;
    if bytes.len() as u64 > MAX_PLUGIN_CATALOG_BYTES {
        return Err("extension catalog exceeds the 64 KiB limit".into());
    }
    let catalog: ProcessExtensionCatalogFile = serde_json::from_slice(&bytes)
        .map_err(|_| "extension catalog is invalid JSON".to_owned())?;
    let selected_ids = explicit_ids.unwrap_or(&catalog.enabled);
    if catalog.version != 1
        || (!selected_ids.is_empty() && catalog.extensions.is_empty())
        || catalog.extensions.len() > MAX_PROCESS_EXTENSION_CATALOG_ENTRIES
    {
        return Err("unsupported or invalid extension catalog size/version".into());
    }
    if selected_ids.len() > MAX_ENABLED_PROCESS_EXTENSIONS
        || (explicit_ids.is_some() && selected_ids.is_empty())
    {
        return Err(format!(
            "select 1 to {MAX_ENABLED_PROCESS_EXTENSIONS} process extensions"
        ));
    }
    let mut selected = Vec::with_capacity(selected_ids.len());
    let mut selected_set = BTreeSet::new();
    for selected_id in selected_ids {
        let id = ExtensionId::parse(selected_id.clone())
            .map_err(|_| "invalid extension plugin ID in selection".to_owned())?;
        if !selected_set.insert(id.clone()) {
            return Err(format!("duplicate extension selection: {selected_id}"));
        }
        selected.push(id);
    }

    let base = catalog_path
        .parent()
        .ok_or_else(|| "extension catalog has no parent directory".to_owned())?;
    let selected_strings = selected
        .iter()
        .map(|id| id.as_str())
        .collect::<BTreeSet<_>>();
    let mut entries = BTreeMap::new();
    let mut seen_ids = BTreeSet::new();
    for entry in catalog.extensions {
        let plugin_id = ExtensionId::parse(entry.plugin_id.clone())
            .map_err(|_| "extension catalog contains an invalid plugin ID".to_owned())?;
        if !seen_ids.insert(plugin_id.clone()) {
            return Err(format!(
                "extension catalog contains duplicate plugin ID: {}",
                plugin_id.as_str()
            ));
        }
        if entry.protocol != 1 {
            return Err(format!(
                "unsupported process extension protocol for {}",
                plugin_id.as_str()
            ));
        }
        if entry.sha256.len() != 64
            || !entry
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!(
                "invalid SHA-256 for process extension {}",
                plugin_id.as_str()
            ));
        }

        let features = entry.features.iter().cloned().collect::<BTreeSet<_>>();
        if features.len() != entry.features.len()
            || !features.contains("capability/1")
            || features
                .iter()
                .any(|feature| !matches!(feature.as_str(), "capability/1" | "ui-contribution/1"))
        {
            return Err(format!(
                "invalid process extension features for {}",
                plugin_id.as_str()
            ));
        }

        let grants = entry.grants.iter().cloned().collect::<BTreeSet<_>>();
        if grants.len() != entry.grants.len()
            || grants.iter().any(|grant| {
                !matches!(
                    grant.as_str(),
                    "usage" | "message-role" | "tool" | "check" | "lifecycle" | "diagnostic"
                )
            })
        {
            return Err(format!(
                "invalid process extension grants for {}",
                plugin_id.as_str()
            ));
        }
        if entry.executable.as_os_str().is_empty()
            || entry.executable.to_string_lossy().len() > 4_096
        {
            return Err(format!(
                "invalid executable path for process extension {}",
                plugin_id.as_str()
            ));
        }
        let executable = if entry.executable.is_absolute() {
            entry.executable
        } else {
            base.join(entry.executable)
        };
        if !executable.is_absolute() || executable.to_string_lossy().len() > 4_096 {
            return Err(format!(
                "invalid executable path for process extension {}",
                plugin_id.as_str()
            ));
        }

        if selected_strings.contains(plugin_id.as_str()) {
            let executable = std::fs::canonicalize(&executable).map_err(|_| {
                format!(
                    "cannot resolve executable for process extension {}",
                    plugin_id.as_str()
                )
            })?;
            let metadata = std::fs::metadata(&executable).map_err(|_| {
                format!(
                    "cannot inspect executable for process extension {}",
                    plugin_id.as_str()
                )
            })?;
            if !metadata.is_file() || metadata.len() > MAX_PROCESS_EXTENSION_ARTIFACT_BYTES {
                return Err(format!(
                    "executable for process extension {} must be a regular file no larger than 64 MiB",
                    plugin_id.as_str()
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return Err(format!(
                        "executable for process extension {} is not executable",
                        plugin_id.as_str()
                    ));
                }
            }
            entries.insert(
                plugin_id.clone(),
                ResolvedProcessExtension {
                    plugin_id,
                    executable,
                    sha256: entry.sha256,
                    protocol: entry.protocol,
                    features,
                    grants,
                },
            );
        }
    }

    let mut extensions = Vec::with_capacity(selected.len());
    for id in selected {
        let extension = entries.remove(&id).ok_or_else(|| {
            format!(
                "selected extension is not present in catalog: {}",
                id.as_str()
            )
        })?;
        extensions.push(extension);
    }
    Ok(ResolvedExtensionCatalog { extensions })
}

#[cfg(target_os = "linux")]
fn default_cockpit_extensions(
    config_home: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Result<ResolvedExtensionCatalog, String> {
    use std::path::Path;

    let base = config_home
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .map(Path::to_owned)
        .or_else(|| {
            home.map(Path::new)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".config"))
        });
    match base {
        Some(base) => load_process_extension_selection(
            &base.join("agenturm-rust").join("extensions.json"),
            None,
        ),
        None => Ok(ResolvedExtensionCatalog::default()),
    }
}

fn parse_query_options(args: &[String], review: bool) -> Result<Command, String> {
    let usage = || {
        if review {
            "usage: agenturm review --db <path> --harness-id <id> --session-id <id> (--turn-id <id> | --turn-of-trace <trace-id> --trace-source-id <id>) [--worker-id <id>] [--linked-traces] [--format text|json|markdown|html] [--output <new-directory>] [query paging options]".to_owned()
        } else {
            self::usage()
        }
    };
    let mut db = None;
    let mut harness_id = None;
    let mut session_id = None;
    let mut turn_id = None;
    let mut worker_id = None;
    let mut trace_id = None;
    let mut trace_source_id = None;
    let mut linked_traces = false;
    let mut format = None;
    let mut output = None;
    let mut before_timestamp = None;
    let mut before_harness = None;
    let mut before_event = None;
    let mut before_revision = None;
    let mut before_snapshot_id = None;
    let mut limit = None;
    let mut index = 0;
    while index < args.len() {
        let key = &args[index];
        if review && key == "--linked-traces" {
            if linked_traces {
                return Err(usage());
            }
            linked_traces = true;
            index += 1;
            continue;
        }
        let value = args.get(index + 1).ok_or_else(usage)?;
        if value.starts_with("--") || value.is_empty() {
            return Err(usage());
        }
        let slot = match key.as_str() {
            "--db" => &mut db,
            "--harness-id" => &mut harness_id,
            "--session-id" => &mut session_id,
            "--turn-id" if review => &mut turn_id,
            "--worker-id" if review => &mut worker_id,
            "--turn-of-trace" if review => &mut trace_id,
            "--trace-source-id" if review => &mut trace_source_id,
            "--format" if review => &mut format,
            "--output" if review => &mut output,
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
        .map(|value| value.parse::<ReadSnapshot>().map_err(|_| usage()))
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
    let query = HistoryQueryOptions {
        db: PathBuf::from(db.ok_or_else(usage)?),
        harness_id,
        session_id,
        before,
        snapshot_id,
        limit,
    };
    if review {
        let anchor = match (turn_id, trace_id, trace_source_id) {
            (Some(turn_id), None, None) => ReviewAnchor::Turn { turn_id, worker_id },
            (None, Some(trace_id), Some(trace_source_id)) => ReviewAnchor::Trace {
                trace_id: trace_id.to_ascii_lowercase(),
                trace_source_id,
                expected_worker_id: worker_id,
            },
            _ => return Err(usage()),
        };
        if query.harness_id.is_none() || query.session_id.is_none() {
            return Err(usage());
        }
        Ok(Command::Review {
            query,
            selection: ReviewSelection {
                anchor,
                linked_traces,
            },
            output: ReviewOutput {
                format: format
                    .as_deref()
                    .map(review_receipt::ReviewFormat::parse)
                    .transpose()?,
                directory: output.map(PathBuf::from),
            },
        })
    } else {
        Ok(Command::Query(query))
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
    "usage: agenturm [--help|--version|status] | agenturm cockpit [--db <path> [--extension-catalog <json-path> --extension <plugin-id> ...]] | agenturm inspect --db <path> [--extension-catalog <json-path> --extension <plugin-id> ...] | agenturm query --db <path> [--harness-id <id>] [--session-id <id>] [--limit <1..16384>] [--snapshot-id <id>] [--before-timestamp-ns <ns> --before-harness-id <id> --before-event-id <id> --before-revision <n>] | agenturm backup --db <path> [--output <new-path>|--bundle <new-directory>] | agenturm restore --bundle <directory> --output <new-directory> | agenturm prune --db <path> --before-unix-ns <ns> | agenturm discover [--follow] [--sessions-per-harness <1..8>] | agenturm collect --config <json-path> --db <path> [--watch [--interval-ms <100..60000>]] | agenturm service install --config <json-path> --db <path> [--interval-ms <100..60000>] | agenturm service status | agenturm service uninstall | agenturm ingest-usage --plugin-id <id> --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id> | agenturm ingest-codex-usage --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>".into()
}

fn parse_db_and_option<'a>(args: &'a [String], option: &str) -> Result<(PathBuf, &'a str), String> {
    let (db, value) = parse_two_options(args, "--db", option)?;
    Ok((db.into(), value))
}

fn parse_two_options<'a>(
    args: &'a [String],
    first_option: &str,
    second_option: &str,
) -> Result<(&'a str, &'a str), String> {
    let [first, first_value, second, second_value] = args else {
        return Err(usage());
    };
    let (left, right) = match (first.as_str(), second.as_str()) {
        (first, second) if first == first_option && second == second_option => {
            (first_value, second_value)
        }
        (first, second) if first == second_option && second == first_option => {
            (second_value, first_value)
        }
        _ => return Err(usage()),
    };
    if [left, right]
        .iter()
        .any(|item| item.is_empty() || item.starts_with("--"))
    {
        return Err(usage());
    }
    Ok((left, right))
}

pub fn execute_backup(db: &std::path::Path, output: &std::path::Path) -> Result<(), String> {
    let store = Store::open_read_only(db).map_err(|error| format!("open store failed: {error}"))?;
    store.backup_to(output).map_err(|error| error.to_string())
}

pub fn execute_backup_workspace(
    db: &std::path::Path,
    output: &std::path::Path,
) -> Result<(), String> {
    Store::backup_workspace_bundle(db, output).map_err(|error| error.to_string())
}

pub fn execute_restore_workspace(
    bundle: &std::path::Path,
    output: &std::path::Path,
) -> Result<(), String> {
    Store::restore_workspace_bundle(bundle, output).map_err(|error| error.to_string())
}

pub fn execute_prune(db: &std::path::Path, before_unix_ns: u64) -> Result<String, String> {
    if !db.is_file() {
        return Err("retention requires an existing observation database".into());
    }
    let mut store =
        Store::open_collector(db).map_err(|error| format!("open store failed: {error}"))?;
    let outcome = store
        .prune_observations_before(before_unix_ns)
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "Removed {} observation(s) before {} ns. Restart history queries from the newest page.\n",
        outcome.removed_observations, outcome.cutoff_unix_ns
    ))
}

pub fn execute_inspect(db_path: &std::path::Path) -> Result<Snapshot, String> {
    let store =
        Store::open_read_only(db_path).map_err(|error| format!("open store failed: {error}"))?;
    let mut app = AgenturmApp::default_composition();
    Ok(app.refresh_history_query(&store, ObservationQuery::latest(INSPECT_PAGE_SIZE)))
}

pub fn execute_cockpit(db: Option<&std::path::Path>) -> Result<(), String> {
    execute_cockpit_with_extensions(db, ResolvedExtensionCatalog::default())
}

pub fn execute_cockpit_with_extensions(
    db: Option<&std::path::Path>,
    extensions: ResolvedExtensionCatalog,
) -> Result<(), String> {
    if let Some(db) = db {
        return execute_inspect_ui(&InspectOptions {
            db: db.to_path_buf(),
            extensions,
        });
    }
    let mut startup = None;
    let mut app = AgenturmApp::default_composition();
    #[cfg(target_os = "linux")]
    let extensions = if extensions.extensions.is_empty() {
        default_cockpit_extensions(
            std::env::var_os("XDG_CONFIG_HOME").as_deref(),
            std::env::var_os("HOME").as_deref(),
        )?
    } else {
        extensions
    };
    install_process_extensions(&mut app, &extensions)?;
    let mut store = None;
    agenturm_ui::run_workspace(move |request| {
        if startup.is_none() {
            startup = Some(cockpit_startup::prepare_cockpit_store(None)?);
        }
        let startup = startup.as_ref().expect("cockpit store prepared");
        if store.is_none() {
            store = Some(
                Store::open_read_only(&startup.db)
                    .map_err(|error| format!("Open history failed: {error}"))?,
            );
        }
        let store = store.as_ref().expect("history opened");
        match request {
            WorkspaceRequest::Work(command) if cockpit_workflows::is_prompt_command(&command) => {
                startup.prompt(command).map(WorkspaceResponse::Work)
            }
            request => {
                workspace_response(&mut app, store, &startup.db, request, || startup.notice())
            }
        }
    })
    .map_err(|error| error.to_string())
}

pub fn execute_inspect_ui(options: &InspectOptions) -> Result<(), String> {
    let mut app = AgenturmApp::default_composition();
    install_process_extensions(&mut app, &options.extensions)?;
    let store = Store::open_read_only(&options.db)
        .map_err(|error| format!("open store failed: {error}"))?;
    let db = options.db.clone();
    agenturm_ui::run_workspace(move |request| {
        workspace_response(&mut app, &store, &db, request, || None)
    })
    .map_err(|error| error.to_string())
}

fn install_process_extensions(
    app: &mut AgenturmApp,
    catalog: &ResolvedExtensionCatalog,
) -> Result<(), String> {
    for extension in &catalog.extensions {
        verify_extension_artifact(&extension.executable, &extension.sha256)?;
        let grants = extension
            .grants
            .iter()
            .map(|grant| match grant.as_str() {
                "usage" => Ok(ExtensionDataGrant::Usage),
                "message-role" => Ok(ExtensionDataGrant::MessageRole),
                "tool" => Ok(ExtensionDataGrant::Tool),
                "check" => Ok(ExtensionDataGrant::Check),
                "lifecycle" => Ok(ExtensionDataGrant::Lifecycle),
                "diagnostic" => Ok(ExtensionDataGrant::Diagnostic),
                _ => Err(format!(
                    "unsupported data grant for {}",
                    extension.plugin_id.as_str()
                )),
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let process_extension =
            ProcessObservationExtension::new(ProcessObservationExtensionConfig {
                plugin_id: extension.plugin_id.clone(),
                executable: extension.executable.clone(),
                sha256: extension.sha256.clone(),
                features: extension.features.iter().cloned().collect(),
                grants,
            })?;
        app.register_observation_extension(extension.plugin_id.clone(), process_extension)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn workspace_response(
    app: &mut AgenturmApp,
    store: &impl HistoryReader,
    db: &std::path::Path,
    request: WorkspaceRequest,
    notice: impl FnOnce() -> Option<String>,
) -> Result<WorkspaceResponse, String> {
    match request {
        WorkspaceRequest::Sessions(before) => {
            let mut catalog = load_session_catalog(store, db, before)?;
            if catalog.notice.is_none() {
                catalog.notice = notice();
            }
            Ok(WorkspaceResponse::Sessions(catalog))
        }
        WorkspaceRequest::Work(command) => {
            cockpit_workflows::execute(store, db, command).map(WorkspaceResponse::Work)
        }
        WorkspaceRequest::MarkReviewed(session) => {
            cockpit_workflows::verify_review_snapshot(store, &session)?;
            let key = ReviewKey {
                harness_id: ExtensionId::parse(session.harness_id)
                    .map_err(|error| error.to_string())?,
                session_id: session.session_id,
            };
            let version = StoredReviewVersion {
                receipt_sequence: session.review_version.receipt_sequence,
                retained_observations: session.review_version.retained_observations,
                read_incarnation: session.snapshot_id.incarnation(),
            };
            let ledger =
                ReviewLedger::open(db).map_err(|error| format!("Open reviews failed: {error}"))?;
            let saved = ledger
                .mark_reviewed(&key, version, session.snapshot_id)
                .map_err(|error| format!("Save review failed: {error}"))?;
            if !saved
                && ledger
                    .reviewed_in(std::slice::from_ref(&key), version.read_incarnation)
                    .map_err(|error| format!("Read reviews failed: {error}"))?
                    .get(&key)
                    != Some(&version)
            {
                return Err(
                    "A newer review is already saved. Refresh to see current evidence.".into(),
                );
            }
            Ok(WorkspaceResponse::ReviewSaved)
        }
        WorkspaceRequest::History { selection, before } => {
            load_workspace_history(app, store, selection, before).map(WorkspaceResponse::History)
        }
    }
}

fn load_session_catalog(
    store: &impl HistoryReader,
    db: &std::path::Path,
    before: Option<SessionCursor>,
) -> Result<SessionCatalog, String> {
    let mut query = SessionQuery::latest(200);
    query.before = before.map(|cursor| SessionPosition {
        observed_at_unix_ns: cursor.updated_at_unix_ns,
        harness_id: cursor.harness_id,
        session_id: cursor.session_id,
        snapshot_id: cursor.snapshot_id,
    });
    query.snapshot_id = query.before.as_ref().map(|cursor| cursor.snapshot_id);
    let page = store
        .query_sessions(&query)
        .map_err(|error| format!("Read sessions failed: {error}"))?;
    let keys = page
        .sessions
        .iter()
        .map(|session| ReviewKey {
            harness_id: session.harness_id.clone(),
            session_id: session.session_id.clone(),
        })
        .collect::<Vec<_>>();
    let (reviewed, notice) = match ReviewLedger::open(db)
        .and_then(|ledger| ledger.reviewed_in(&keys, page.snapshot_id.incarnation()))
    {
        Ok(reviewed) => (reviewed, None),
        Err(error) => (
            BTreeMap::new(),
            Some(format!("Review status unavailable: {error}")),
        ),
    };
    let sessions = page
        .sessions
        .into_iter()
        .zip(keys)
        .map(|(session, key)| {
            let (state, reason, turn_id) = session.lifecycle.map_or((None, None, None), |life| {
                (Some(life.state), life.reason_code, life.turn_id)
            });
            RecordedSession {
                harness_id: session.harness_id.as_str().to_owned(),
                session_id: session.session_id,
                updated_at_unix_ns: session.observed_at_unix_ns,
                snapshot_id: page.snapshot_id,
                state,
                reason,
                turn_id,
                parent_session_id: session.parent_session_id,
                relationship_kind: session.relationship_kind,
                review_version: ReviewVersion {
                    receipt_sequence: session.review_version.receipt_sequence,
                    retained_observations: session.review_version.retained_observations,
                },
                reviewed: reviewed.get(&key) == Some(&session.review_version),
            }
        })
        .collect();
    let older = page.next_before.map(|cursor| SessionCursor {
        harness_id: cursor.harness_id,
        session_id: cursor.session_id,
        updated_at_unix_ns: cursor.observed_at_unix_ns,
        snapshot_id: cursor.snapshot_id,
    });
    Ok(SessionCatalog {
        sessions,
        older,
        notice,
    })
}

trait HistoryReader: ObservationStore {
    fn local_host_data(&self) -> bool {
        true
    }
    fn workspace_ledger(
        &self,
        marker: &std::path::Path,
    ) -> Result<agenturm_store::WorkspaceLedger, String>;
    fn query_sessions(&self, query: &SessionQuery) -> Result<agenturm_store::SessionPage, String>;
    fn query_linked_traces(
        &self,
        query: &ObservationQuery,
    ) -> Result<agenturm_kernel::ObservationPage, String>;
    fn query_linked_turn_traces(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> Result<agenturm_kernel::ObservationPage, String>;
    fn review_trace_turn(
        &self,
        query: &TraceReviewQuery,
    ) -> Result<Option<ResolvedTraceReview>, String>;
}

impl HistoryReader for Store {
    fn workspace_ledger(
        &self,
        marker: &std::path::Path,
    ) -> Result<agenturm_store::WorkspaceLedger, String> {
        agenturm_store::WorkspaceLedger::open(marker).map_err(|error| error.to_string())
    }
    fn query_sessions(&self, query: &SessionQuery) -> Result<agenturm_store::SessionPage, String> {
        Store::query_sessions(self, query).map_err(|error| error.to_string())
    }
    fn query_linked_traces(
        &self,
        query: &ObservationQuery,
    ) -> Result<agenturm_kernel::ObservationPage, String> {
        Store::query_linked_traces(self, query).map_err(|error| error.to_string())
    }
    fn query_linked_turn_traces(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> Result<agenturm_kernel::ObservationPage, String> {
        Store::query_linked_turn_traces(self, query, turn_id, worker_id)
            .map_err(|error| error.to_string())
    }
    fn review_trace_turn(
        &self,
        query: &TraceReviewQuery,
    ) -> Result<Option<ResolvedTraceReview>, String> {
        Store::review_trace_turn(self, query, None).map_err(|error| error.to_string())
    }
}

#[cfg(target_os = "linux")]
impl HistoryReader for agenturm_storage_service::NativeStore {
    fn workspace_ledger(
        &self,
        marker: &std::path::Path,
    ) -> Result<agenturm_store::WorkspaceLedger, String> {
        agenturm_store::WorkspaceLedger::open_with_history(marker, self.clone())
            .map_err(|error| error.to_string())
    }
    fn query_sessions(&self, query: &SessionQuery) -> Result<agenturm_store::SessionPage, String> {
        self.query_sessions(query)
            .map_err(|error| error.to_string())
    }
    fn query_linked_traces(
        &self,
        query: &ObservationQuery,
    ) -> Result<agenturm_kernel::ObservationPage, String> {
        self.query_linked_traces(query)
            .map_err(|error| error.to_string())
    }
    fn query_linked_turn_traces(
        &self,
        query: &ObservationQuery,
        turn_id: &str,
        worker_id: Option<&str>,
    ) -> Result<agenturm_kernel::ObservationPage, String> {
        self.query_linked_turn_traces(query, turn_id, worker_id)
            .map_err(|error| error.to_string())
    }
    fn review_trace_turn(
        &self,
        query: &TraceReviewQuery,
    ) -> Result<Option<ResolvedTraceReview>, String> {
        self.review_trace_turn(query)
            .map_err(|error| error.to_string())
    }
}

fn load_workspace_history(
    app: &mut AgenturmApp,
    store: &impl HistoryReader,
    selection: HistorySelection,
    before: Option<ObservationPosition>,
) -> Result<agenturm_ui::HistoryPage, String> {
    let (source, session, pinned) = match &selection {
        HistorySelection::All(_) => (None, None, None),
        HistorySelection::Session(session, _) => (
            Some(&session.harness_id),
            Some(&session.session_id),
            Some(session.snapshot_id),
        ),
        HistorySelection::Turn(turn) => (
            Some(&turn.harness_id),
            Some(&turn.session_id),
            Some(turn.snapshot_id),
        ),
    };
    let query = ObservationQuery {
        harness_id: source.cloned(),
        session_id: session.cloned(),
        snapshot_id: before.as_ref().map(|cursor| cursor.snapshot_id).or(pinned),
        before,
        limit: INSPECT_PAGE_SIZE,
    };
    match selection {
        HistorySelection::Turn(turn) => {
            let snapshot = app.kernel.refresh_worker_turn_with_store(
                store,
                query,
                &turn.turn_id,
                turn.worker_id.as_deref(),
            );
            Ok(inspector_activity::turn_page(snapshot, &turn))
        }
        HistorySelection::Session(session, view) => {
            let select_pane: fn(session_evidence::SessionPaneIds) -> ExtensionId = match view {
                SessionView::Overview => |ids| ids.overview,
                SessionView::Activity => |ids| ids.activity,
                SessionView::Review => |ids| ids.review,
                SessionView::Usage => |ids| ids.usage,
                SessionView::Traces => {
                    let page = if session.harness_id.starts_with("otlp.") {
                        store.query_observations(&query)
                    } else {
                        store.query_linked_traces(&query)
                    }
                    .map_err(|error| format!("Read session traces failed: {error}"))?;
                    let mut snapshot = Snapshot {
                        observations: page.observations,
                        observations_truncated: page.has_more,
                        history_snapshot_id: page.snapshot_id,
                        ..Snapshot::default()
                    };
                    let pane = session_evidence::add_session_trace_pane(
                        &mut snapshot,
                        &session.harness_id,
                        &session.session_id,
                    );
                    let mut page = inspector_activity::overview_page(snapshot);
                    page.initial_pane = Some(pane);
                    return Ok(page);
                }
            };
            let mut snapshot = app.refresh_history_query(store, query);
            let panes = session_evidence::add_session_panes(
                &mut snapshot,
                &session.harness_id,
                &session.session_id,
            );
            let mut page = inspector_activity::overview_page(snapshot);
            if !page.snapshot.observations.is_empty() {
                page.review_pane = Some(panes.review.clone());
            }
            page.initial_pane = Some(select_pane(panes));
            Ok(page)
        }
        HistorySelection::All(view) => {
            let mut snapshot = app.refresh_history_query(store, query);
            let ids = session_evidence::add_workspace_panes(&mut snapshot);
            let host = store
                .local_host_data()
                .then(|| cockpit_host::add_host_pane(&mut snapshot));
            let mut page = inspector_activity::overview_page(snapshot);
            page.initial_pane = Some(match view {
                WorkspaceView::Overview => {
                    ExtensionId::parse("pane.overview").expect("built-in overview ID is valid")
                }
                WorkspaceView::Activity => ids.activity,
                WorkspaceView::Mcp => ids.mcp,
                WorkspaceView::Operations => ids.operations,
                WorkspaceView::Findings => ids.findings,
                WorkspaceView::Host => host.ok_or("Host data is unavailable from this receiver")?,
                WorkspaceView::Usage => ids.usage,
                WorkspaceView::Traces => ids.traces,
            });
            Ok(page)
        }
    }
}

/// Executes one bounded read-only history query and returns a stable cursor
/// that can be passed back as `--before-*` to fetch the next older page.
pub fn execute_query(options: &HistoryQueryOptions) -> Result<String, String> {
    let store = Store::open_read_only(&options.db)
        .map_err(|error| format!("open existing store failed: {error}"))?;
    query_from_store(&store, options)
}

/// Reads one exact recorded turn. An omitted worker selects only workerless evidence.
pub fn execute_review(
    options: &HistoryQueryOptions,
    turn_id: &str,
    worker_id: Option<&str>,
) -> Result<String, String> {
    execute_review_with_output(options, turn_id, worker_id, &ReviewOutput::default())
}

pub fn execute_review_with_output(
    options: &HistoryQueryOptions,
    turn_id: &str,
    worker_id: Option<&str>,
    output: &ReviewOutput,
) -> Result<String, String> {
    execute_selected_review(
        options,
        &ReviewSelection {
            anchor: ReviewAnchor::Turn {
                turn_id: turn_id.into(),
                worker_id: worker_id.map(str::to_owned),
            },
            linked_traces: false,
        },
        output,
    )
}

pub fn execute_selected_review(
    options: &HistoryQueryOptions,
    selection: &ReviewSelection,
    output: &ReviewOutput,
) -> Result<String, String> {
    let store = Store::open_read_only(&options.db)
        .map_err(|error| format!("open existing store failed: {error}"))?;
    review_from_store(&store, options, selection, output)
}

fn query_from_store(
    store: &impl ObservationStore,
    options: &HistoryQueryOptions,
) -> Result<String, String> {
    let page = store
        .query_observations(&options.observation_query())
        .map_err(|error| format!("query store failed: {error}"))?;
    let data = history_page_data(page);
    serde_json::to_string_pretty(&data).map_err(|_| "serialize history query failed".into())
}

fn review_from_store(
    store: &impl HistoryReader,
    options: &HistoryQueryOptions,
    selection: &ReviewSelection,
    output: &ReviewOutput,
) -> Result<String, String> {
    let harness_id = options
        .harness_id
        .as_deref()
        .ok_or("Review requires a source ID")?;
    let session_id = options
        .session_id
        .as_deref()
        .ok_or("Review requires a session ID")?;
    let query = options.observation_query();
    let (turn_id, worker_id, page) = match &selection.anchor {
        ReviewAnchor::Turn { turn_id, worker_id } => {
            let page = if selection.linked_traces {
                store.query_linked_turn_traces(&query, turn_id, worker_id.as_deref())
            } else {
                store.query_worker_turn_observations(&query, turn_id, worker_id.as_deref())
            }
            .map_err(|error| format!("query store failed: {error}"))?;
            (turn_id.clone(), worker_id.clone(), page)
        }
        ReviewAnchor::Trace {
            trace_id,
            trace_source_id,
            expected_worker_id,
        } => {
            let resolved = store
                .review_trace_turn(&TraceReviewQuery {
                    page: query,
                    trace_source_id: trace_source_id.clone(),
                    trace_id: trace_id.clone(),
                    expected_worker_id: expected_worker_id.clone(),
                    linked_traces: selection.linked_traces,
                })?
                .ok_or("Trace has no unique explicit turn for the selected native scope at this snapshot")?;
            (resolved.turn_id, resolved.worker_id, resolved.page)
        }
    };
    let scope = review_receipt::ReviewScope {
        harness_id,
        session_id,
        turn_id: &turn_id,
        worker_id: worker_id.as_deref(),
    };
    let snapshot = page.snapshot_id;
    review_receipt::render(history_page_data(page), scope, snapshot, output)
}

fn history_page_data(page: agenturm_kernel::ObservationPage) -> serde_json::Value {
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
                ObservationKind::Tool { category, result } => {
                    serde_json::json!({"kind":"tool", "category":category, "result":result})
                }
                ObservationKind::FileEdit(edit) => serde_json::json!({
                    "kind":"file_edit", "added":edit.added, "updated":edit.updated,
                    "deleted":edit.deleted, "moved":edit.moved,
                }),
                ObservationKind::McpCall {
                    server,
                    tool,
                    method,
                    duration_ns,
                    result,
                } => serde_json::json!({
                    "kind":"mcp_call", "server":server, "tool":tool,
                    "method":method.as_str(), "duration_ns":duration_ns.map(|nanos| nanos.to_string()),
                    "result":result,
                }),
                ObservationKind::TraceSpan(span) => {
                    let mut evidence = serde_json::json!({
                        "kind":"trace_span", "trace_id":span.trace_id, "span_id":span.span_id,
                        "parent_span_id":span.parent_span_id,
                        "started_at_unix_ns":span.started_at_unix_ns.to_string(),
                        "status":span.status.as_str(),
                    });
                    if let Some(target) = &span.target_source_id {
                        evidence["target_source_id"] = serde_json::json!(target);
                    }
                    evidence
                }
                ObservationKind::Lifecycle { state, reason_code } => {
                    serde_json::json!({"kind":"lifecycle", "state":state, "reason_code":reason_code})
                }
                ObservationKind::Check { category, result } => {
                    serde_json::json!({"kind":"check", "category":category, "result":result})
                }
                ObservationKind::PullRequestRaised { repository, number } => {
                    serde_json::json!({"kind":"pull_request_raised", "repository":repository, "number":number})
                }
                ObservationKind::Relationship {
                    relationship_kind,
                    parent_session_id,
                } => {
                    serde_json::json!({"kind":"relationship", "relationship_kind":relationship_kind, "parent_session_id":parent_session_id})
                }
                ObservationKind::Message { role } => {
                    serde_json::json!({"kind":"message", "role":role})
                }
                ObservationKind::CreditUsage { nanocredits } => {
                    serde_json::json!({"kind":"credit_usage", "nanocredits":nanocredits})
                }
                ObservationKind::ReportedCost(cost) => serde_json::json!({
                    "kind":"reported_cost", "currency":cost.currency,
                    "nanounits":cost.nanounits.to_string(), "span":cost.span,
                }),
                ObservationKind::Diagnostic { code, severity } => {
                    serde_json::json!({"kind":"diagnostic", "code":code, "severity":severity})
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
                    "scope":match row.identity.scope {
                        agenturm_kernel::ObservationScope::Source => "source",
                        agenturm_kernel::ObservationScope::Session => "session",
                    },
                    "session_id":row.identity.session_id,
                    "worker_id":row.identity.worker_id,
                    "turn_id":row.identity.turn_id,
                    "operation_id":row.identity.operation_id,
                },
                "observation":kind,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "observations":observations,
        "has_more":page.has_more,
        "next_before":next_before,
    })
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
    let collector = ConfiguredCollector::open_with_stop(options, Some(&stop))?;
    watch_collector(collector, interval, &stop)
}

fn watch_collector<S: ObservationStore>(
    mut collector: ConfiguredCollector<S>,
    interval: Duration,
    stop: &Arc<AtomicBool>,
) -> Result<WatchReport, String> {
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

type CollectorMaintenance<S> = fn(&mut S) -> Result<(), String>;

pub struct ConfiguredCollector<S = Store> {
    kernel: Kernel,
    store: S,
    source_ids: Vec<ExtensionId>,
    startup_failures: BTreeMap<ExtensionId, String>,
    pending_sources: BTreeMap<ExtensionId, (ProcessConfig, Instant)>,
    stop_flag: Option<Arc<AtomicBool>>,
    plugin_host: PathBuf,
    follow: Option<FollowSources>,
    #[cfg(target_os = "linux")]
    database: PathBuf,
    #[cfg(target_os = "linux")]
    validate_local_database: bool,
    next_scan: Instant,
    local_maintenance: Option<CollectorMaintenance<S>>,
    next_maintenance: Instant,
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
        let database = std::path::absolute(&options.db)
            .map_err(|error| format!("resolve database path failed: {error}"))?;
        #[cfg(target_os = "linux")]
        if let Some(follow) = &config.follow {
            validate_source_root_isolation(&database, follow.selection.roots())?;
        }
        let store = Store::open_collector(&database)
            .map_err(|error| format!("open store failed: {error}"))?;
        Self::with_local_store(config, database, store, stop)
    }

    fn with_local_store(
        config: ValidatedCollectionConfig,
        database: PathBuf,
        store: Store,
        stop: Option<&Arc<AtomicBool>>,
    ) -> Result<Self, String> {
        let mut collector = Self::with_store(config, database, store, stop)?;
        collector.local_maintenance = Some(|store| {
            store
                .apply_current_automatic_retention()
                .map(|_| ())
                .map_err(|_| {
                    "Automatic retention could not run; retained history is still available".into()
                })
        });
        Ok(collector)
    }
}

impl<S: ObservationStore> ConfiguredCollector<S> {
    fn with_store(
        config: ValidatedCollectionConfig,
        database: PathBuf,
        store: S,
        stop: Option<&Arc<AtomicBool>>,
    ) -> Result<Self, String> {
        #[cfg(not(target_os = "linux"))]
        let _ = database;
        let mut collector = Self {
            kernel: Kernel::new(),
            store,
            source_ids: Vec::with_capacity(config.sources.len()),
            startup_failures: BTreeMap::new(),
            pending_sources: BTreeMap::new(),
            stop_flag: stop.cloned(),
            plugin_host: config.plugin_host,
            follow: config.follow,
            #[cfg(target_os = "linux")]
            database,
            #[cfg(target_os = "linux")]
            validate_local_database: true,
            next_scan: Instant::now() + FOLLOW_INTERVAL,
            local_maintenance: None,
            next_maintenance: Instant::now(),
        };
        for source in config.sources {
            collector.add_source(source)?;
        }
        Ok(collector)
    }

    fn add_source(&mut self, source: ConfiguredSource) -> Result<(), String> {
        let source_id = ExtensionId::parse(source.source_id.clone())
            .map_err(|error| format!("invalid source ID: {error}"))?;
        self.source_ids.push(source_id.clone());
        if self
            .stop_flag
            .as_ref()
            .is_some_and(|stop| stop.load(Ordering::Relaxed))
        {
            self.startup_failures.insert(
                source_id,
                "startup skipped because shutdown was requested".into(),
            );
            return Ok(());
        }
        let mut config = ProcessConfig::new(
            self.plugin_host.clone(),
            source.adapter,
            source.source_root,
            source.source_id,
            source.plugin_id,
        )
        .with_observation_format(source.observation_format);
        if let Some(sha256) = source.artifact_sha256 {
            config = config.with_artifact_sha256(sha256);
        }
        if let Some(features) = source.features {
            config = config.with_expected_features(features);
        }
        if let Some(stop) = &self.stop_flag {
            config = config.with_stop_flag(Arc::clone(stop));
        }
        match ProcessSource::spawn(config.clone()) {
            Ok(adapter) => {
                if let Err(error) = self
                    .kernel
                    .register_source_adapter(source_id.clone(), adapter)
                {
                    self.startup_failures.insert(source_id, error.to_string());
                }
            }
            Err(error) => {
                self.startup_failures
                    .insert(source_id.clone(), error.to_string());
                if self.stop_flag.is_some() {
                    self.pending_sources
                        .insert(source_id, (config, Instant::now() + Duration::from_secs(1)));
                }
            }
        }
        Ok(())
    }

    fn rescan_follow_sources(&mut self) -> Result<(), String> {
        let Some(follow) = &self.follow else {
            return Ok(());
        };
        #[cfg(target_os = "linux")]
        if self.validate_local_database {
            validate_source_root_isolation(&self.database, follow.selection.roots())?;
        }
        let selected = follow.resolve()?;
        let selected_ids = selected
            .iter()
            .map(|source| source.source_id.as_str())
            .collect::<BTreeSet<_>>();
        let old_ids = self
            .source_ids
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        self.source_ids.retain(|id| {
            if selected_ids.contains(id.as_str()) {
                return true;
            }
            self.kernel.unregister_source_adapter(id);
            self.pending_sources.remove(id);
            self.startup_failures.remove(id);
            false
        });
        for source in selected {
            if !old_ids.contains(&source.source_id) {
                self.add_source(source)?;
            }
        }
        Ok(())
    }

    pub fn refresh(&mut self) -> CollectionReport {
        let maintenance_error = if Instant::now() >= self.next_maintenance {
            self.next_maintenance = Instant::now() + Duration::from_secs(60);
            self.local_maintenance
                .and_then(|maintain| maintain(&mut self.store).err())
        } else {
            None
        };
        self.retry_pending_sources();
        let snapshot = self.kernel.refresh_with_store(&mut self.store);
        let shutdown_requested = self
            .stop_flag
            .as_ref()
            .is_some_and(|stop| stop.load(Ordering::Relaxed));
        let mut report = if self.follow.is_some()
            && self.stop_flag.is_some()
            && Instant::now() >= self.next_scan
            && !shutdown_requested
        {
            self.next_scan = Instant::now() + FOLLOW_INTERVAL;
            // Give each selected source its current poll before replacing it.
            // Findings from newly selected sources appear in the next cycle.
            let polled_ids = self.source_ids.clone();
            let startup_failures = self.startup_failures.clone();
            let discovery_failure = self.rescan_follow_sources().err();
            build_collection_report_with_discovery(
                &polled_ids,
                &startup_failures,
                &snapshot,
                shutdown_requested,
                discovery_failure,
            )
        } else {
            build_collection_report_with_discovery(
                &self.source_ids,
                &self.startup_failures,
                &snapshot,
                shutdown_requested,
                None,
            )
        };
        if let Some(message) = maintenance_error {
            report.summary.push_str(&format!("{message}\n"));
            report.global_failures.push(message);
        }
        report
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
            match ProcessSource::spawn(config) {
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

#[cfg(test)]
fn build_collection_report(
    source_ids: &[ExtensionId],
    startup_failures: &BTreeMap<ExtensionId, String>,
    snapshot: &Snapshot,
    shutdown_requested: bool,
) -> CollectionReport {
    build_collection_report_with_discovery(
        source_ids,
        startup_failures,
        snapshot,
        shutdown_requested,
        None,
    )
}

fn build_collection_report_with_discovery(
    source_ids: &[ExtensionId],
    startup_failures: &BTreeMap<ExtensionId, String>,
    snapshot: &Snapshot,
    shutdown_requested: bool,
    discovery_failure: Option<String>,
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
                    format!("committed {} observation(s)", status.inserted)
                };
                lines.push(format!(
                    "{}: {outcome}, skipped {} validated fact(s) without a stored projection, ACK sent={}",
                    source_id.as_str(),
                    status.skipped_unprojected_facts,
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
    if let Some(error) = discovery_failure {
        let failure = format!("discovery: {error}");
        all_lines.push(failure.clone());
        global_failures.push(failure);
    }
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
    let config: CollectionConfigFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid collection config: {error}"))?;
    let base = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let base = std::path::absolute(base)
        .map_err(|error| format!("resolve collection config directory failed: {error}"))?;
    validate_collection_config(config, base)
}

fn validate_collection_config(
    mut config: CollectionConfigFile,
    base: PathBuf,
) -> Result<ValidatedCollectionConfig, String> {
    if !matches!(config.version, 1 | 2) {
        return Err("unsupported collection config version".into());
    }
    if config.follow.is_some() && (config.version != 2 || !config.sources.is_empty()) {
        return Err(
            "follow discovery requires collection config v2 without explicit sources".into(),
        );
    }
    if config.follow.is_none()
        && (config.sources.is_empty() || config.sources.len() > MAX_CONFIGURED_SOURCES)
    {
        return Err(format!(
            "collection config must define 1 to {MAX_CONFIGURED_SOURCES} sources"
        ));
    }

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

    let catalog = match (config.version, config.plugin_catalog.take()) {
        (1, None) => BTreeMap::new(),
        (1, Some(_)) => {
            return Err("collection config v1 cannot declare a plugin catalog".into());
        }
        (2, Some(catalog_path)) => {
            let catalog_path = resolve(catalog_path);
            load_adapter_catalog(&catalog_path)?
        }
        (2, None) => {
            return Err("collection config v2 requires plugin_catalog".into());
        }
        _ => unreachable!("collection config version was validated"),
    };

    let (sources, follow) = match config.follow {
        Some(mut selection) => {
            if !(1..=discovery::MAX_SESSIONS_PER_HARNESS).contains(&selection.sessions_per_harness)
                || selection.roots().any(|root| !valid_config_path(root))
            {
                return Err("invalid follow discovery roots or session limit".into());
            }
            selection.resolve_paths(resolve);
            if selection
                .roots()
                .any(|root| !root.is_absolute() || !valid_config_path(root))
            {
                return Err("follow discovery roots must resolve to bounded absolute paths".into());
            }
            for plugin in selection.plugins() {
                if !catalog.contains_key(plugin) {
                    return Err(format!(
                        "follow discovery requires plugin {plugin} in the catalog"
                    ));
                }
            }
            let follow = FollowSources {
                selection,
                catalog,
                base,
            };
            (follow.resolve()?, Some(follow))
        }
        None => (
            resolve_collection_sources(config.sources, config.version, &catalog, &base)?,
            None,
        ),
    };
    Ok(ValidatedCollectionConfig {
        plugin_host: config.plugin_host,
        sources,
        follow,
    })
}

fn resolve_collection_sources(
    inputs: Vec<CollectionSourceInput>,
    version: u32,
    catalog: &BTreeMap<String, ValidatedAdapter>,
    base: &std::path::Path,
) -> Result<Vec<ConfiguredSource>, String> {
    let resolve = |value: PathBuf| {
        if value.is_absolute() {
            value
        } else {
            base.join(value)
        }
    };
    let mut sources = Vec::with_capacity(inputs.len());
    let mut seen_source_ids = BTreeSet::new();
    let mut seen_collection_targets = BTreeSet::new();
    for mut source in inputs {
        ExtensionId::parse(source.source_id.clone())
            .map_err(|error| format!("invalid source ID: {error}"))?;
        ExtensionId::parse(source.plugin_id.clone())
            .map_err(|_| "invalid plugin ID in collection config".to_owned())?;
        if !seen_source_ids.insert(source.source_id.clone()) {
            return Err(format!("duplicate source ID: {}", source.source_id));
        }
        let (adapter, artifact_sha256, features, observation_format) = match version {
            1 => {
                if catalog.contains_key(&source.plugin_id) {
                    return Err("collection config v1 cannot select catalog plugins".into());
                }
                let adapter = source
                    .adapter
                    .take()
                    .ok_or_else(|| "collection config v1 source requires adapter".to_owned())?;
                if adapter.as_os_str().is_empty() {
                    return Err("adapter and source-root paths must not be empty".into());
                }
                (resolve(adapter), None, None, ObservationFormat::default())
            }
            2 => {
                if source.adapter.is_some() {
                    return Err(
                        "collection config v2 sources select adapters by plugin_id only".into(),
                    );
                }
                let plugin = catalog.get(&source.plugin_id).ok_or_else(|| {
                    format!(
                        "plugin ID is not present in the selected catalog: {}",
                        source.plugin_id
                    )
                })?;
                (
                    plugin.adapter.clone(),
                    Some(plugin.sha256.clone()),
                    Some(plugin.features.clone()),
                    plugin.observation_format,
                )
            }
            _ => unreachable!("collection config version was validated"),
        };
        if source.source_root.as_os_str().is_empty() {
            return Err("adapter and source-root paths must not be empty".into());
        }
        source.source_root = resolve(std::mem::take(&mut source.source_root));
        if !valid_config_path(&adapter)
            || !valid_config_path(&source.source_root)
            || !adapter.is_absolute()
            || !source.source_root.is_absolute()
        {
            return Err(
                "adapter and source-root paths must be nonempty, bounded, and absolute after resolution"
                    .into(),
            );
        }
        let target_identity = (
            normalized_path_identity(&adapter),
            normalized_path_identity(&source.source_root),
        );
        if !seen_collection_targets.insert(target_identity) {
            return Err("the same adapter and source-root pair cannot be configured twice".into());
        }
        sources.push(ConfiguredSource {
            plugin_id: source.plugin_id,
            adapter,
            source_root: source.source_root,
            source_id: source.source_id,
            artifact_sha256,
            features,
            observation_format,
        });
    }
    Ok(sources)
}

fn load_adapter_catalog(
    path: &std::path::Path,
) -> Result<BTreeMap<String, ValidatedAdapter>, String> {
    let file = File::open(path).map_err(|_| "cannot open plugin catalog".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect plugin catalog".to_owned())?;
    if !metadata.is_file() {
        return Err("plugin catalog must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_PLUGIN_CATALOG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read plugin catalog".to_owned())?;
    if bytes.len() as u64 > MAX_PLUGIN_CATALOG_BYTES {
        return Err("plugin catalog exceeds the 64 KiB limit".into());
    }
    let catalog: AdapterCatalogFile =
        serde_json::from_slice(&bytes).map_err(|_| "plugin catalog is invalid JSON".to_owned())?;
    if catalog.version != 1 || catalog.plugins.is_empty() || catalog.plugins.len() > 64 {
        return Err("unsupported or empty plugin catalog".into());
    }
    let base = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let base = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| "cannot resolve plugin catalog directory".to_owned())?
            .join(base)
    };
    let mut plugins = BTreeMap::new();
    for mut plugin in catalog.plugins {
        ExtensionId::parse(plugin.plugin_id.clone())
            .map_err(|_| "plugin catalog contains an invalid plugin ID".to_owned())?;
        if plugin.protocol != 1 {
            return Err(format!(
                "unsupported plugin protocol for {}",
                plugin.plugin_id
            ));
        }
        let observation_format = plugin
            .observation_format
            .parse()
            .map_err(|_| format!("unsupported observation format for {}", plugin.plugin_id))?;
        if plugin.sha256.len() != 64
            || !plugin
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!("invalid SHA-256 for {}", plugin.plugin_id));
        }
        if plugin.features.is_empty()
            || plugin.features.len() > 64
            || plugin
                .features
                .iter()
                .any(|feature| !valid_plugin_feature(feature))
            || plugin.features.iter().collect::<BTreeSet<_>>().len() != plugin.features.len()
            || !plugin
                .features
                .iter()
                .any(|feature| feature == "source-adapter/1")
            || !plugin
                .features
                .iter()
                .any(|feature| feature == "configured-root/1")
        {
            return Err(format!(
                "invalid source adapter features for {}",
                plugin.plugin_id
            ));
        }
        if !plugin.adapter.is_absolute() {
            plugin.adapter = base.join(plugin.adapter);
        }
        if !valid_config_path(&plugin.adapter) || !plugin.adapter.is_absolute() {
            return Err(format!("invalid adapter path for {}", plugin.plugin_id));
        }
        plugin.adapter = std::fs::canonicalize(&plugin.adapter)
            .map_err(|_| format!("adapter artifact is unavailable for {}", plugin.plugin_id))?;
        agenturm_runtime::verify_artifact_sha256(&plugin.adapter, &plugin.sha256)
            .map_err(|_| format!("adapter artifact hash mismatch for {}", plugin.plugin_id))?;
        let adapter = ValidatedAdapter {
            adapter: plugin.adapter,
            sha256: plugin.sha256,
            features: plugin.features,
            observation_format,
        };
        if plugins.insert(plugin.plugin_id, adapter).is_some() {
            return Err("plugin catalog contains duplicate plugin IDs".into());
        }
    }
    Ok(plugins)
}

fn valid_config_path(path: &std::path::Path) -> bool {
    !path.as_os_str().is_empty() && path.to_string_lossy().len() <= 4_096
}

fn valid_plugin_feature(value: &str) -> bool {
    let Some((name, version)) = value.split_once('/') else {
        return false;
    };
    !name.is_empty()
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
        && !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_digit())
        && !version.starts_with('0')
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
    follow: Option<FollowSources>,
}

#[derive(Debug)]
struct FollowSources {
    selection: discovery::FollowDiscovery,
    catalog: BTreeMap<String, ValidatedAdapter>,
    base: PathBuf,
}

impl FollowSources {
    fn resolve(&self) -> Result<Vec<ConfiguredSource>, String> {
        resolve_collection_sources(self.selection.sources()?, 2, &self.catalog, &self.base)
    }
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
    let source = ProcessSource::spawn(config).map_err(|error| error.to_string())?;
    let mut store = Store::open_collector(&options.db)
        .map_err(|error| format!("open store failed: {error}"))?;
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
        format!("Committed {} observation(s)", status.inserted)
    };
    Ok(format!(
        "{summary}; source ACK sent. Skipped {} validated fact(s) without a stored projection.\n",
        status.skipped_unprojected_facts
    ))
}

pub fn render(command: Command) -> String {
    match command {
        Command::Help => concat!(
            "Agenturm · Rust application (early composition shell)\n",
            "\n",
            "Usage: agenturm [--help|--version|status]\n",
            "       agenturm adapter install --manifest <local-json> --sha256 <digest> --root <private-directory>\n",
            "       agenturm broker <collect|query|review|cockpit|task|grant|content|backup|integrity|prune> --broker-config <json> [command options]\n",
            "       agenturm broker serve-history --broker-config <json> --listen <loopback-IP:port> --token-file <path>\n",
            "       agenturm cockpit [--new-window] [--db <path> [--extension-catalog <json-path> --extension <plugin-id> ...]]\n",
            "       agenturm cockpit --once [--db <path>] [--json] [--limit <1..256>]\n",
            "       agenturm cockpit --endpoint http://<loopback-IP:port> --token-file <path> [--new-window]\n",
            "       agenturm broker cockpit --broker-config <json> [--new-window]\n",
            "       agenturm inspect --db <path> [--extension-catalog <json-path> --extension <plugin-id> ...]\n",
            "       agenturm query --db <path> [--harness-id <id>] [--session-id <id>] [--limit <1..16384>] [--snapshot-id <id>] [--before-timestamp-ns <ns> --before-harness-id <id> --before-event-id <id> --before-revision <n>]\n",
            "       agenturm review --db <path> --harness-id <id> --session-id <id> (--turn-id <id> | --turn-of-trace <trace-id> --trace-source-id <id>) [--worker-id <id>] [--linked-traces] [--format text|json|markdown|html] [--output <new-directory>] [query paging options]\n",
            "       agenturm investigate <instruction|path|command|source> <query> [options]\n",
            "       agenturm backup --db <path> --output <new-path>\n",
            "       agenturm backup --db <path> --bundle <new-directory>\n",
            "       agenturm restore --bundle <directory> --output <new-directory>\n",
            "       agenturm prune --db <path> --before-unix-ns <ns>\n",
            "       agenturm retention <status|set|disable|run> --db <path> [--max-age-days <days>]\n",
            "       agenturm content <grant|revoke|list|preview> [options]\n",
            "       agenturm serve-traces --listen <loopback-IP:port> --db <path> --source-id otlp.<id> [--payload-scope <standalone|harness-id> --token-file <path>]\n",
            "       agenturm run --db <path> [--source-id otlp.<id>] [--payload-scope <standalone|harness-id>] -- <program> [args...]\n",
            "       agenturm trace-payload <binding|grant|revoke|snapshot|preview> [options]\n",
            "       agenturm import-traces --file <otlp-json-path|-> --db <path> --source-id otlp.<id>\n",
            "       agenturm import-jsonl --file <span-jsonl-path|-> --db <path> --source-id otlp.<id> [--session-id <id>]\n",
            "       agenturm import-evals --file <results.json|-> --db <path> --source-id otlp.<id>\n",
            "       agenturm evals --db <path> --source-id otlp.<id> --trace-id <id> [--json]\n",
            "       agenturm attention status --db <path>\n",
            "       agenturm attention watch --db <path> [--once | --max-polls <1..10000>] [--interval-ms <100..60000>]\n",
            "       agenturm attention <status|watch> --endpoint http://<loopback-IP:port> --token-file <path> [watch options]\n",
            "       agenturm task <list|add|show|complete|reopen|link|unlink|handoffs|export> [options]\n",
            "       agenturm discover [--follow] [--sessions-per-harness <1..8>]\n",
            "       agenturm collect --config <json-path> --db <path>\n",
            "       agenturm collect --config <json-path> --db <path> --watch [--interval-ms <100..60000>]\n",
            "       agenturm ingest-usage --plugin-id <id> --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>\n",
            "       agenturm ingest-codex-usage --plugin-host <absolute-path> --adapter <absolute-path> --source-root <absolute-path> --db <path> --source-id <id>\n",
            "\n",
            "This explicit projection stores token counters and skips other supported facts.\n",
            "Run `agenturm` in a terminal to discover sessions and open the cockpit.\n",
            "On Linux, the default cockpit loads selected IDs from the enabled array in agenturm-rust/extensions.json under your configuration directory.\n",
            "Use `backup --bundle` to include saved tasks, handoffs, and reviews. Open history.sqlite in the restored directory.\n",
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
                "Registered-plugin observation decoder: available\n",
                "Configured multi-source collection: one-shot and foreground watch available\n",
                "{}"
            ),
            SERVICE_STATUS_LINE
        ),
        Command::Backup { .. }
        | Command::BackupWorkspace { .. }
        | Command::RestoreWorkspace { .. }
        | Command::Prune { .. }
        | Command::Retention(_)
        | Command::Summary(_)
        | Command::Content(_)
        | Command::TracePayload(_) => String::new(),
        Command::Discover { .. } | Command::Cockpit { .. } => String::new(),
        #[cfg(target_os = "linux")]
        Command::RemoteCockpit(_) | Command::Window(_) => String::new(),
        Command::Inspect(_) => String::new(),
        #[cfg(target_os = "linux")]
        Command::Broker(_) | Command::InstallAdapter(_) => String::new(),
        Command::Query(_) | Command::Review { .. } | Command::Attention(_) | Command::Task(_) | Command::Investigate(_) => {
            String::new()
        }
        Command::Ingest(_) | Command::ImportTraces(_) | Command::ImportJsonl(_) | Command::Evals(_) | Command::ServeTraces(_) | Command::Run(_) => String::new(),
        Command::Collect(_) => String::new(),
        Command::Service(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_preserves_recorded_lifecycle_turns_and_reasons() {
        let lifecycle =
            |event_id: &str, time, turn_id: Option<&str>, state: &str, reason: Option<&str>| {
                Observation {
                    harness_id: ExtensionId::parse("harness.codex").unwrap(),
                    event_id: event_id.into(),
                    revision: 0,
                    identity: agenturm_kernel::ObservationIdentity {
                        scope: agenturm_kernel::ObservationScope::Session,
                        session_id: Some("session-1".into()),
                        worker_id: None,
                        turn_id: turn_id.map(str::to_owned),
                        operation_id: None,
                    },
                    observed_at_unix_ns: time,
                    kind: ObservationKind::Lifecycle {
                        state: state.into(),
                        reason_code: reason.map(str::to_owned),
                    },
                }
            };
        let interrupted = lifecycle(
            "event-1",
            1,
            Some("turn-1"),
            "interrupted",
            Some("codex_task_aborted"),
        );
        let running = lifecycle("event-2", 2, None, "running", None);
        let completed = lifecycle("event-3", 3, Some("turn-2"), "completed", None);
        let values = |events: &[Observation]| {
            overview_rows(events)
                .into_iter()
                .map(|row| (row.label, row.value))
                .collect::<BTreeMap<_, _>>()
        };

        let ending = values(std::slice::from_ref(&interrupted));
        assert_eq!(
            ending["Latest lifecycle evidence"],
            "reason=codex_task_aborted · state=interrupted · harness.codex/session-1"
        );
        assert_eq!(ending["Lifecycle turn"], "turn-1");

        let ongoing = values(&[running.clone(), interrupted.clone()]);
        assert_eq!(ongoing["Lifecycle turn"], "not recorded");
        assert_eq!(ongoing["Ended turn"], "turn-1");
        assert_eq!(
            ongoing["Latest recorded turn end"],
            "reason=codex_task_aborted · state=interrupted · harness.codex/session-1"
        );

        let finished = values(&[running, completed, interrupted]);
        assert_eq!(finished["Lifecycle turn"], "turn-2");
        assert_eq!(
            finished["Latest lifecycle evidence"],
            "reason=not-recorded · state=completed · harness.codex/session-1"
        );
        assert!(!finished.contains_key("Latest recorded turn end"));
    }

    #[test]
    fn overview_exposes_latest_diagnostic_code_severity_and_scope() {
        let diagnostic = |event_id: &str,
                          timestamp,
                          scope,
                          session_id: Option<&str>,
                          code: &str,
                          severity: &str| Observation {
            harness_id: ExtensionId::parse("harness.pi").unwrap(),
            event_id: event_id.into(),
            revision: 0,
            identity: agenturm_kernel::ObservationIdentity {
                scope,
                session_id: session_id.map(str::to_owned),
                worker_id: None,
                turn_id: None,
                operation_id: None,
            },
            observed_at_unix_ns: timestamp,
            kind: ObservationKind::Diagnostic {
                code: code.into(),
                severity: severity.into(),
            },
        };
        let source = diagnostic(
            "source-gap",
            2,
            agenturm_kernel::ObservationScope::Source,
            None,
            "possible_source_gap",
            "warning",
        );
        let session = diagnostic(
            "session-gap",
            1,
            agenturm_kernel::ObservationScope::Session,
            Some("session-7"),
            "cursor_expired",
            "error",
        );

        let source_rows = overview_rows(std::slice::from_ref(&source));
        assert_eq!(
            source_rows
                .iter()
                .find(|row| row.label == "Latest diagnostic")
                .unwrap()
                .value,
            "possible_source_gap · warning · harness.pi · source"
        );
        let mixed_rows = overview_rows(&[source.clone(), session]);
        assert_eq!(
            mixed_rows
                .iter()
                .find(|row| row.label == "Latest diagnostic")
                .unwrap()
                .value,
            "possible_source_gap · warning · harness.pi · source",
            "newest evidence is selected by event time"
        );
        let corrected = Observation {
            revision: 2,
            kind: ObservationKind::Diagnostic {
                code: "cursor_expired".into(),
                severity: "error".into(),
            },
            ..source.clone()
        };
        let revised_rows = overview_rows(&[source.clone(), corrected]);
        assert_eq!(
            revised_rows
                .iter()
                .find(|row| row.label == "Latest diagnostic")
                .unwrap()
                .value,
            "cursor_expired · error · harness.pi · source",
            "equal-time revisions use the highest revision"
        );
        let latest_session = diagnostic(
            "session-latest",
            3,
            agenturm_kernel::ObservationScope::Session,
            Some("session-7"),
            "cursor_expired",
            "error",
        );
        let session_rows = overview_rows(&[latest_session]);
        assert_eq!(
            session_rows
                .iter()
                .find(|row| row.label == "Latest diagnostic")
                .unwrap()
                .value,
            "cursor_expired · error · harness.pi · session session-7"
        );
    }

    #[test]
    fn overview_counts_typed_message_roles_without_content() {
        let observation = Observation {
            harness_id: ExtensionId::parse("harness.codex").unwrap(),
            event_id: "message-1".into(),
            revision: 0,
            identity: agenturm_kernel::ObservationIdentity {
                scope: agenturm_kernel::ObservationScope::Session,
                session_id: Some("session-1".into()),
                worker_id: None,
                turn_id: Some("turn-1".into()),
                operation_id: None,
            },
            observed_at_unix_ns: 1,
            kind: ObservationKind::Message {
                role: "user".into(),
            },
        };
        let rows = overview_rows(&[observation]);
        let count = rows
            .iter()
            .find(|row| row.label == "Message role observations")
            .unwrap();
        assert_eq!(count.value, "1");
    }

    struct FixtureHarness;

    impl Harness for FixtureHarness {
        fn poll(&mut self) -> Result<Vec<agenturm_kernel::RawObservation>, String> {
            Ok(vec![agenturm_kernel::RawObservation {
                event_id: "fixture:event-1".into(),
                revision: 0,
                identity: agenturm_kernel::ObservationIdentity {
                    scope: agenturm_kernel::ObservationScope::Session,
                    session_id: Some("session-1".into()),
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
            Command::Inspect(InspectOptions {
                db: PathBuf::from("./state.sqlite"),
                extensions: ResolvedExtensionCatalog::default(),
            })
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
                snapshot_id: Some(12.into()),
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
                    snapshot_id: 1.into(),
                }),
                snapshot_id: Some(1.into()),
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

    #[cfg(unix)]
    fn extension_catalog_test_directory() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-app-extension-catalog-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[cfg(unix)]
    fn extension_catalog_entry(plugin_id: &str, executable: &str) -> serde_json::Value {
        serde_json::json!({
            "plugin_id": plugin_id,
            "executable": executable,
            "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "protocol": 1,
            "features": ["capability/1", "ui-contribution/1"],
            "grants": ["usage", "message-role"]
        })
    }

    #[cfg(unix)]
    fn write_extension_catalog(path: &std::path::Path, value: &serde_json::Value) {
        std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn inspect_resolves_only_selected_extension_entries_before_store_open() {
        let directory = extension_catalog_test_directory();
        let executable = directory.join("extension-fixture");
        std::fs::copy("/bin/true", &executable).unwrap();
        let metadata = std::fs::metadata(&executable).unwrap();
        let mut permissions = metadata.permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let catalog_path = directory.join("extensions.json");
        let catalog = serde_json::json!({
            "version": 1,
            "extensions": [
                extension_catalog_entry("example.prompt-signals", "extension-fixture"),
                extension_catalog_entry("example.not-selected", "missing/unselected-plugin")
            ]
        });
        write_extension_catalog(&catalog_path, &catalog);

        let options = parse_command(&[
            "inspect".into(),
            "--extension".into(),
            "example.prompt-signals".into(),
            "--extension-catalog".into(),
            catalog_path.to_string_lossy().into_owned(),
            "--db".into(),
            "not-created.sqlite".into(),
        ])
        .unwrap();
        let Command::Inspect(options) = options else {
            panic!("inspect options expected");
        };
        assert_eq!(options.db, PathBuf::from("not-created.sqlite"));
        assert_eq!(options.extensions.extensions.len(), 1);
        let extension = &options.extensions.extensions[0];
        assert_eq!(extension.plugin_id.as_str(), "example.prompt-signals");
        assert_eq!(
            extension.executable,
            std::fs::canonicalize(executable).unwrap()
        );
        assert_eq!(extension.sha256, "0".repeat(64));
        assert_eq!(extension.protocol, 1);
        assert_eq!(
            extension.features,
            BTreeSet::from(["capability/1".into(), "ui-contribution/1".into()])
        );
        assert_eq!(
            extension.grants,
            BTreeSet::from(["usage".into(), "message-role".into()])
        );

        let database = directory.join("must-not-open.sqlite");
        let error = execute_inspect_ui(&InspectOptions {
            db: database.clone(),
            extensions: options.extensions,
        })
        .unwrap_err();
        assert!(error.contains("SHA-256 mismatch"));
        assert!(!database.exists());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cockpit_enables_only_selected_user_extensions_and_accepts_explicit_inspect_flags() {
        let directory = extension_catalog_test_directory();
        let config_home = directory.join("xdg");
        let home = directory.join("home");
        let config_dir = config_home.join("agenturm-rust");
        std::fs::create_dir_all(&config_dir).unwrap();
        let catalog_path = config_dir.join("extensions.json");
        let defaults =
            || default_cockpit_extensions(Some(config_home.as_os_str()), Some(home.as_os_str()));
        assert!(defaults().unwrap().extensions.is_empty());
        write_extension_catalog(
            &catalog_path,
            &serde_json::json!({
                "version": 1, "enabled": [], "extensions": []
            }),
        );
        assert!(defaults().unwrap().extensions.is_empty());

        let mut catalog = serde_json::json!({
            "version": 1,
            "extensions": [
                extension_catalog_entry("example.selected", "/bin/true"),
                extension_catalog_entry("example.unselected", "missing")
            ]
        });
        write_extension_catalog(&catalog_path, &catalog);
        assert!(defaults().unwrap().extensions.is_empty());
        catalog["enabled"] = serde_json::json!(["example.selected"]);
        write_extension_catalog(&catalog_path, &catalog);
        let selected = defaults().unwrap();
        assert_eq!(selected.extensions.len(), 1);
        assert_eq!(
            selected.extensions[0].plugin_id.as_str(),
            "example.selected"
        );

        let fallback = home.join(".config/agenturm-rust");
        std::fs::create_dir_all(&fallback).unwrap();
        write_extension_catalog(&fallback.join("extensions.json"), &catalog);
        assert_eq!(
            default_cockpit_extensions(Some("relative".as_ref()), Some(home.as_os_str())).unwrap(),
            selected
        );

        let command = parse_command(&[
            "cockpit".into(),
            "--db".into(),
            "existing.sqlite".into(),
            "--extension-catalog".into(),
            catalog_path.to_string_lossy().into_owned(),
            "--extension".into(),
            "example.selected".into(),
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::Cockpit {
                db: Some("existing.sqlite".into()),
                extensions: selected,
            }
        );
        assert!(
            execute_cockpit_with_extensions(
                Some(&config_dir.join("must-not-open.sqlite")),
                defaults().unwrap(),
            )
            .unwrap_err()
            .contains("SHA-256 mismatch")
        );
        assert!(!config_dir.join("must-not-open.sqlite").exists());

        catalog["enabled"] = serde_json::json!(["example.absent"]);
        write_extension_catalog(&catalog_path, &catalog);
        assert!(defaults().unwrap_err().contains("not present in catalog"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn inspect_requires_catalog_selection_and_rejects_duplicate_unknown_and_excess_ids() {
        let directory = extension_catalog_test_directory();
        let catalog_path = directory.join("extensions.json");
        let catalog = serde_json::json!({
            "version": 1,
            "extensions": [extension_catalog_entry("example.one", "missing-plugin")]
        });
        write_extension_catalog(&catalog_path, &catalog);
        let catalog_arg = catalog_path.to_string_lossy().into_owned();

        assert!(
            parse_command(&[
                "inspect".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--extension".into(),
                "example.one".into(),
            ])
            .unwrap_err()
            .contains("requires --extension-catalog")
        );
        assert!(
            parse_command(&[
                "inspect".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--extension-catalog".into(),
                catalog_arg.clone(),
            ])
            .unwrap_err()
            .contains("requires at least one --extension")
        );
        assert!(
            parse_command(&[
                "inspect".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--extension-catalog".into(),
                catalog_arg.clone(),
                "--extension".into(),
                "example.one".into(),
                "--extension".into(),
                "example.one".into(),
            ])
            .unwrap_err()
            .contains("duplicate extension selection")
        );
        assert!(
            parse_command(&[
                "inspect".into(),
                "--db".into(),
                "state.sqlite".into(),
                "--extension-catalog".into(),
                catalog_arg.clone(),
                "--extension".into(),
                "example.unknown".into(),
            ])
            .unwrap_err()
            .contains("not present in catalog")
        );

        let mut too_many = vec![
            "inspect".into(),
            "--db".into(),
            "state.sqlite".into(),
            "--extension-catalog".into(),
            catalog_arg,
        ];
        for _ in 0..=MAX_ENABLED_PROCESS_EXTENSIONS {
            too_many.extend(["--extension".into(), "example.one".into()]);
        }
        assert!(
            parse_command(&too_many)
                .unwrap_err()
                .contains("at most 4 extensions")
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn extension_catalog_rejects_bad_schema_declarations_and_oversized_files() {
        let directory = extension_catalog_test_directory();
        let executable = directory.join("plugin");
        std::fs::copy("/bin/true", &executable).unwrap();
        let catalog_path = directory.join("extensions.json");
        let valid_entry = extension_catalog_entry("example.selected", "plugin");
        let selected = vec!["example.selected".to_owned()];

        let invalid_catalogs = [
            serde_json::json!({"version":2,"extensions":[valid_entry.clone()]}),
            serde_json::json!({"version":1,"extensions":[valid_entry.clone(),valid_entry.clone()]}),
            serde_json::json!({"version":1,"extensions":[{
                "plugin_id":"example.selected","executable":"plugin","sha256":"ABCDEF",
                "protocol":1,"features":["capability/1"],"grants":[]
            }]}),
            serde_json::json!({"version":1,"extensions":[{
                "plugin_id":"example.selected","executable":"plugin","sha256":"0000000000000000000000000000000000000000000000000000000000000000",
                "protocol":2,"features":["capability/1"],"grants":[]
            }]}),
            serde_json::json!({"version":1,"extensions":[{
                "plugin_id":"example.selected","executable":"plugin","sha256":"0000000000000000000000000000000000000000000000000000000000000000",
                "protocol":1,"features":["capability/1","unknown/1"],"grants":[]
            }]}),
            serde_json::json!({"version":1,"extensions":[{
                "plugin_id":"example.selected","executable":"plugin","sha256":"0000000000000000000000000000000000000000000000000000000000000000",
                "protocol":1,"features":["capability/1"],"grants":["raw-content"]
            }]}),
            serde_json::json!({"version":1,"extensions":[{
                "plugin_id":"example.selected","executable":"plugin","sha256":"0000000000000000000000000000000000000000000000000000000000000000",
                "protocol":1,"features":["capability/1","capability/1"],"grants":[]
            }]}),
            serde_json::json!({"version":1,"extensions":[{
                "plugin_id":"example.selected","executable":"plugin","sha256":"0000000000000000000000000000000000000000000000000000000000000000",
                "protocol":1,"features":["capability/1"],"grants":["usage","usage"]
            }]}),
        ];
        for catalog in invalid_catalogs {
            write_extension_catalog(&catalog_path, &catalog);
            assert!(load_process_extension_catalog(&catalog_path, &selected).is_err());
        }

        write_extension_catalog(
            &catalog_path,
            &serde_json::json!({
                "version": 1,
                "extensions": [valid_entry],
                "unexpected": true
            }),
        );
        assert!(load_process_extension_catalog(&catalog_path, &selected).is_err());

        std::fs::write(
            &catalog_path,
            vec![b' '; MAX_PLUGIN_CATALOG_BYTES as usize + 1],
        )
        .unwrap();
        assert!(
            load_process_extension_catalog(&catalog_path, &selected)
                .unwrap_err()
                .contains("64 KiB limit")
        );

        let over_count = serde_json::json!({
            "version": 1,
            "extensions": (0..=MAX_PROCESS_EXTENSION_CATALOG_ENTRIES)
                .map(|index| extension_catalog_entry(&format!("example.plugin-{index}"), "plugin"))
                .collect::<Vec<_>>()
        });
        write_extension_catalog(&catalog_path, &over_count);
        assert!(load_process_extension_catalog(&catalog_path, &selected).is_err());

        std::fs::remove_dir_all(directory).unwrap();
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
                scope: agenturm_store::StoredScope::Session,
                session_id: Some("session-a".into()),
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
        let mut check_event = make_event("event-1", 10);
        check_event.kind = StoredKind::Check {
            category: "lint".into(),
            result: "passed".into(),
        };
        let mut lifecycle_event = make_event("event-2", 20);
        lifecycle_event.kind = StoredKind::Lifecycle {
            state: "interrupted".into(),
            reason_code: Some("codex_task_aborted".into()),
        };
        let mut tool_event = make_event("event-3", 30);
        tool_event.kind = StoredKind::Tool {
            category: "shell".into(),
            result: "failed".into(),
        };
        let mut message_event = make_event("event-4", 15);
        message_event.kind = StoredKind::Message {
            role: "assistant".into(),
        };
        let mut source_diagnostic = make_event("event-5", 25);
        source_diagnostic.identity = StoredIdentity {
            scope: agenturm_store::StoredScope::Source,
            session_id: None,
            worker_id: None,
            turn_id: None,
            operation_id: None,
        };
        source_diagnostic.kind = StoredKind::Diagnostic {
            code: "possible_source_gap".into(),
            severity: "warning".into(),
        };
        store
            .commit_batch(&ObservationBatch {
                request_id: "query-app-request".into(),
                source_id: "harness.codex".into(),
                expected_cursor: None,
                next_cursor: "cursor-1".into(),
                observations: vec![
                    check_event,
                    lifecycle_event,
                    tool_event,
                    message_event,
                    source_diagnostic,
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
        assert_eq!(first["observations"][1]["observation"]["kind"], "tool");
        assert_eq!(first["observations"][1]["observation"]["category"], "shell");
        assert_eq!(first["observations"][1]["observation"]["result"], "failed");
        assert_eq!(first["observations"][0]["observation"]["kind"], "lifecycle");
        assert_eq!(
            first["observations"][0]["observation"]["state"],
            "interrupted"
        );
        assert_eq!(
            first["observations"][0]["observation"]["reason_code"],
            "codex_task_aborted"
        );
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
        assert_eq!(older["observations"][0]["observation"]["kind"], "check");
        assert_eq!(older["observations"][0]["observation"]["category"], "lint");
        assert_eq!(older["observations"][0]["observation"]["result"], "passed");
        assert_eq!(older["observations"][1]["observation"]["kind"], "message");
        assert_eq!(older["observations"][1]["observation"]["role"], "assistant");
        let all_scopes = execute_query(&HistoryQueryOptions {
            db: path.clone(),
            harness_id: Some("harness.codex".into()),
            session_id: None,
            before: None,
            snapshot_id: None,
            limit: 10,
        })
        .unwrap();
        let all_scopes: serde_json::Value = serde_json::from_str(&all_scopes).unwrap();
        let source_row = all_scopes["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["event_id"] == "event-5")
            .unwrap();
        assert_eq!(source_row["identity"]["scope"], "source");
        assert!(source_row["identity"]["session_id"].is_null());
        assert_eq!(source_row["observation"]["code"], "possible_source_gap");
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
        assert!(status.contains("Registered-plugin observation decoder: available"));
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
            artifact_sha256: None,
            features: None,
            observation_format: ObservationFormat::default(),
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
                skipped_unprojected_facts: 0,
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
                .contains("harness.healthy: committed 2 observation")
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
    fn discover_cli_accepts_follow_and_rejects_duplicate_flags() {
        let parse = |args: &[&str]| {
            parse_command(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
        };
        assert_eq!(
            parse(&["discover", "--sessions-per-harness", "1", "--follow"]).unwrap(),
            Command::Discover {
                sessions_per_harness: 1,
                follow: true
            }
        );
        assert!(parse(&["discover", "--follow", "--follow"]).is_err());
        assert!(parse(&["discover", "--follow", "--sessions-per-harness", "0"]).is_err());
    }

    #[test]
    fn version_two_collection_config_resolves_and_pins_catalog_plugins_before_store_open() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-plugin-catalog-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let adapter = directory.join("adapter.bin");
        std::fs::write(&adapter, b"adapter-artifact").unwrap();
        let catalog_path = directory.join("plugins.json");
        let catalog = serde_json::json!({
            "version": 1,
            "plugins": [{
                "plugin_id": "example.rust-pi-harness",
                "adapter": "adapter.bin",
                "sha256": "2de1f3a1752dc35308e48cde8ee2e146620d022fa0974ffbd4acf078bcb665f0",
                "protocol": 1,
                "features": ["source-adapter/1", "configured-root/1"]
            }]
        });
        std::fs::write(&catalog_path, serde_json::to_vec(&catalog).unwrap()).unwrap();
        let config_path = directory.join("sources.json");
        let config = serde_json::json!({
            "version": 2,
            "plugin_host": "plugin-host",
            "plugin_catalog": "plugins.json",
            "sources": [{
                "plugin_id": "example.rust-pi-harness",
                "source_root": "sessions",
                "source_id": "harness.pi"
            }]
        });
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();

        let loaded = load_collection_config(&config_path).unwrap();
        assert_eq!(loaded.sources.len(), 1);
        assert_eq!(
            loaded.sources[0].adapter,
            std::fs::canonicalize(&adapter).unwrap()
        );
        assert_eq!(
            loaded.sources[0].artifact_sha256.as_deref(),
            Some("2de1f3a1752dc35308e48cde8ee2e146620d022fa0974ffbd4acf078bcb665f0")
        );
        assert_eq!(loaded.sources[0].features.as_ref().unwrap().len(), 2);
        assert_eq!(
            loaded.sources[0].observation_format,
            ObservationFormat::default()
        );

        let db = directory.join("should-not-open.sqlite");
        let options = ConfiguredCollectOptions {
            config: config_path.clone(),
            db: db.clone(),
            watch_interval: None,
        };

        let unknown_plugin = serde_json::json!({
            "version": 2,
            "plugin_host": "plugin-host",
            "plugin_catalog": "plugins.json",
            "sources": [{"plugin_id":"example.unknown","source_root":"sessions","source_id":"harness.unknown"}]
        });
        std::fs::write(&config_path, serde_json::to_vec(&unknown_plugin).unwrap()).unwrap();
        assert!(
            load_collection_config(&config_path)
                .unwrap_err()
                .contains("not present")
        );
        assert!(ConfiguredCollector::open(&options).is_err());
        assert!(
            !db.exists(),
            "invalid plugin selection must fail before opening the store"
        );

        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let duplicate_catalog = serde_json::json!({
            "version": 1,
            "plugins": [catalog["plugins"][0].clone(), catalog["plugins"][0].clone()]
        });
        std::fs::write(
            &catalog_path,
            serde_json::to_vec(&duplicate_catalog).unwrap(),
        )
        .unwrap();
        assert!(
            load_collection_config(&config_path)
                .unwrap_err()
                .contains("duplicate plugin IDs")
        );
        std::fs::write(&catalog_path, serde_json::to_vec(&catalog).unwrap()).unwrap();
        std::fs::write(&adapter, b"tampered-adapter").unwrap();
        assert!(
            load_collection_config(&config_path)
                .unwrap_err()
                .contains("hash mismatch")
        );
        assert!(ConfiguredCollector::open(&options).is_err());
        assert!(
            !db.exists(),
            "tampered adapter must fail before opening the store"
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
