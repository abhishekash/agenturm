//! Default cockpit launch and session-owned collection for a bundled install.

use super::{
    CollectionConfigFile, CollectionReport, ConfiguredCollector, Store, discovery,
    validate_collection_config,
};
use agenturm_kernel::{ExtensionId, Observation};
use agenturm_store::StoreError;
use agenturm_ui::{WorkCommand, WorkResult};
use std::env;
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(super) struct CockpitStartup {
    pub(super) db: PathBuf,
    notice: Option<String>,
    collection: Option<ActiveCollector>,
}

impl CockpitStartup {
    pub(super) fn source_authority(&self) -> Option<discovery::SourceAuthority> {
        self.collection
            .as_ref()?
            .authority
            .lock()
            .ok()
            .map(|authority| authority.clone())
    }

    pub(super) fn prompt(&self, command: WorkCommand) -> Result<WorkResult, String> {
        let Some(collector) = &self.collection else {
            return super::cockpit_workflows::execute_prompt(&self.db, None, command);
        };
        let (reply, result) = mpsc::sync_channel(1);
        collector
            .requests
            .as_ref()
            .ok_or("Cockpit collector stopped")?
            .send(CollectorRequest::Prompt {
                command: Box::new(command),
                reply,
            })
            .map_err(|_| "Cockpit collector stopped")?;
        result.recv().map_err(|_| "Cockpit collector stopped")?
    }

    pub(super) fn notice(&self) -> Option<String> {
        self.collection
            .as_ref()
            .and_then(|collection| {
                let notice = collection.notice.lock().ok()?.clone();
                let receiver = collection.receiver_notice.lock().ok()?.clone();
                merge_collection_notices(notice, receiver)
            })
            .or_else(|| self.notice.clone())
    }
}

enum CollectorRequest {
    Prompt {
        command: Box<WorkCommand>,
        reply: mpsc::SyncSender<Result<WorkResult, String>>,
    },
    Trace {
        source: ExtensionId,
        observations: Vec<Observation>,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
}

struct ActiveCollector {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    receiver: Option<JoinHandle<()>>,
    requests: Option<mpsc::SyncSender<CollectorRequest>>,
    notice: Arc<Mutex<Option<String>>>,
    receiver_notice: Arc<Mutex<Option<String>>>,
    authority: Arc<Mutex<discovery::SourceAuthority>>,
}

impl ActiveCollector {
    fn start(
        mut collector: ConfiguredCollector,
        stop: Arc<AtomicBool>,
        initial_report: CollectionReport,
    ) -> Result<Self, String> {
        let worker_stop = Arc::clone(&stop);
        let notice = Arc::new(Mutex::new(collection_notice(&initial_report)));
        let worker_notice = Arc::clone(&notice);
        let mut native_members = super::native_session_context::StartupMembers::default();
        let authority = Arc::new(Mutex::new(
            native_members
                .update(Some(collector.source_authority()))
                .expect("configured source authority exists"),
        ));
        let worker_authority = Arc::clone(&authority);
        let (requests, work) = mpsc::sync_channel::<CollectorRequest>(2);
        let receiver_notice = Arc::new(Mutex::new(None));
        let bound_receiver = super::trace_receiver::TraceReceiver::bind_cockpit();
        let worker = thread::Builder::new()
            .name("agenturm-cockpit-collector".into())
            .spawn(move || {
                let interval = Duration::from_secs(1);
                let mut refresh = Instant::now() + interval;
                while !worker_stop.load(Ordering::Relaxed) {
                    match work.recv_timeout(refresh.saturating_duration_since(Instant::now())) {
                        Ok(CollectorRequest::Prompt { command, reply }) => {
                            let result = match *command {
                                WorkCommand::EnablePromptHistory(owner) => {
                                    enable_prompt_history(&mut collector, owner)
                                }
                                command => super::cockpit_workflows::execute_prompt_with_config(
                                    &collector.database,
                                    Some(&collector.store),
                                    command,
                                    Some(&collector),
                                ),
                            };
                            let _ = reply.send(result);
                        }
                        Ok(CollectorRequest::Trace {
                            source,
                            observations,
                            reply,
                        }) => {
                            let result = super::trace_import::commit_with_store(
                                &mut collector.store,
                                &source,
                                &observations,
                            )
                            .map(|_| ());
                            let _ = reply.send(result);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    if worker_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if Instant::now() < refresh {
                        continue;
                    }
                    refresh = Instant::now() + interval;
                    let report = collector.refresh();
                    if let Ok(mut authority) = worker_authority.lock() {
                        *authority = native_members
                            .update(Some(collector.source_authority()))
                            .expect("configured source authority exists");
                    }
                    if let Ok(mut notice) = worker_notice.lock() {
                        *notice = collection_notice(&report);
                    }
                }
            })
            .map_err(|error| format!("start cockpit collector failed: {error}"))?;
        let mut receiver = None;
        let startup_error = if let Ok(bound) = bound_receiver {
            let trace_stop = Arc::clone(&stop);
            let trace_requests = requests.clone();
            let failure = Arc::clone(&receiver_notice);
            match thread::Builder::new()
                .name("agenturm-cockpit-otlp".into())
                .spawn(move || {
                    if bound
                        .serve_with(&trace_stop, |source, observations| {
                            let (reply, result) = mpsc::sync_channel(1);
                            trace_requests
                                .try_send(CollectorRequest::Trace {
                                    source: source.clone(),
                                    observations,
                                    reply,
                                })
                                .map_err(|_| "Cockpit collector is busy")?;
                            result
                                .recv_timeout(Duration::from_secs(3))
                                .map_err(|_| "Cockpit collector is busy")?
                        })
                        .is_err()
                        && let Ok(mut notice) = failure.lock()
                    {
                        *notice = Some("The local OTLP receiver stopped.".into());
                    }
                }) {
                Ok(worker) => {
                    receiver = Some(worker);
                    None
                }
                Err(_) => Some("The local OTLP receiver could not start."),
            }
        } else {
            Some("The local OTLP port is unavailable. History is open.")
        };
        if let Some(startup_error) = startup_error
            && let Ok(mut notice) = receiver_notice.lock()
        {
            *notice = Some(startup_error.to_owned());
        }
        Ok(Self {
            stop,
            worker: Some(worker),
            receiver,
            requests: Some(requests),
            notice,
            receiver_notice,
            authority,
        })
    }
}

fn enable_prompt_history(
    collector: &mut ConfiguredCollector,
    owner: agenturm_ui::WorkAnchor,
) -> Result<WorkResult, String> {
    #[cfg(target_os = "linux")]
    {
        let scope =
            super::content_preview::prompt_grant_scope(&collector.store, collector, &owner)?;
        collector
            .store
            .grant_content(&scope)
            .map_err(|error| error.to_string())?;
        super::cockpit_workflows::execute_prompt_with_config(
            &collector.database,
            Some(&collector.store),
            WorkCommand::Prompts(owner),
            Some(collector),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (collector, owner);
        Err("Prompt history is available on Linux".into())
    }
}

impl Drop for ActiveCollector {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.requests.take();
        if let Some(receiver) = self.receiver.take() {
            let _ = receiver.join();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn prepare_cockpit_store(db: Option<&Path>) -> Result<CockpitStartup, String> {
    if let Some(db) = db {
        return Ok(CockpitStartup {
            db: db.to_owned(),
            notice: Some("Explicit database · saved history; discovery disabled".into()),
            collection: None,
        });
    }

    let state_home = env::var_os("XDG_STATE_HOME");
    let home = env::var_os("HOME");
    let directory = state_directory(state_home.as_deref(), home.as_deref())?;
    let db = directory.join("history.sqlite");
    let config = discovery::discover(discovery::DEFAULT_SESSIONS_PER_HARNESS, true)
        .and_then(|json| {
            serde_json::from_str::<CollectionConfigFile>(&json)
                .map_err(|error| format!("read bundled discovery configuration failed: {error}"))
        })
        .and_then(|config| validate_collection_config(config, directory.clone()));
    let config = match config {
        Ok(config) => config,
        Err(error) if db.is_file() => {
            Store::open_read_only(&db).map_err(|store_error| store_error.to_string())?;
            return Ok(CockpitStartup {
                db,
                notice: Some(format!("Showing saved history. Discovery failed: {error}")),
                collection: None,
            });
        }
        Err(error) => return Err(error),
    };

    #[cfg(target_os = "linux")]
    if let Some(follow) = &config.follow {
        super::validate_source_root_isolation(&db, follow.selection.roots())?;
    }

    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(directory)
        .map_err(|error| format!("create cockpit state directory failed: {error}"))?;

    let mut store = match Store::open_collector(&db) {
        Ok(store) => store,
        Err(StoreError::WriterLease(error)) if error.kind() == ErrorKind::WouldBlock => {
            Store::open_read_only(&db).map_err(|error| error.to_string())?;
            return Ok(CockpitStartup {
                db,
                notice: Some(
                    "Opened shared history while another process owned the writer lease.".into(),
                ),
                collection: None,
            });
        }
        Err(error) => return Err(format!("open cockpit database failed: {error}")),
    };
    store
        .initialize_managed_retention()
        .map_err(|_| "Could not initialize the managed retention policy")?;
    let stop = Arc::new(AtomicBool::new(false));
    let mut collector =
        ConfiguredCollector::with_local_store(config, db.clone(), store, Some(&stop))?;
    let report = collector.refresh();
    let collection = ActiveCollector::start(collector, stop, report)?;
    Ok(CockpitStartup {
        db,
        notice: None,
        collection: Some(collection),
    })
}

fn collection_notice(report: &CollectionReport) -> Option<String> {
    if report.has_failures() {
        let mut failures = Vec::new();
        if report.failed_sources > 0 {
            failures.push(format!(
                "{} source {}",
                report.failed_sources,
                if report.failed_sources == 1 {
                    "failure"
                } else {
                    "failures"
                }
            ));
        }
        if !report.global_failures.is_empty() {
            failures.push(format!(
                "{} collection {}",
                report.global_failures.len(),
                if report.global_failures.len() == 1 {
                    "failure"
                } else {
                    "failures"
                }
            ));
        }
        let mut notice = format!(
            "{}\nShowing available history.\n{}",
            failures.join(" · "),
            report.summary.trim_end()
        );
        if let Some(coverage) = report.discovery {
            notice.push('\n');
            notice.push_str(&coverage.notice());
        }
        Some(notice)
    } else {
        report.discovery.map(|discovery| {
            let summary = if discovery.scan_partial || discovery.handles_partial {
                "Discovery incomplete · history available".into()
            } else {
                let queued = discovery.eligible.saturating_sub(discovery.selected);
                if queued > 0 {
                    format!("Discovering sessions · {queued} queued")
                } else {
                    "Collection complete".into()
                }
            };
            format!("{summary}\n{}", discovery.notice())
        })
    }
}

/// The footer displays the first line, so receiver failures must remain there
/// even when a successful discovery notice already has a detailed second line.
fn merge_collection_notices(
    collection: Option<String>,
    receiver: Option<String>,
) -> Option<String> {
    match (collection, receiver) {
        (Some(collection), Some(receiver)) => {
            let collection_summary = collection.lines().next().unwrap_or_default();
            let receiver_summary = receiver.lines().next().unwrap_or_default();
            // Keep both failure counts and receiver health visible. Discovery
            // counters can stay in the full scrollable collection status.
            let summary = if collection_summary.contains("failure") {
                format!("{collection_summary} · OTLP unavailable")
            } else {
                receiver_summary.to_owned()
            };
            Some(format!("{summary}\n{receiver}\n{collection}"))
        }
        (collection, receiver) => collection.or(receiver),
    }
}

pub(super) fn state_directory(
    state_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<PathBuf, String> {
    let state_home = state_home.map(Path::new).filter(|path| path.is_absolute());
    let home = home.map(Path::new).filter(|path| path.is_absolute());
    if let Some(state_home) = state_home {
        return Ok(state_home.join("agenturm-rust"));
    }
    let home = home
        .ok_or_else(|| "set an absolute XDG_STATE_HOME or HOME to launch the cockpit".to_owned())?;
    #[cfg(target_os = "macos")]
    return Ok(home.join("Library/Application Support/Agenturm/Rust"));
    #[cfg(not(target_os = "macos"))]
    Ok(home.join(".local/state/agenturm-rust"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_notice_keeps_failure_counts_brief_and_full_diagnostics_readable() {
        let report = CollectionReport {
            summary: "18 sources checked; 2 source failures; 1 global failure\nsource-a: permission denied\nsource-b: failed to read\nstorage: write failed\n".into(),
            failed_sources: 2,
            global_failures: vec!["write failed".into()],
            discovery: Some(discovery::DiscoveryCoverage {
                selected: 16,
                eligible: 341,
                ..Default::default()
            }),
        };
        let notice = collection_notice(&report).unwrap();
        assert_eq!(
            notice.lines().next(),
            Some("2 source failures · 1 collection failure")
        );
        for detail in [
            "source-a: permission denied",
            "source-b: failed to read",
            "storage: write failed",
            "Discovery: 16/341 selected",
            "325 queued",
        ] {
            assert!(notice.contains(detail), "missing {detail}: {notice}");
        }
    }

    #[test]
    fn healthy_discovery_notice_does_not_hide_partial_or_queued_coverage() {
        let mut report = CollectionReport {
            summary: String::new(),
            failed_sources: 0,
            global_failures: vec![],
            discovery: None,
        };
        assert!(collection_notice(&report).is_none());
        report.discovery = Some(discovery::DiscoveryCoverage {
            selected: 3,
            eligible: 5,
            ..Default::default()
        });
        let notice = collection_notice(&report).unwrap();
        assert_eq!(
            notice.lines().next(),
            Some("Discovering sessions · 2 queued")
        );
        assert!(notice.contains("Discovery: 3/5 selected"));
        report.discovery.as_mut().unwrap().scan_partial = true;
        assert_eq!(
            collection_notice(&report).unwrap().lines().next(),
            Some("Discovery incomplete · history available")
        );
        report.discovery = Some(discovery::DiscoveryCoverage {
            selected: 5,
            eligible: 5,
            ..Default::default()
        });
        assert_eq!(
            collection_notice(&report).unwrap().lines().next(),
            Some("Collection complete")
        );
    }

    #[test]
    fn receiver_failure_remains_on_the_visible_notice_line() {
        let receiver = "The local OTLP port is unavailable. History is open.".to_owned();
        let healthy = "Collection complete\nDiscovery: 3/3 selected".to_owned();
        let notice =
            merge_collection_notices(Some(healthy.clone()), Some(receiver.clone())).unwrap();
        assert_eq!(notice.lines().next(), Some(receiver.as_str()));
        assert!(notice.contains(&healthy));
        let failed = "1 source failure\nsource-a: permission denied".to_owned();
        let notice =
            merge_collection_notices(Some(failed.clone()), Some(receiver.clone())).unwrap();
        assert_eq!(
            notice.lines().next(),
            Some("1 source failure · OTLP unavailable")
        );
        assert!(notice.contains(&failed));
        assert!(notice.contains(&receiver));
        assert_eq!(
            merge_collection_notices(None, Some(receiver.clone())),
            Some(receiver)
        );
    }

    #[test]
    fn cockpit_uses_xdg_state_home_or_the_standard_home_directory() {
        let cwd = env::current_dir().unwrap();
        let state_home = cwd.join("state");
        let home = cwd.join("home");
        assert_eq!(
            state_directory(Some(state_home.as_os_str()), Some(home.as_os_str())).unwrap(),
            {
                #[cfg(target_os = "macos")]
                {
                    state_home.join("agenturm-rust")
                }
                #[cfg(not(target_os = "macos"))]
                {
                    state_home.join("agenturm-rust")
                }
            },
        );
        assert_eq!(
            state_directory(Some(OsStr::new("relative-state")), Some(home.as_os_str())).unwrap(),
            {
                #[cfg(target_os = "macos")]
                {
                    home.join("Library/Application Support/Agenturm/Rust")
                }
                #[cfg(not(target_os = "macos"))]
                {
                    home.join(".local/state/agenturm-rust")
                }
            },
        );
        assert!(state_directory(None, Some(OsStr::new("relative-home"))).is_err());
    }
}
