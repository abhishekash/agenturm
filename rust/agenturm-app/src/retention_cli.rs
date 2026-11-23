//! Explicit configuration and inspection of the writer's age policy.

use agenturm_store::{AutomaticRetentionPolicy, Store};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(super) struct LocalRetentionMaintenance {
    db: PathBuf,
    next: Instant,
}
impl LocalRetentionMaintenance {
    pub(super) fn new(db: PathBuf) -> Self {
        Self {
            db,
            next: Instant::now(),
        }
    }
    pub(super) fn tick(&mut self) {
        if Instant::now() < self.next {
            return;
        }
        self.next = Instant::now() + Duration::from_secs(60);
        if !self.db.is_file() {
            return;
        }
        if self.apply().is_err() {
            eprintln!("Automatic retention could not run; retained history remains available");
        }
    }
    fn apply(&self) -> Result<(), String> {
        let reader = Store::open_read_only(&self.db).map_err(|_| "Cannot read retention policy")?;
        if reader
            .automatic_retention_status()
            .map_err(|_| "Cannot read retention policy")?
            .policy
            .and_then(|policy| policy.max_age_ns)
            .is_none()
        {
            return Ok(());
        }
        drop(reader);
        let mut store =
            Store::open_collector(&self.db).map_err(|_| "Cannot acquire retention writer")?;
        store
            .apply_current_automatic_retention()
            .map(|_| ())
            .map_err(|_| "Cannot apply retention policy".into())
    }
}

pub(super) const USAGE: &str = "agenturm retention <status|set|disable|run> --db <path> [--max-age-days <positive-days>]\nFor broker history use agenturm broker retention with --broker-config in place of --db.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetentionOptions {
    db: Option<PathBuf>,
    action: Action,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Action {
    Status,
    Set(AutomaticRetentionPolicy),
    Run,
}

pub(super) fn parse(args: &[String], broker: bool) -> Result<RetentionOptions, String> {
    let [verb, flags @ ..] = args else {
        return Err(USAGE.into());
    };
    let mut db = None;
    let mut age = None;
    let (pairs, remainder) = flags.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(USAGE.into());
    }
    for [flag, value] in pairs {
        match flag.as_str() {
            "--db" if !broker && db.is_none() && !value.is_empty() && !value.starts_with("--") => {
                db = Some(value.into())
            }
            "--max-age-days" if age.is_none() => age = Some(parse_age(value)?),
            _ => return Err(USAGE.into()),
        }
    }
    if !broker && db.is_none() {
        return Err(USAGE.into());
    }
    let action = match (verb.as_str(), age) {
        ("status", None) => Action::Status,
        ("set", Some(age)) => Action::Set(AutomaticRetentionPolicy {
            max_age_ns: Some(age),
        }),
        ("disable", None) => Action::Set(AutomaticRetentionPolicy { max_age_ns: None }),
        ("run", None) => Action::Run,
        _ => return Err(USAGE.into()),
    };
    Ok(RetentionOptions { db, action })
}

fn parse_age(value: &str) -> Result<u64, String> {
    let days: f64 = value
        .parse()
        .map_err(|_| "Retention days must be positive and finite")?;
    if !days.is_finite() || days <= 0.0 {
        return Err("Retention days must be positive and finite".into());
    }
    let duration = Duration::try_from_secs_f64(days * 86_400.0)
        .map_err(|_| "Retention days exceed the supported range")?;
    let ns = u64::try_from(duration.as_nanos())
        .map_err(|_| "Retention days exceed the supported range")?;
    AutomaticRetentionPolicy {
        max_age_ns: Some(ns),
    }
    .validate()
    .map_err(|_| "Retention days exceed the supported range")?;
    Ok(ns)
}

fn encode(value: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map(|value| value + "\n")
        .map_err(|_| "Could not encode retention status".into())
}

pub fn execute_retention(options: &RetentionOptions) -> Result<String, String> {
    let db = options.db.as_ref().ok_or("Supply a history database")?;
    match options.action {
        Action::Status => {
            let store = Store::open_read_only(db).map_err(|error| error.to_string())?;
            encode(
                &store
                    .automatic_retention_status()
                    .map_err(|error| error.to_string())?,
            )
        }
        Action::Set(policy) => {
            let mut store = Store::open_collector(db).map_err(|error| error.to_string())?;
            store
                .set_automatic_retention(policy)
                .map_err(|error| error.to_string())?;
            encode(
                &store
                    .automatic_retention_status()
                    .map_err(|error| error.to_string())?,
            )
        }
        Action::Run => {
            if !db.is_file() {
                return Err("Automatic retention requires an existing history".into());
            }
            let mut store = Store::open_collector(db).map_err(|error| error.to_string())?;
            let outcome = store
                .apply_current_automatic_retention()
                .map_err(|error| error.to_string())?;
            encode(
                &serde_json::json!({"outcome": outcome, "status": store.automatic_retention_status().map_err(|error| error.to_string())?}),
            )
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn execute_broker(
    options: &RetentionOptions,
    store: &agenturm_storage_service::NativeStore,
) -> Result<String, String> {
    match options.action {
        Action::Status => encode(
            &store
                .automatic_retention_status()
                .map_err(|error| error.to_string())?,
        ),
        Action::Set(policy) => encode(
            &store
                .set_automatic_retention(policy)
                .map_err(|error| error.to_string())?,
        ),
        Action::Run => {
            let outcome = store
                .apply_current_automatic_retention()
                .map_err(|error| error.to_string())?;
            encode(
                &serde_json::json!({"outcome": outcome, "status": store.automatic_retention_status().map_err(|error| error.to_string())?}),
            )
        }
    }
}
