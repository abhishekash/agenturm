//! Explicit host measurement capture and bounded retained-history reads.

use agenturm_kernel::{
    ExtensionId, ObservationPage, ObservationPosition, ObservationQuery, ObservationStore,
    is_host_source,
};
use agenturm_store::{MAX_HOST_HISTORY_PAGE, Store};
use serde::Deserialize;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const USAGE: &str = "agenturm host record --db <path> --source-id host.<label> [--watch [--interval-ms <1000..60000>]]\nagenturm host history --db <path> [--source-id host.<label>] [--limit <1..64>] [--before <position-json>]";
const DEFAULT_INTERVAL_MS: u64 = 5_000;
const INTERVAL_MS: std::ops::RangeInclusive<u64> = 1_000..=60_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostCaptureConfig {
    source_id: String,
    interval_ms: Option<u64>,
}

#[derive(Debug)]
pub(super) struct HostCaptureSchedule {
    pub source: ExtensionId,
    interval: Duration,
    next_capture: Instant,
}

pub(super) struct HostCaptureResult {
    pub source: ExtensionId,
    pub inserted: Result<usize, String>,
}

impl HostCaptureConfig {
    pub fn validate(self) -> Result<HostCaptureSchedule, String> {
        if !cfg!(target_os = "linux") {
            return Err("Managed Host capture requires Linux procfs".into());
        }
        if !is_host_source(&self.source_id) {
            return Err("Managed Host capture requires a host.<label> source ID".into());
        }
        let source = ExtensionId::parse(self.source_id)
            .map_err(|_| "Invalid managed Host capture source ID")?;
        let interval_ms = self.interval_ms.unwrap_or(DEFAULT_INTERVAL_MS);
        if !INTERVAL_MS.contains(&interval_ms) {
            return Err("Managed Host capture interval must be 1000 to 60000 milliseconds".into());
        }
        Ok(HostCaptureSchedule {
            source,
            interval: Duration::from_millis(interval_ms),
            next_capture: Instant::now(),
        })
    }
}

impl HostCaptureSchedule {
    pub fn capture_due(&mut self, store: &mut impl ObservationStore) -> Option<HostCaptureResult> {
        if Instant::now() < self.next_capture {
            return None;
        }
        let result = commit_sample(store, &self.source);
        self.next_capture = Instant::now() + self.interval;
        Some(HostCaptureResult {
            source: self.source.clone(),
            inserted: result.map(|receipt| receipt.inserted),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostOptions {
    db: Option<PathBuf>,
    settings: HostSettings,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HostSettings {
    Record {
        source: ExtensionId,
        watch: Option<Duration>,
    },
    History(ObservationQuery),
}

pub(super) fn parse(args: &[String], broker: bool) -> Result<HostOptions, String> {
    let (verb, mut args) = args.split_first().ok_or(USAGE)?;
    let (mut db, mut source, mut limit, mut before, mut interval) = (None, None, None, None, None);
    let mut watch = false;
    while let Some((flag, rest)) = args.split_first() {
        args = rest;
        if flag == "--watch" && !watch && verb == "record" {
            watch = true;
            continue;
        }
        let (value, rest) = args
            .split_first()
            .filter(|(value, _)| !value.is_empty() && !value.starts_with("--"))
            .ok_or(USAGE)?;
        args = rest;
        match flag.as_str() {
            "--db" if !broker && db.is_none() => db = Some(PathBuf::from(value)),
            "--source-id" if source.is_none() && is_host_source(value) => {
                source = Some(ExtensionId::parse(value.clone()).map_err(|_| USAGE)?);
            }
            "--interval-ms" if verb == "record" && interval.is_none() => {
                interval = Some(Duration::from_millis(
                    value
                        .parse()
                        .ok()
                        .filter(|ms| INTERVAL_MS.contains(ms))
                        .ok_or(USAGE)?,
                ));
            }
            "--limit" if verb == "history" && limit.is_none() => {
                limit = Some(
                    value
                        .parse()
                        .ok()
                        .filter(|value| (1..=MAX_HOST_HISTORY_PAGE).contains(value))
                        .ok_or(USAGE)?,
                );
            }
            "--before" if verb == "history" && before.is_none() => {
                before =
                    Some(serde_json::from_str::<ObservationPosition>(value).map_err(|_| USAGE)?);
            }
            _ => return Err(USAGE.into()),
        }
    }
    if !broker && db.is_none() {
        return Err(USAGE.into());
    }
    let settings = match verb.as_str() {
        "record" if watch || interval.is_none() => HostSettings::Record {
            source: source.ok_or(USAGE)?,
            watch: watch.then(|| interval.unwrap_or(Duration::from_millis(DEFAULT_INTERVAL_MS))),
        },
        "history" => HostSettings::History(ObservationQuery {
            harness_id: source.map(|source| source.as_str().to_owned()),
            snapshot_id: before.as_ref().map(|position| position.snapshot_id),
            before,
            ..ObservationQuery::latest(limit.unwrap_or(MAX_HOST_HISTORY_PAGE))
        }),
        _ => return Err(USAGE.into()),
    };
    Ok(HostOptions { db, settings })
}

pub fn execute(options: &HostOptions, output: impl Write) -> Result<(), String> {
    let db = options.db.as_deref().ok_or(USAGE)?;
    match &options.settings {
        HostSettings::History(query) => {
            let store = Store::open_read_only(db).map_err(|error| error.to_string())?;
            print_page(
                store
                    .query_host_history(query, None)
                    .map_err(|error| error.to_string())?,
                output,
            )
        }
        HostSettings::Record { source, watch } => {
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (source, watch, output);
                Err("Host capture requires Linux procfs".into())
            }
            #[cfg(target_os = "linux")]
            {
                let mut store = Store::open_collector(db).map_err(|error| error.to_string())?;
                record(&mut store, source, *watch, output)
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn execute_broker(
    options: &HostOptions,
    mut store: agenturm_storage_service::NativeStore,
    output: impl Write,
) -> Result<(), String> {
    match &options.settings {
        HostSettings::History(query) => {
            let identity = store
                .history_identity()
                .map_err(|error| error.to_string())?;
            let store = store
                .with_workspace_identity(&identity)
                .map_err(|error| error.to_string())?;
            print_page(
                store
                    .query_host_history(query)
                    .map_err(|error| error.to_string())?,
                output,
            )
        }
        HostSettings::Record { source, watch } => record(&mut store, source, *watch, output),
    }
}

fn print_page(page: ObservationPage, mut output: impl Write) -> Result<(), String> {
    let result = serde_json::json!({
        "snapshot_id":page.snapshot_id,
        "older":page.older_position(),
        "samples":page.observations,
    });
    writeln!(output, "{result}").map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
fn record(
    store: &mut impl ObservationStore,
    source: &ExtensionId,
    watch: Option<Duration>,
    mut output: impl Write,
) -> Result<(), String> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = crate::WatchSignalRegistrations::new(Arc::clone(&stop))?;
    while !stop.load(Ordering::Relaxed) {
        let receipt = commit_sample(store, source)?;
        if let Err(error) = writeln!(
            output,
            "{}",
            serde_json::json!({
                "source_id":source.as_str(), "event_id":receipt.event_id, "inserted":receipt.inserted
            })
        )
        .and_then(|()| output.flush())
        {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                break;
            }
            return Err(error.to_string());
        }
        let Some(interval) = watch else {
            break;
        };
        let next = Instant::now() + interval;
        while let Some(remaining) = next.checked_duration_since(Instant::now()) {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(remaining.min(Duration::from_millis(50)));
        }
    }
    Ok(())
}

struct HostReceipt {
    #[cfg(target_os = "linux")]
    event_id: String,
    inserted: usize,
}

#[cfg(not(target_os = "linux"))]
fn commit_sample(_: &mut impl ObservationStore, _: &ExtensionId) -> Result<HostReceipt, String> {
    Err("Host capture requires Linux procfs".into())
}

#[cfg(target_os = "linux")]
fn commit_sample(
    store: &mut impl ObservationStore,
    source: &ExtensionId,
) -> Result<HostReceipt, String> {
    use agenturm_kernel::{Observation, ObservationIdentity, ObservationKind};
    use std::io::Read;
    use std::time::{SystemTime, UNIX_EPOCH};

    const HOST_CURSOR_PREFIX: &str = "host-sample-v1:";
    // Reject unauthorized principals and foreign checkpoint formats before
    // reading procfs. The commit independently checks source grant and CAS.
    let cursor = store.source_cursor(source.as_str())?;
    if cursor
        .as_deref()
        .is_some_and(|cursor| !cursor.starts_with(HOST_CURSOR_PREFIX))
    {
        return Err("Host source already belongs to another collector format".into());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|time| u64::try_from(time.as_nanos()).ok())
        .ok_or("Cannot time-stamp the host sample")?;
    let mut identity = [0_u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut identity))
        .map_err(|_| "Cannot generate a host event ID")?;
    let event_id = format!("{HOST_CURSOR_PREFIX}{:032x}", u128::from_be_bytes(identity));
    let row = Observation {
        harness_id: source.clone(),
        identity: ObservationIdentity::source(),
        event_id: event_id.clone(),
        revision: 0,
        observed_at_unix_ns: now,
        kind: ObservationKind::HostSample(crate::cockpit_host::capture()?),
    };
    let receipt = store.commit(source.as_str(), cursor.as_deref(), &event_id, &[row])?;
    Ok(HostReceipt {
        event_id,
        inserted: receipt.inserted,
    })
}
