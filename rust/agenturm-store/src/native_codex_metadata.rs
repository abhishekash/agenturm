//! Read-only selected Codex metadata. The caller authorizes the exact Codex home.

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const MAX_FIELD_BYTES: usize = 4096;
const MAX_THREADS: usize = 256;
const MAX_COLUMNS: usize = 128;

#[derive(Default)]
pub struct CodexThreadMetadata {
    pub rollout_path: Option<String>,
    pub title: Option<String>,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub agent_nickname: Option<String>,
    pub agent_role: Option<String>,
    pub agent_path: Option<String>,
    pub model_provider: Option<String>,
    pub updated_at_ms: Option<i64>,
}

/// Returns selected columns only. SQLite is denied writable file and
/// shared-memory handles. An unavailable WAL read mark is a normal failure.
pub fn read_codex_thread_metadata(
    path: &Path,
    sessions: &BTreeSet<String>,
) -> rusqlite::Result<BTreeMap<String, CodexThreadMetadata>> {
    if !path.is_absolute() || sessions.len() > MAX_THREADS {
        return Err(rusqlite::Error::InvalidPath(path.into()));
    }
    let mut uri = String::from("file:");
    for byte in path.as_os_str().as_bytes() {
        use std::fmt::Write;
        write!(uri, "%{byte:02X}").expect("writing to String cannot fail");
    }
    uri.push_str("?mode=ro&readonly_shm=1");
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let mut connection = Connection::open_with_flags(uri, flags)?;
    connection.busy_timeout(std::time::Duration::from_millis(60))?;
    connection.execute_batch(
        "PRAGMA trusted_schema=OFF; PRAGMA query_only=ON; PRAGMA temp_store=MEMORY;",
    )?;
    let transaction = connection.transaction()?;
    let ordinary_table = transaction
        .query_row(
            "SELECT 1 FROM pragma_table_list WHERE schema='main' AND name='threads' AND type='table'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !ordinary_table {
        return Ok(BTreeMap::new());
    }
    let mut columns = transaction
        .prepare("SELECT name, pk, hidden FROM pragma_table_xinfo('threads','main') LIMIT ?1")?;
    let mut column_rows = columns.query([MAX_COLUMNS + 1])?;
    let mut readable = BTreeSet::new();
    let mut indexed_id = false;
    while let Some(row) = column_rows.next()? {
        let name: String = row.get(0)?;
        let pk: i32 = row.get(1)?;
        let hidden: i32 = row.get(2)?;
        if hidden == 0 {
            indexed_id |= name == "id" && pk == 1;
            readable.insert(name);
        }
        if readable.len() > MAX_COLUMNS {
            return Ok(BTreeMap::new());
        }
    }
    if !indexed_id || !readable.contains("rollout_path") {
        return Ok(BTreeMap::new());
    }
    let fields = [
        "rollout_path",
        "title",
        "name",
        "cwd",
        "agent_nickname",
        "agent_role",
        "agent_path",
        "model_provider",
        "updated_at_ms",
        "updated_at",
    ];
    const DISPLAY_TITLE: &str = "
        CASE WHEN typeof(title) = 'text' AND length(trim(title)) > 0 AND
            (first_user_message IS NULL OR (
                typeof(first_user_message) = 'text' AND (
                    length(trim(first_user_message)) = 0 OR (
                        instr(trim(first_user_message), trim(title)) <> 1 AND
                        instr(trim(title), trim(first_user_message)) <> 1
                    )
                )
            ))
        THEN title END";
    let selected = fields.map(|name| match name {
        "title" if readable.contains("title") && readable.contains("first_user_message") => {
            DISPLAY_TITLE
        }
        "title" => "NULL",
        name if readable.contains(name) => name,
        _ => "NULL",
    });
    let sql = format!(
        "SELECT {} FROM main.threads WHERE id=?1 LIMIT 2",
        selected.join(", ")
    );
    let mut rows = transaction.prepare(&sql)?;
    let mut result = BTreeMap::new();
    for session in sessions {
        let mut query = rows.query([session])?;
        let value = match query.next()? {
            Some(row) => {
                let get = |column| {
                    row.get_ref(column)
                        .ok()?
                        .as_str()
                        .ok()
                        .filter(|value| value.len() <= MAX_FIELD_BYTES)
                        .map(str::to_owned)
                };
                CodexThreadMetadata {
                    rollout_path: get(0),
                    title: get(1),
                    name: get(2),
                    cwd: get(3),
                    agent_nickname: get(4),
                    agent_role: get(5),
                    agent_path: get(6),
                    model_provider: get(7),
                    updated_at_ms: row.get::<_, Option<i64>>(8).ok().flatten().or_else(|| {
                        row.get::<_, Option<i64>>(9)
                            .ok()
                            .flatten()?
                            .checked_mul(1000)
                    }),
                }
            }
            None => continue,
        };
        if query.next()?.is_none() {
            result.insert(session.clone(), value);
        }
    }
    Ok(result)
}

impl super::Store {
    /// Ensure a local roster metadata projection still belongs to this exact
    /// retained read incarnation and a valid history watermark.
    pub fn verify_native_context_snapshot(
        &self,
        snapshot: agenturm_kernel::ReadSnapshot,
    ) -> super::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        super::retention::resolve_read_snapshot(&transaction, Some(snapshot)).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn selected_codex_metadata_reads_current_wal_without_writing_native_files() {
        let prefix = std::env::temp_dir().join(format!(
            "agenturm-codex-native-context-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&prefix).unwrap();
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let scratch = Scratch(prefix);
        let path = scratch.0.join("state #5.sqlite");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
             CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, title TEXT,
               name TEXT, cwd TEXT, agent_nickname TEXT, agent_role TEXT, agent_path TEXT,
               model_provider TEXT, updated_at_ms INTEGER, first_user_message TEXT);
             INSERT INTO threads VALUES ('exact', '/native/sessions/known.jsonl', 'native title',
               NULL, '/work/project', 'Worker A', 'Backend', 'root/a', 'local', 1000, 'private body');
             INSERT INTO threads VALUES ('unlinked', '/other/session.jsonl', 'Unlinked title',
               NULL, NULL, NULL, NULL, NULL, NULL, 1000, 'other body');
             UPDATE threads SET name='Renamed session', updated_at_ms=2000 WHERE id='exact';
             INSERT INTO threads VALUES ('raw-title', '/native/sessions/raw-title.jsonl',
               'A private initial query', NULL, NULL, NULL, NULL, NULL, NULL, 1000,
               'A private initial query with more text');"
        ).unwrap();
        // A current native read mark is available in the existing SHM.
        let reader = writer.unchecked_transaction().unwrap();
        let _: String = reader
            .query_row("SELECT name FROM threads WHERE id='exact'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let paths = [
            path.clone(),
            scratch.0.join("state #5.sqlite-wal"),
            scratch.0.join("state #5.sqlite-shm"),
        ];
        let bytes = || paths.each_ref().map(|path| fs::read(path).unwrap());
        let original = bytes();
        let projected =
            read_codex_thread_metadata(&path, &["exact".to_owned(), "raw-title".to_owned()].into())
                .unwrap();
        assert_eq!(projected.len(), 2);
        assert!(projected["raw-title"].title.is_none());
        let row = &projected["exact"];
        assert_eq!(row.name.as_deref(), Some("Renamed session"));
        assert_eq!(row.agent_role.as_deref(), Some("Backend"));
        assert_eq!(row.updated_at_ms, Some(2000));
        assert!(
            bytes() == original,
            "native SQLite files changed while checking metadata"
        );
        assert!(
            reader
                .query_row("SELECT count(*) FROM threads", [], |row| row
                    .get::<_, i64>(0))
                .is_ok()
        );
    }
}
