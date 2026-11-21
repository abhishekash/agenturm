//! Default cockpit launch and session-owned collection for a bundled install.

use super::{
    CollectionConfigFile, CollectionReport, ConfiguredCollector, Store, discovery,
    validate_collection_config,
};
use agenturm_store::StoreError;
use std::env;
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub(super) struct CockpitStartup {
    pub(super) db: PathBuf,
    notice: Option<String>,
    collection: Option<ActiveCollector>,
}

impl CockpitStartup {
    pub(super) fn notice(&self) -> Option<String> {
        self.collection
            .as_ref()
            .and_then(|collection| collection.notice.lock().ok()?.clone())
            .or_else(|| self.notice.clone())
    }
}

struct ActiveCollector {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    notice: Arc<Mutex<Option<String>>>,
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
        let worker = thread::Builder::new()
            .name("agenturm-cockpit-collector".into())
            .spawn(move || {
                loop {
                    thread::park_timeout(Duration::from_secs(1));
                    if worker_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let report = collector.refresh();
                    if let Ok(mut notice) = worker_notice.lock() {
                        *notice = collection_notice(&report);
                    }
                }
            })
            .map_err(|error| format!("start cockpit collector failed: {error}"))?;
        Ok(Self {
            stop,
            worker: Some(worker),
            notice,
        })
    }
}

impl Drop for ActiveCollector {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

pub(super) fn prepare_cockpit_store(db: Option<&Path>) -> Result<CockpitStartup, String> {
    if let Some(db) = db {
        return Ok(CockpitStartup {
            db: db.to_owned(),
            notice: None,
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

    let store = match Store::open_collector(&db) {
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
    let stop = Arc::new(AtomicBool::new(false));
    let mut collector = ConfiguredCollector::with_store(config, db.clone(), store, Some(&stop))?;
    let report = collector.refresh();
    let collection = ActiveCollector::start(collector, stop, report)?;
    Ok(CockpitStartup {
        db,
        notice: None,
        collection: Some(collection),
    })
}

fn collection_notice(report: &CollectionReport) -> Option<String> {
    report.has_failures().then(|| {
        format!(
            "Showing available history. {}",
            report
                .summary
                .lines()
                .next()
                .unwrap_or("Collection failed.")
        )
    })
}

fn state_directory(state_home: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf, String> {
    let state_home = state_home.map(Path::new).filter(|path| path.is_absolute());
    let home = home.map(Path::new).filter(|path| path.is_absolute());
    let base = state_home
        .map(Path::to_owned)
        .or_else(|| home.map(|path| path.join(".local/state")))
        .ok_or_else(|| "set an absolute XDG_STATE_HOME or HOME to launch the cockpit".to_owned())?;
    Ok(base.join("agenturm-rust"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cockpit_uses_xdg_state_home_or_the_standard_home_directory() {
        let cwd = env::current_dir().unwrap();
        let state_home = cwd.join("state");
        let home = cwd.join("home");
        assert_eq!(
            state_directory(Some(state_home.as_os_str()), Some(home.as_os_str())).unwrap(),
            state_home.join("agenturm-rust"),
        );
        assert_eq!(
            state_directory(Some(OsStr::new("relative-state")), Some(home.as_os_str())).unwrap(),
            home.join(".local/state/agenturm-rust"),
        );
        assert!(state_directory(None, Some(OsStr::new("relative-home"))).is_err());
    }
}
