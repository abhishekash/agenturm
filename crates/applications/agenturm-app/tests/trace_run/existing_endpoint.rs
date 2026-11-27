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

fn launch(directory: &Path, endpoint: &str, token: &Path, mode: &str, extra: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agenturm"));
    command
        .current_dir(directory)
        .env("AGENTURM_TEST_TRACE_PRODUCER", mode)
        .env(
            "AGENTURM_TEST_TRACE_READY",
            directory.join("producer-ready"),
        )
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
        .arg("--")
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "native_otlp_producer", "--nocapture"]);
    command
}

#[test]
fn existing_authenticated_receiver_owns_the_source_and_retains_the_explicit_session() {
    let scratch = Scratch::new();
    let receiver = Listener::start(&scratch.0, true);
    let mut child = launch(
        &scratch.0,
        &receiver.endpoint,
        &receiver.token_file,
        "exit",
        &[],
    )
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("native-stderr"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(BEARER));
    assert!(!scratch.0.join("state").exists());
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
    assert_eq!(identity.session_id.as_deref(), Some("launched-session"));
    assert!(identity.turn_id.is_none() && identity.worker_id.is_none());
    assert!(rows.iter().all(|row| &row.identity == identity));
    assert!(rows.iter().any(|row| matches!(&row.kind, ObservationKind::TraceSpan(span) if span.target_source_id.is_none())));

    let forged = scratch.0.join("wrong.token");
    fs::write(&forged, "a".repeat(64)).unwrap();
    fs::set_permissions(&forged, fs::Permissions::from_mode(0o600)).unwrap();
    let denied = launch(&scratch.0, &receiver.endpoint, &forged, "exit", &[])
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
    let no_auth = launch(
        &scratch.0,
        &receiver.endpoint,
        &receiver.token_file,
        "exit",
        &[],
    )
    .output()
    .unwrap();
    assert!(!no_auth.status.success() && no_auth.stdout.is_empty());
    assert!(String::from_utf8_lossy(&no_auth.stderr).contains("must require producer bearer"));
    for extra in [
        &["--db", "unwanted.sqlite"][..],
        &["--source-id", "otlp.wrong"],
        &["--payload-scope", "standalone"],
    ] {
        let output = launch(
            &scratch.0,
            &receiver.endpoint,
            &receiver.token_file,
            "exit",
            extra,
        )
        .output()
        .unwrap();
        assert!(!output.status.success() && output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be combined"));
    }
    let removed_flag = launch(
        &scratch.0,
        &receiver.endpoint,
        &receiver.token_file,
        "exit",
        &["--capture-content"],
    )
    .output()
    .unwrap();
    assert!(!removed_flag.status.success());
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
        let output = launch(&scratch.0, endpoint, &receiver.token_file, "exit", &[])
            .output()
            .unwrap();
        assert!(!output.status.success() && output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("literal loopback"),
            "{endpoint}"
        );
    }
    fs::set_permissions(&receiver.token_file, fs::Permissions::from_mode(0o644)).unwrap();
    let unsafe_token = launch(
        &scratch.0,
        &receiver.endpoint,
        &receiver.token_file,
        "exit",
        &[],
    )
    .output()
    .unwrap();
    assert!(!unsafe_token.status.success() && unsafe_token.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unsafe_token.stderr).contains("owner-only regular file"));
}

#[test]
fn existing_receiver_outlives_a_signaled_launch() {
    let scratch = Scratch::new();
    let receiver = Listener::start(&scratch.0, true);
    let mut launcher = ChildGuard(
        launch(
            &scratch.0,
            &receiver.endpoint,
            &receiver.token_file,
            "wait",
            &[],
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap(),
    );
    let ready = scratch.0.join("producer-ready");
    let deadline = Instant::now() + Duration::from_secs(8);
    let producer = loop {
        if let Ok(value) = fs::read_to_string(&ready)
            && value.ends_with('\n')
        {
            break value
                .lines()
                .nth(1)
                .unwrap()
                .parse::<libc::pid_t>()
                .unwrap();
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
