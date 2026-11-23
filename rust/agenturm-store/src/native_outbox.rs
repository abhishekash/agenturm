//! Durable, ordered dispatch of metadata-only native lifecycle events.

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const APPLICATION_ID: i64 = 0x414e_4f58;
const SCHEMA: i64 = 1;
const MAX_SENDERS: i64 = 1_000;
const MAX_PENDING: i64 = 1_000;
const MAX_RECEIPTS: i64 = 65_536;
const MAX_REQUESTS: i64 = 1_000;
const FLUSH_LIMIT: usize = 100;
const UNAVAILABLE: &str = "Native outbox is unavailable";
const UNSAFE_STATE: &str = "Native outbox must be an owner-only application database";
const BINDING_CHANGED: &str = "Native sender binding changed; use a new explicit binding";

type Result<T> = std::result::Result<T, String>;

pub use agenturm_kernel::native_attention::NativeAttentionKind as EventKind;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub event_id: String,
    /// Absent only for Codex's transport-ordered completion notification.
    pub sequence: Option<u64>,
    pub kind: EventKind,
    /// Python bindings name the retained trace. Native Rust bindings use an exact turn.
    pub trace_id: Option<String>,
    pub turn_id: String,
    pub request_id: Option<String>,
    pub source_time_unix_ns: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedEvent {
    pub event: Event,
    pub sequence: u64,
    pub request_kind: Option<EventKind>,
    pub received_at_unix_ns: u64,
}

pub struct NativeOutbox(Connection);

impl NativeOutbox {
    pub fn open(path: &Path, owner_uid: u32) -> Result<Self> {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(UNAVAILABLE.into()),
        }
        let prior = private_file(path, owner_uid)?;
        let mut db = sql(Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        ))?;
        let current = private_file(path, owner_uid)?;
        if (prior.dev(), prior.ino()) != (current.dev(), current.ino()) {
            return Err(UNSAFE_STATE.into());
        }
        sql(db.busy_timeout(Duration::from_secs(3)))?;
        sql(db.pragma_update(None, "trusted_schema", false))?;
        sql(db.pragma_update(None, "synchronous", "FULL"))?;
        let tx = sql(db.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let application: i64 =
            sql(tx.pragma_query_value(None, "application_id", |row| row.get(0)))?;
        let version: i64 = sql(tx.pragma_query_value(None, "user_version", |row| row.get(0)))?;
        match (application, version) {
            (APPLICATION_ID, SCHEMA) => {}
            (0, 0) if count(&tx, "SELECT COUNT(*) FROM sqlite_schema")? == 0 => {
                sql(tx.execute_batch(
                    "CREATE TABLE senders(
                        sender_id TEXT PRIMARY KEY, binding_hash BLOB NOT NULL, sequence INTEGER NOT NULL
                    ) STRICT;
                    CREATE TABLE receipts(
                        sender_id TEXT NOT NULL, event_id TEXT NOT NULL, event_hash BLOB NOT NULL,
                        sequence INTEGER NOT NULL, payload TEXT,
                        PRIMARY KEY(sender_id,event_id), UNIQUE(sender_id,sequence)
                    ) STRICT;
                    CREATE INDEX pending_events ON receipts(sender_id, sequence) WHERE payload IS NOT NULL;
                    CREATE TABLE requests(
                        sender_id TEXT NOT NULL, request_id TEXT NOT NULL, kind TEXT NOT NULL,
                        trace_id TEXT NOT NULL, turn_id TEXT NOT NULL, resolved INTEGER NOT NULL,
                        PRIMARY KEY(sender_id,request_id)
                    ) STRICT;",
                ))?;
                sql(tx.pragma_update(None, "application_id", APPLICATION_ID))?;
                sql(tx.pragma_update(None, "user_version", SCHEMA))?;
            }
            _ => return Err(UNSAFE_STATE.into()),
        }
        sql(tx.commit())?;
        Ok(Self(db))
    }

    pub fn enqueue(&mut self, sender: &str, binding: &[u8; 32], event: Event) -> Result<bool> {
        validate_event(sender, &event)?;
        let digest = fingerprint(&event)?;
        let tx = sql(self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let last = sender_sequence(&tx, sender, binding)?;
        if last.is_none() {
            if count(&tx, "SELECT COUNT(*) FROM senders")? >= MAX_SENDERS {
                return Err("Native sender capacity reached".into());
            }
            sql(tx.execute(
                "INSERT INTO senders VALUES(?1,?2,0)",
                params![sender, binding],
            ))?;
        }
        let prior: Option<Vec<u8>> = sql(tx
            .query_row(
                "SELECT event_hash FROM receipts WHERE sender_id=?1 AND event_id=?2",
                params![sender, event.event_id],
                |row| row.get(0),
            )
            .optional())?;
        if let Some(prior) = prior {
            if prior == digest {
                return Ok(false);
            }
            return Err("Native event identity conflicts with prior metadata".into());
        }
        let last = last.unwrap_or(0);
        let sequence = event.sequence.unwrap_or_else(|| last.saturating_add(1));
        if sequence <= last || sequence > i64::MAX as u64 {
            return Err("Native event order is stale or invalid".into());
        }
        if count(
            &tx,
            "SELECT COUNT(*) FROM receipts WHERE payload IS NOT NULL",
        )? >= MAX_PENDING
        {
            return Err("Native pending outbox capacity reached; retry pending events".into());
        }
        if count(&tx, "SELECT COUNT(*) FROM receipts")? >= MAX_RECEIPTS {
            return Err("Native event receipt capacity reached".into());
        }
        let request_kind = update_request(&tx, sender, &event)?;
        let received_at_unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|time| u64::try_from(time.as_nanos()).ok())
            .filter(|time| (1..=i64::MAX as u64).contains(time))
            .ok_or("Native bridge receipt time is unavailable")?;
        let row = QueuedEvent {
            event,
            sequence,
            request_kind,
            received_at_unix_ns,
        };
        let payload = serde_json::to_string(&row).map_err(|_| UNAVAILABLE)?;
        sql(tx.execute(
            "INSERT INTO receipts VALUES(?1,?2,?3,?4,?5)",
            params![sender, row.event.event_id, digest, sequence, payload],
        ))?;
        sql(tx.execute(
            "UPDATE senders SET sequence=?2 WHERE sender_id=?1",
            params![sender, sequence],
        ))?;
        sql(tx.commit())?;
        Ok(true)
    }

    /// Serialize concurrent flushers and preserve the durable pending row until
    /// the selected receiver confirms acceptance for the exact event.
    pub fn flush(
        &mut self,
        sender: &str,
        binding: &[u8; 32],
        mut post: impl FnMut(&QueuedEvent) -> Result<()>,
    ) -> Result<usize> {
        for sent in 0..FLUSH_LIMIT {
            let tx = sql(self
                .0
                .transaction_with_behavior(TransactionBehavior::Immediate))?;
            if sender_sequence(&tx, sender, binding)?.is_none() {
                return Ok(sent);
            }
            let next: Option<(String, String)> = sql(tx
                .query_row(
                    "SELECT event_id,payload FROM receipts
                 WHERE sender_id=?1 AND payload IS NOT NULL ORDER BY sequence LIMIT 1",
                    [sender],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional())?;
            let Some((key, payload)) = next else {
                return Ok(sent);
            };
            let event: QueuedEvent = serde_json::from_str(&payload).map_err(|_| UNAVAILABLE)?;
            if key != event.event.event_id {
                return Err(UNSAFE_STATE.into());
            }
            validate_event(sender, &event.event)?;
            post(&event)?;
            sql(tx.execute(
                "UPDATE receipts SET payload=NULL WHERE sender_id=?1 AND event_id=?2",
                params![sender, key],
            ))?;
            sql(tx.commit())?;
        }
        Ok(FLUSH_LIMIT)
    }
}

fn private_file(path: &Path, owner: u32) -> Result<fs::Metadata> {
    let info = fs::symlink_metadata(path).map_err(|_| UNSAFE_STATE)?;
    if !info.is_file() || info.nlink() != 1 || info.mode() & 0o077 != 0 || info.uid() != owner {
        return Err(UNSAFE_STATE.into());
    }
    Ok(info)
}

fn sql<T>(value: rusqlite::Result<T>) -> Result<T> {
    value.map_err(|_| UNAVAILABLE.into())
}

fn count(tx: &Transaction<'_>, query: &str) -> Result<i64> {
    sql(tx.query_row(query, [], |row| row.get(0)))
}

fn sender_sequence(tx: &Transaction<'_>, sender: &str, binding: &[u8; 32]) -> Result<Option<u64>> {
    let prior: Option<(u64, Vec<u8>)> = sql(tx
        .query_row(
            "SELECT sequence,binding_hash FROM senders WHERE sender_id=?1",
            [sender],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional())?;
    match prior {
        Some((sequence, hash)) if hash == *binding => Ok(Some(sequence)),
        Some(_) => Err(BINDING_CHANGED.into()),
        None => Ok(None),
    }
}

fn update_request(tx: &Transaction<'_>, sender: &str, event: &Event) -> Result<Option<EventKind>> {
    let Some(request) = event.request_id.as_deref() else {
        return Ok(None);
    };
    let trace_id = event.trace_id.as_deref().unwrap_or("");
    let query =
        "SELECT kind,trace_id,turn_id,resolved FROM requests WHERE sender_id=?1 AND request_id=?2";
    let prior: Option<(String, String, String, bool)> = sql(tx
        .query_row(query, params![sender, request], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional())?;
    if event.kind.is_request() {
        if prior.is_some() {
            return Err("Native request identity was reused".into());
        }
        if count(tx, "SELECT COUNT(*) FROM requests")? >= MAX_REQUESTS {
            return Err("Native request capacity reached".into());
        }
        sql(tx.execute(
            "INSERT INTO requests VALUES(?1,?2,?3,?4,?5,0)",
            params![
                sender,
                request,
                event.kind.as_str(),
                trace_id,
                event.turn_id
            ],
        ))?;
        return Ok(Some(event.kind));
    }
    let Some((kind, trace, turn, false)) = prior else {
        return Err("No matching unresolved native request".into());
    };
    if trace != trace_id || turn != event.turn_id {
        return Err("Native resolution does not match the recorded request scope".into());
    }
    let kind = match kind.as_str() {
        "approval_requested" => EventKind::ApprovalRequested,
        "reply_requested" => EventKind::ReplyRequested,
        _ => return Err(UNSAFE_STATE.into()),
    };
    sql(tx.execute(
        "UPDATE requests SET resolved=1 WHERE sender_id=?1 AND request_id=?2",
        params![sender, request],
    ))?;
    Ok(Some(kind))
}

fn fingerprint(event: &Event) -> Result<[u8; 32]> {
    let payload = serde_json::to_vec(event).map_err(|_| UNAVAILABLE)?;
    Ok(Sha256::digest(payload).into())
}

fn validate_event(sender: &str, event: &Event) -> Result<()> {
    let request_required = event.kind.is_request() || event.kind == EventKind::RequestResolved;
    let valid_event_id = |value: &str| {
        if event.trace_id.is_some() {
            valid_identifier(value)
        } else {
            valid_native_id(value, 200)
        }
    };
    let valid = valid_identifier(sender)
        && valid_event_id(&event.event_id)
        && valid_scope(event)
        && (match event.request_id.as_deref() {
            Some(id) => request_required && valid_event_id(id),
            None => !request_required,
        })
        && event
            .source_time_unix_ns
            .is_none_or(|time| time <= i64::MAX as u64)
        && event.sequence.map_or(
            event.trace_id.is_some() && event.kind == EventKind::TurnCompleted,
            |sequence| (1..=i64::MAX as u64).contains(&sequence),
        );
    valid
        .then_some(())
        .ok_or_else(|| "Invalid native lifecycle event".into())
}

fn valid_scope(event: &Event) -> bool {
    match event.trace_id.as_deref() {
        Some(trace) => {
            hex_id(trace, 32) && (event.turn_id.is_empty() || hex_id(&event.turn_id, 32))
        }
        None => valid_native_id(&event.turn_id, 512),
    }
}

fn valid_native_id(value: &str, max: usize) -> bool {
    value.len() <= max
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

pub fn valid_identifier(value: &str) -> bool {
    (1..=512).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
}

pub fn hex_id(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "agenturm-native-outbox-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample(kind: EventKind, sequence: u64) -> Event {
        Event {
            event_id: format!("event-{sequence}"),
            sequence: Some(sequence),
            kind,
            trace_id: Some("a".repeat(32)),
            turn_id: "b".repeat(32),
            request_id: (kind.is_request() || kind == EventKind::RequestResolved)
                .then(|| "request".into()),
            source_time_unix_ns: Some(0),
        }
    }

    #[test]
    fn ambiguous_reply_preserves_immutable_event_and_retries_before_later_events() {
        let scratch = Scratch::new();
        let path = scratch.0.join("outbox.sqlite");
        let owner = fs::metadata(&scratch.0).unwrap().uid();
        let binding = [1; 32];
        let first = sample(EventKind::ApprovalRequested, 1);
        let mut outbox = NativeOutbox::open(&path, owner).unwrap();
        assert!(outbox.enqueue("sender", &binding, first.clone()).unwrap());
        let mut attempted = None;
        assert!(
            outbox
                .flush("sender", &binding, |row| {
                    attempted = Some(row.clone());
                    Err("lost reply".into())
                })
                .is_err()
        );
        drop(outbox);
        let mut outbox = NativeOutbox::open(&path, owner).unwrap();
        assert!(!outbox.enqueue("sender", &binding, first.clone()).unwrap());
        assert!(
            outbox
                .flush("sender", &[7; 32], |_| panic!(
                    "different binding cannot dispatch"
                ))
                .is_err()
        );
        let mut wrong = sample(EventKind::RequestResolved, 2);
        wrong.turn_id = "c".repeat(32);
        assert!(outbox.enqueue("sender", &binding, wrong).is_err());
        let mut wrong = sample(EventKind::RequestResolved, 2);
        wrong.request_id = Some("other-request".into());
        assert!(outbox.enqueue("sender", &binding, wrong).is_err());
        let resolution = sample(EventKind::RequestResolved, 2);
        assert!(
            outbox
                .enqueue("sender", &binding, resolution.clone())
                .unwrap()
        );
        let mut delivered = Vec::new();
        assert_eq!(
            outbox.flush("sender", &binding, |row| {
                delivered.push(row.clone());
                Ok(())
            }),
            Ok(2)
        );
        assert_eq!(delivered[0], attempted.unwrap());
        assert_eq!(delivered[1].event, resolution);
        assert_eq!(
            delivered[1].request_kind,
            Some(EventKind::ApprovalRequested)
        );
        assert_eq!(delivered[0].event.source_time_unix_ns, Some(0));
        assert!(delivered[0].received_at_unix_ns > 0);
        assert_eq!(
            outbox.flush("sender", &binding, |_| panic!("confirmed event replayed")),
            Ok(0)
        );
        let mut conflict = first;
        conflict.kind = EventKind::ReplyRequested;
        assert!(outbox.enqueue("sender", &binding, conflict).is_err());
        assert!(
            outbox
                .enqueue("sender", &binding, sample(EventKind::ReplyRequested, 3))
                .is_err()
        );
        assert!(
            outbox
                .enqueue("sender", &binding, sample(EventKind::RequestResolved, 3))
                .is_err()
        );
    }

    #[test]
    fn completion_transport_order_is_assigned_once_and_never_revives_confirmed_identity() {
        let scratch = Scratch::new();
        let path = scratch.0.join("outbox.sqlite");
        let owner = fs::metadata(&scratch.0).unwrap().uid();
        let mut outbox = NativeOutbox::open(&path, owner).unwrap();
        let binding = [2; 32];
        let mut first = sample(EventKind::TurnCompleted, 1);
        first.sequence = None;
        let mut second = sample(EventKind::TurnCompleted, 2);
        second.sequence = None;
        assert!(outbox.enqueue("codex", &binding, first.clone()).unwrap());
        assert!(outbox.enqueue("codex", &binding, second).unwrap());
        let mut order = Vec::new();
        assert_eq!(
            outbox.flush("codex", &binding, |row| {
                order.push(row.sequence);
                Ok(())
            }),
            Ok(2)
        );
        assert_eq!(order, [1, 2]);
        assert!(!outbox.enqueue("codex", &binding, first).unwrap());
        assert!(
            outbox
                .enqueue("codex", &binding, sample(EventKind::TurnStarted, 1))
                .is_err()
        );
    }

    #[test]
    fn an_exact_native_turn_needs_no_synthetic_trace_id() {
        let scratch = Scratch::new();
        let path = scratch.0.join("outbox.sqlite");
        let owner = fs::metadata(&scratch.0).unwrap().uid();
        let mut outbox = NativeOutbox::open(&path, owner).unwrap();
        let binding = [6; 32];
        let mut event = sample(EventKind::ReplyRequested, 1);
        event.turn_id = "turn/native-1".into();
        event.trace_id = None;
        assert!(outbox.enqueue("sender", &binding, event.clone()).unwrap());
        assert_eq!(
            outbox.flush("sender", &binding, |queued| {
                assert_eq!(queued.event, event);
                Ok(())
            }),
            Ok(1)
        );
        let mut resolution = sample(EventKind::RequestResolved, 2);
        resolution.trace_id = None;
        resolution.turn_id = event.turn_id;
        assert!(outbox.enqueue("sender", &binding, resolution).unwrap());
    }

    #[test]
    fn opening_outbox_refuses_unsafe_files_and_foreign_sqlite_schema() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let scratch = Scratch::new();
        let path = scratch.0.join("history.sqlite");
        let owner = fs::metadata(&scratch.0).unwrap().uid();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        use std::io::Write;
        file.write_all(b"unmanaged bytes").unwrap();
        assert!(NativeOutbox::open(&path, owner).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"unmanaged bytes");

        let link = scratch.0.join("foreign-link.sqlite");
        symlink(&path, &link).unwrap();
        assert!(NativeOutbox::open(&link, owner).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(NativeOutbox::open(&path, owner).is_err());

        let foreign = scratch.0.join("foreign.sqlite");
        drop(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&foreign)
                .unwrap(),
        );
        let db = Connection::open(&foreign).unwrap();
        db.execute_batch("CREATE TABLE foreign_data (some_key TEXT);")
            .unwrap();
        assert!(NativeOutbox::open(&foreign, owner).is_err());
        let count: u64 = db
            .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
