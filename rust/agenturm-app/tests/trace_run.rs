#![cfg(unix)]

use agenturm_kernel::{ObservationQuery, ObservationStore};
use agenturm_store::Store;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const FIXTURE: &str = "AGENTURM_TEST_TRACE_PRODUCER";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "agenturm-trace-run-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
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

fn launcher(db: &std::path::Path, mode: &str, source: Option<&str>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agenturm"));
    command.arg("run").arg("--db").arg(db);
    if let Some(source) = source {
        command.args(["--source-id", source]);
    }
    command
        .arg("--")
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "native_otlp_producer", "--nocapture"])
        .env(FIXTURE, mode)
        .env(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "https://invalid.example/base",
        )
        .env(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "https://invalid.example/traces",
        )
        .env(
            "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
            "https://invalid.example/metrics",
        )
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc")
        .env("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "http/json")
        .env("OTEL_EXPORTER_OTLP_COMPRESSION", "gzip")
        .env(
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
            "test-header=private-fixture",
        )
        .env(
            "OTEL_EXPORTER_OTLP_HEADERS",
            "another-header=private-fixture",
        )
        .env("OTEL_CONFIG_FILE", "/invalid/old-config")
        .env("OTEL_EXPERIMENTAL_CONFIG_FILE", "/invalid/old-config")
        .env("OTEL_SDK_DISABLED", "true")
        .env("NO_PROXY", "unrelated.example");
    command
}

#[test]
fn launcher_overrides_exporter_and_preserves_stdio_and_exit_status() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let mut child = launcher(&db, "exit", Some("otlp.launch-fixture"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"native-stdin")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(23), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains("native-stdout"));
    assert!(stderr.contains("native-stderr"));
    assert!(!stdout.contains("Listening for OTLP"));
    assert!(!stdout.contains("private-fixture"));
    assert!(!stderr.contains("private-fixture"));
    assert_trace(&db, "otlp.launch-fixture");
}

#[test]
fn launcher_forwards_signal_and_closes_receiver() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let ready = scratch.0.join("producer-ready");
    let child = launcher(&db, "wait", None)
        .env("AGENTURM_TEST_TRACE_READY", &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut child = ChildGuard(child);
    let deadline = Instant::now() + Duration::from_secs(12);
    let state = loop {
        if let Ok(state) = fs::read_to_string(&ready)
            && state.ends_with('\n')
        {
            break state;
        }
        assert!(
            Instant::now() < deadline && child.0.try_wait().unwrap().is_none(),
            "producer did not reach its steady state"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let mut fields = state.lines();
    let endpoint: SocketAddr = fields.next().unwrap().parse().unwrap();
    let producer: libc::pid_t = fields.next().unwrap().parse().unwrap();
    // SAFETY: the PID identifies the test launcher child, which is still supervised.
    assert_eq!(
        unsafe { libc::kill(child.0.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "launcher did not stop after SIGTERM"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(128 + libc::SIGTERM));
    assert!(TcpStream::connect_timeout(&endpoint, Duration::from_millis(250)).is_err());
    // SAFETY: signal zero checks liveness without delivering a signal.
    assert_eq!(unsafe { libc::kill(producer, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert_trace(&db, "otlp.run");
}

#[test]
fn assigns_unscoped_spans_to_the_actual_command_launch() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let output = launcher(&db, "unscoped", None).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let store = Store::open_read_only(&db).unwrap();
    let rows = store.observation_page(2).unwrap().observations;
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0]
            .identity
            .session_id
            .as_deref()
            .unwrap()
            .starts_with("launch-")
    );
    assert!(
        matches!(&rows[0].kind, agenturm_kernel::ObservationKind::TraceSpan(span)
        if span.target_source_id.is_none())
    );
}

fn assert_trace(db: &std::path::Path, source: &str) {
    let store = Store::open_read_only(db).unwrap();
    let mut query = ObservationQuery::latest(10);
    query.harness_id = Some(source.into());
    let page = ObservationStore::query_observations(&store, &query).unwrap();
    assert_eq!(page.observations.len(), 1);
    let row = &page.observations[0];
    assert_eq!(row.identity.session_id.as_deref(), Some("launched-session"));
    assert!(
        matches!(&row.kind, agenturm_kernel::ObservationKind::TraceSpan(span)
        if span.trace_id == "03".repeat(16) && span.span_id == "04".repeat(8))
    );
    assert!(!String::from_utf8_lossy(&fs::read(db).unwrap()).contains("private fixture span"));
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// This child is a native producer. It follows the OTLP environment selection
/// and sends a binary ExportTraceServiceRequest over HTTP.
#[test]
fn native_otlp_producer() {
    let Some(mode) = std::env::var_os(FIXTURE) else {
        return;
    };
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").unwrap();
    let base = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").unwrap();
    assert_eq!(endpoint, format!("{base}/v1/traces"));
    let address: SocketAddr = base.strip_prefix("http://").unwrap().parse().unwrap();
    assert!(address.ip().is_loopback());
    assert_ne!(address.port(), 0);
    for key in [
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
    ] {
        assert_eq!(std::env::var(key).unwrap(), "http/protobuf");
    }
    for key in [
        "OTEL_EXPORTER_OTLP_COMPRESSION",
        "OTEL_EXPORTER_OTLP_TRACES_COMPRESSION",
    ] {
        assert_eq!(std::env::var(key).unwrap(), "none");
    }
    for key in [
        "OTEL_EXPORTER_OTLP_HEADERS",
        "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
        "OTEL_CONFIG_FILE",
        "OTEL_EXPERIMENTAL_CONFIG_FILE",
    ] {
        assert!(std::env::var_os(key).is_none());
    }
    assert_eq!(std::env::var("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT").unwrap(), format!("http://{address}/v1/metrics"));
    assert_eq!(std::env::var("OTEL_EXPORTER_OTLP_METRICS_PROTOCOL").unwrap(), "http/protobuf");
    assert_eq!(std::env::var("OTEL_SDK_DISABLED").unwrap(), "false");
    assert_eq!(std::env::var("OTEL_TRACES_EXPORTER").unwrap(), "otlp");
    assert_eq!(std::env::var("OTEL_METRICS_EXPORTER").unwrap(), "otlp");
    assert_eq!(std::env::var("OTEL_LOGS_EXPORTER").unwrap(), "none");
    for key in ["NO_PROXY", "no_proxy"] {
        let value = std::env::var(key).unwrap();
        assert!(value.split(',').any(|item| item == "127.0.0.1"));
        assert!(value.split(',').any(|item| item == "unrelated.example"));
    }

    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(8)))
        .unwrap();
    let export = export(mode != "unscoped");
    write!(
        socket,
        "POST /v1/traces HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        export.len()
    ).unwrap();
    socket.write_all(&export).unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("Content-Type: application/x-protobuf"));
    if mode == "exit" {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        assert_eq!(input, "native-stdin");
        println!("native-stdout");
        eprintln!("native-stderr");
        std::process::exit(23);
    } else if mode == "wait" {
        let ready = std::env::var_os("AGENTURM_TEST_TRACE_READY").unwrap();
        fs::write(ready, format!("{address}\n{}\n", std::process::id())).unwrap();
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    } else if mode == "unscoped" {
        return;
    }
    panic!("unexpected trace producer fixture mode");
}

fn export(scoped: bool) -> Vec<u8> {
    let mut span = message(1, &[3; 16]);
    span.extend(message(2, &[4; 8]));
    span.extend(message(5, b"private fixture span"));
    span.push(7 << 3 | 1);
    span.extend(10_u64.to_le_bytes());
    span.push(8 << 3 | 1);
    span.extend(20_u64.to_le_bytes());
    if scoped {
        let mut identity = message(1, b"session.id");
        identity.extend(message(2, &message(1, b"launched-session")));
        span.extend(message(9, &identity));
    }
    message(1, &message(2, &message(2, &span)))
}

fn message(number: u8, fields: &[u8]) -> Vec<u8> {
    assert!(fields.len() < 128);
    let mut encoded = vec![number << 3 | 2, fields.len() as u8];
    encoded.extend(fields);
    encoded
}
