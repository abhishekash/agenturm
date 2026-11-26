#![cfg(target_os = "linux")]

use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationQuery,
    ObservationStore,
};
use agenturm_store::Store;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAY_NS: u64 = 86_400_000_000_000;

#[test]
fn standalone_receiver_applies_persisted_age_policy_while_idle_and_respects_disable() {
    let root = Scratch::new();
    let db = root.0.join("history.sqlite");
    let mut store = Store::open(&db).unwrap();
    let now = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    )
    .unwrap();
    let row = |event: &str, time| Observation {
        harness_id: ExtensionId::parse("harness.test").unwrap(),
        event_id: event.into(),
        revision: 0,
        identity: ObservationIdentity::session("fixture"),
        observed_at_unix_ns: time,
        kind: ObservationKind::SessionStarted,
    };
    let rows = [
        row("old", now - 2 * DAY_NS),
        row("recent", now - 1_000_000_000),
    ];
    store.commit("harness.test", None, "one", &rows).unwrap();
    drop(store);
    let policy = json(run(&db, &["set", "--max-age-days", "1"]));
    assert_eq!(policy["policy"]["max_age_ns"], DAY_NS);
    let before = std::fs::read(&db).unwrap();
    assert_eq!(json(run(&db, &["status"])), policy);
    assert_eq!(
        std::fs::read(&db).unwrap(),
        before,
        "status must not write the history"
    );
    {
        let _receiver = Receiver::start(&db);
        let saved = Store::open_read_only(&db).unwrap();
        let page = saved
            .query_observations(&ObservationQuery::latest(10))
            .unwrap();
        assert_eq!(page.observations.len(), 1);
        assert_eq!(page.observations[0].event_id, "recent");
        assert_eq!(
            saved.source_cursor("harness.test").unwrap().as_deref(),
            Some("one")
        );
    }
    let mut replay = Store::open(&db).unwrap();
    assert_eq!(
        replay
            .commit("harness.test", None, "one", &rows)
            .unwrap()
            .inserted,
        0
    );
    drop(replay);
    // A much shorter age would remove the remaining row. Disabling it must
    // survive another receiver start and its first idle maintenance tick.
    json(run(&db, &["set", "--max-age-days", "0.000000001"]));
    let disabled = json(run(&db, &["disable"]));
    assert!(disabled["policy"]["max_age_ns"].is_null());
    {
        let _receiver = Receiver::start(&db);
        let saved = Store::open_read_only(&db).unwrap();
        assert_eq!(
            saved
                .query_observations(&ObservationQuery::latest(10))
                .unwrap()
                .observations
                .len(),
            1
        );
    }
}

#[test]
fn invalid_retention_options_and_read_only_status_do_not_create_history() {
    let root = Scratch::new();
    let db = root.0.join("absent.sqlite");
    for args in [
        vec!["status"],
        vec!["run"],
        vec!["set", "--max-age-days", "NaN"],
        vec!["set", "--max-age-days", "0"],
        vec!["set", "--max-age-days", "1e300"],
        vec!["disable", "--max-age-days", "1"],
    ] {
        assert!(!run(&db, &args).status.success());
        assert!(!db.exists());
    }
    let disabled = json(run(&db, &["disable"]));
    assert!(disabled["policy"]["max_age_ns"].is_null());
    let mut store = Store::open(&db).unwrap();
    store.initialize_managed_retention().unwrap();
    assert_eq!(
        store
            .automatic_retention_status()
            .unwrap()
            .policy
            .unwrap()
            .max_age_ns,
        None
    );
}

fn run(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .arg("retention")
        .args(args)
        .arg("--db")
        .arg(db)
        .output()
        .unwrap()
}
fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

struct Receiver {
    child: Child,
    // Keep the pipe open after reading the address. Dropping it immediately
    // races the receiver's next status print and can terminate the child with
    // BrokenPipe before the readiness request completes.
    _stdout: BufReader<std::process::ChildStdout>,
}
impl Receiver {
    fn start(db: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agenturm"))
            .args([
                "serve-traces",
                "--listen",
                "127.0.0.1:0",
                "--source-id",
                "otlp.retention",
                "--db",
            ])
            .arg(db)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut receiver = Self {
            child,
            _stdout: stdout,
        };
        let mut line = String::new();
        receiver._stdout.read_line(&mut line).unwrap();
        let address = line
            .trim()
            .strip_prefix("Listening for OTLP traces at http://")
            .unwrap()
            .strip_suffix("/v1/traces")
            .unwrap()
            .parse::<SocketAddr>()
            .unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let body = r#"{"resourceSpans":[]}"#;
        write!(stream, "POST /v1/traces HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut response = String::new();
        stream.take(4096).read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        receiver
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "agenturm-retention-cli-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
