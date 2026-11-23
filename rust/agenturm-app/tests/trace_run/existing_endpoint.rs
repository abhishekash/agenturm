use super::*;
use agenturm_kernel::ObservationKind;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const BEARER: &str = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
const SOURCE: &str = "otlp.existing-receiver";

struct Listener {
    _child: ChildGuard,
    endpoint: String,
    token_file: PathBuf,
    database: PathBuf,
}

impl Listener {
    fn start(root: &Path, authenticated: bool) -> Self {
        let database = root.join("receiver.sqlite");
        let token_file = root.join("producer.token");
        fs::write(&token_file, format!("{BEARER}\n")).unwrap();
        fs::set_permissions(&token_file, fs::Permissions::from_mode(0o600)).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_agenturm"));
        command
            .args(["serve-traces", "--listen", "127.0.0.2:0", "--db"])
            .arg(&database)
            .args(["--source-id", SOURCE]);
        if authenticated {
            command
                .args(["--payload-scope", "standalone", "--token-file"])
                .arg(&token_file);
        }
        let mut child = ChildGuard(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            let _ = send.send(line);
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        });
        let ready = receive.recv_timeout(Duration::from_secs(8)).unwrap();
        let endpoint = ready
            .trim_end()
            .strip_prefix("Listening for OTLP traces at ")
            .unwrap()
            .to_owned();
        Self {
            _child: child,
            endpoint,
            token_file,
            database,
        }
    }
}

fn launch(directory: &Path, endpoint: &str, token: &Path, extra: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agenturm"));
    command
        .current_dir(directory)
        .env("PYTHONPATH", directory.join("sdk"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("HOME", directory)
        .env("XDG_STATE_HOME", directory.join("state"))
        .env(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "http://invalid.example/v1/traces",
        )
        .env("OTEL_EXPORTER_OTLP_HEADERS", "inherited-private-fixture")
        .env("NO_PROXY", "unrelated.example")
        .args(["run", "--endpoint", endpoint, "--token-file"])
        .arg(token)
        .args(extra)
        .args([
            "--instrument",
            "python",
            "--service",
            "exact-launch",
            "--",
            "python3",
            "target.py",
            "literal arg",
        ]);
    command
}

fn sdk_fixture(directory: &Path) {
    let sdk = directory.join("sdk/observatory");
    fs::create_dir_all(&sdk).unwrap();
    fs::write(sdk.join("__init__.py"), "").unwrap();
    fs::write(
        sdk.join("auto.py"),
        r#"
import http.client
import json
import os
import runpy
import sys
from urllib.parse import unquote, urlsplit

class SetupError(Exception):
    pass

def run_agent(target, arguments, *, endpoint, service, module, capture_content, transport,
              session_id, managed_launcher):
    assert sys.flags.safe_path and managed_launcher
    assert transport == "otlp" and not capture_content and not module
    assert service == "exact-launch" and session_id.startswith("launch-")
    assert endpoint == os.environ["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"]
    assert not os.environ.get("OTEL_EXPORTER_OTLP_HEADERS")
    assert "127.0.0.2" in os.environ["NO_PROXY"].split(",")
    location = urlsplit(endpoint)
    assert location.hostname == "127.0.0.2" and location.path == "/v1/traces"
    header = os.environ["OTEL_EXPORTER_OTLP_TRACES_HEADERS"]
    assert os.environ["OTEL_EXPORTER_OTLP_METRICS_HEADERS"] == header
    assert os.environ["OTEL_EXPORTER_OTLP_LOGS_HEADERS"] == header
    key, value = header.split("=", 1)
    assert key == "authorization"
    connection = http.client.HTTPConnection(location.hostname, location.port, timeout=4)
    span = dict(traceId="1"*32, spanId="2"*16, startTimeUnixNano="5", endTimeUnixNano="10",
                attributes=[dict(key="session.id", value=dict(stringValue=session_id))])
    batch = dict(resourceSpans=[dict(scopeSpans=[dict(spans=[span])])])
    connection.request("POST", location.path, json.dumps(batch), {
        "Content-Type": "application/json", "Authorization": unquote(value),
    })
    response = connection.getresponse()
    assert response.status == 200 and response.read() == b"{}"
    connection.close()
    sys.argv = [target, *arguments]
    runpy.run_path(target, run_name="__main__")
"#,
    )
    .unwrap();
    fs::write(directory.join("target.py"), "import sys\nassert sys.argv[1:] == ['literal arg']\nprint('child-observed')\nraise SystemExit(23)\n").unwrap();
}

#[test]
fn existing_authenticated_receiver_owns_the_source_and_retains_the_explicit_session() {
    if !python_available() {
        return;
    }
    let scratch = Scratch::new();
    let receiver = Listener::start(&scratch.0, true);
    let child_dir = scratch.0.join("program");
    sdk_fixture(&child_dir);
    let output = launch(&child_dir, &receiver.endpoint, &receiver.token_file, &[])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23), "{output:?}");
    assert_eq!(output.stdout, b"child-observed\n");
    assert!(!String::from_utf8_lossy(&output.stderr).contains(BEARER));
    assert!(!child_dir.join("state").exists());
    assert!(!child_dir.join("history.sqlite").exists());
    let store = Store::open_read_only(&receiver.database).unwrap();
    let rows = store
        .query_observations(&ObservationQuery::latest(10))
        .unwrap()
        .observations;
    assert_eq!(
        rows.len(),
        2,
        "one span and its classification; no probe admission"
    );
    assert!(rows.iter().all(|row| row.harness_id.as_str() == SOURCE));
    let identity = &rows[0].identity;
    assert!(
        identity
            .session_id
            .as_deref()
            .unwrap()
            .starts_with("launch-")
    );
    assert!(identity.turn_id.is_none() && identity.worker_id.is_none());
    assert!(rows.iter().all(|row| &row.identity == identity));
    assert!(rows.iter().any(|row| matches!(&row.kind, ObservationKind::TraceSpan(span) if span.target_source_id.is_none())));

    let forged = scratch.0.join("wrong.token");
    fs::write(&forged, "a".repeat(64)).unwrap();
    fs::set_permissions(&forged, fs::Permissions::from_mode(0o600)).unwrap();
    let denied = launch(&child_dir, &receiver.endpoint, &forged, &[])
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(denied.stdout.is_empty());
    let message = String::from_utf8_lossy(&denied.stderr);
    assert!(message.contains("authenticated startup probe"));
    assert!(!message.contains(&"a".repeat(64)));
}

#[test]
fn existing_receiver_rejects_unauthenticated_listeners_and_conflicting_flags_before_launch() {
    let scratch = Scratch::new();
    let receiver = Listener::start(&scratch.0, false);
    let no_auth = launch(&scratch.0, &receiver.endpoint, &receiver.token_file, &[])
        .output()
        .unwrap();
    assert!(!no_auth.status.success() && no_auth.stdout.is_empty());
    assert!(String::from_utf8_lossy(&no_auth.stderr).contains("must require producer bearer"));
    for extra in [
        &["--db", "unwanted.sqlite"][..],
        &["--source-id", "otlp.wrong"],
        &["--payload-scope", "standalone"],
        &["--capture-content"],
    ] {
        let output = launch(&scratch.0, &receiver.endpoint, &receiver.token_file, extra)
            .output()
            .unwrap();
        assert!(!output.status.success() && output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be combined"));
    }
    assert!(!scratch.0.join("unwanted.sqlite").exists());
    for endpoint in [
        "http://localhost:4318",
        "http://192.0.2.1:4318",
        "http://127.0.0.1:0",
        "https://127.0.0.1:4318",
        "http://127.0.0.1:4318/other",
        "http://127.0.0.1:4318/v1/traces?x=1",
        "http://[::1%4]:4318",
    ] {
        let output = launch(&scratch.0, endpoint, &receiver.token_file, &[])
            .output()
            .unwrap();
        assert!(!output.status.success() && output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("literal loopback"),
            "{endpoint}"
        );
    }
    fs::set_permissions(&receiver.token_file, fs::Permissions::from_mode(0o644)).unwrap();
    let unsafe_token = launch(&scratch.0, &receiver.endpoint, &receiver.token_file, &[])
        .output()
        .unwrap();
    assert!(!unsafe_token.status.success() && unsafe_token.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unsafe_token.stderr).contains("owner-only regular file"));
}

#[test]
fn existing_receiver_outlives_a_signaled_launch() {
    if !python_available() {
        return;
    }
    let scratch = Scratch::new();
    let receiver = Listener::start(&scratch.0, true);
    let child_dir = scratch.0.join("program");
    sdk_fixture(&child_dir);
    fs::write(
        child_dir.join("target.py"),
        r#"
from pathlib import Path
import os
import time
Path("ready").write_text(str(os.getpid()) + "\n")
while True:
    time.sleep(0.1)
"#,
    )
    .unwrap();
    let mut launcher = ChildGuard(
        launch(&child_dir, &receiver.endpoint, &receiver.token_file, &[])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    let producer = loop {
        if let Ok(value) = fs::read_to_string(child_dir.join("ready"))
            && value.ends_with('\n')
        {
            break value.trim().parse::<libc::pid_t>().unwrap();
        }
        assert!(Instant::now() < deadline && launcher.0.try_wait().unwrap().is_none());
        thread::sleep(Duration::from_millis(20));
    };
    // SAFETY: send the signal to the live launcher child owned by this test.
    assert_eq!(
        unsafe { libc::kill(launcher.0.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = launcher.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(128 + libc::SIGTERM));
    // SAFETY: signal zero checks whether the reaped child PID still exists.
    assert_eq!(unsafe { libc::kill(producer, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    let address = receiver
        .endpoint
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/v1/traces")
        .unwrap()
        .parse()
        .unwrap();
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok());
}
