use agenturm_app::{AgenturmApp, execute_inspect};
use agenturm_kernel::ObservationQuery;
use agenturm_lifecycle_extension_fixture::LifecycleExtensionPlugin;
use agenturm_store::Store;
use agenturm_ui::UiModel;
use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
#[path = "fixtures/crash_recovery.rs"]
mod crash_recovery;

fn temporary_directory() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agenturm-app-collect-e2e-{}-{nonce}",
        std::process::id()
    ))
}

#[cfg(unix)]
fn standalone_harness_paths() -> Option<(String, String)> {
    let (Ok(host), Ok(adapter)) = (
        std::env::var("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var("AGENTURM_RUNTIME_STANDALONE_ADAPTER"),
    ) else {
        eprintln!("skipping standalone harness test: set plugin host and standalone adapter paths");
        return None;
    };
    Some((host, adapter))
}

#[cfg(unix)]
fn standalone_collection_config(
    root: &std::path::Path,
    host: &str,
    adapter: &str,
    observation_format: &str,
    inputs: &[(&str, &str)],
) -> PathBuf {
    const PLUGIN_ID: &str = "fixture.standalone-harness";
    fs::create_dir_all(root).expect("create standalone fixture directory");
    let hash = Command::new("sha256sum")
        .arg(adapter)
        .output()
        .expect("hash independently built adapter");
    assert!(hash.status.success());
    let sha256 = std::str::from_utf8(&hash.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    let catalog = serde_json::json!({
        "version": 1,
        "plugins": [{
            "plugin_id": PLUGIN_ID,
            "adapter": adapter,
            "sha256": sha256,
            "protocol": 1,
            "features": ["source-adapter/1", "configured-root/1"],
            "observation_format": observation_format,
        }]
    });
    fs::write(
        root.join("plugins.json"),
        serde_json::to_vec(&catalog).unwrap(),
    )
    .expect("write standalone plugin catalog");
    let sources: Vec<_> = inputs
        .iter()
        .map(|(source_id, input)| {
            let source_root = root.join(source_id);
            fs::write(&source_root, input).expect("write native fixture input");
            serde_json::json!({
                "plugin_id": PLUGIN_ID,
                "source_root": source_root,
                "source_id": source_id,
            })
        })
        .collect();
    let config = serde_json::json!({
        "version": 2,
        "plugin_host": host,
        "plugin_catalog": "plugins.json",
        "sources": sources,
    });
    let path = root.join("sources.json");
    fs::write(&path, serde_json::to_vec(&config).unwrap())
        .expect("write standalone collection config");
    path
}

#[cfg(unix)]
fn collect_once(config: &std::path::Path, database: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .arg("collect")
        .arg("--config")
        .arg(config)
        .arg("--db")
        .arg(database)
        .output()
        .expect("run configured collection CLI")
}

#[cfg(unix)]
#[test]
fn standalone_harness_commits_without_admitting_malformed_or_failed_sources() {
    let Some((host, adapter)) = standalone_harness_paths() else {
        return;
    };
    let root = temporary_directory();
    let config = standalone_collection_config(
        &root,
        &host,
        &adapter,
        "agenturm-observations/1",
        &[
            ("harness.crash", "crash"),
            ("harness.malformed", "malformed"),
            ("harness.standalone", "tokens=42\n"),
        ],
    );
    let database = root.join("agenturm.sqlite");
    let output = collect_once(&config, &database);
    let summary = std::str::from_utf8(&output.stdout).expect("collection output is UTF-8");
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        summary.contains("3 sources checked; 2 source failures; 0 global failures"),
        "{summary}"
    );
    assert!(
        summary.contains("harness.crash: AdmissionPoll: plugin host closed its data pipe"),
        "{summary}"
    );
    assert!(
        summary.contains(
            "harness.malformed: AdmissionPoll: source batch rejected: invalid observation schema"
        ),
        "{summary}"
    );
    assert!(
        summary.contains("harness.standalone: committed 1 observation(s)"),
        "{summary}"
    );
    assert_eq!(summary.matches("ACK sent=true").count(), 1, "{summary}");

    let snapshot = execute_inspect(&database).expect("reopen committed collection history");
    assert!(snapshot.failures.is_empty());
    assert_eq!(snapshot.observations.len(), 1);
    let observation = &snapshot.observations[0];
    assert_eq!(observation.harness_id.as_str(), "harness.standalone");
    assert_eq!(observation.event_id, "standalone-event-1");
    assert_eq!(observation.observed_at_unix_ns, 9_007_199_254_740_993);
    assert_eq!(
        observation.identity.session_id.as_deref(),
        Some("standalone-session")
    );
    assert!(matches!(
        observation.kind,
        agenturm_kernel::ObservationKind::Usage {
            input_tokens: Some(42),
            ..
        }
    ));

    let store = Store::open_read_only(&database).expect("reopen checkpoints");
    assert_eq!(
        store
            .source_cursor("harness.standalone")
            .unwrap()
            .as_deref(),
        Some("standalone-record-1")
    );
    assert_eq!(store.source_cursor("harness.crash").unwrap(), None);
    assert_eq!(store.source_cursor("harness.malformed").unwrap(), None);
    drop(store);
    fs::remove_dir_all(root).expect("remove standalone fixture directory");
}

#[cfg(unix)]
#[test]
fn catalog_rejects_unsupported_observation_format_before_creating_database() {
    let Some((host, adapter)) = standalone_harness_paths() else {
        return;
    };
    let root = temporary_directory();
    let config = standalone_collection_config(
        &root,
        &host,
        &adapter,
        "agenturm-observations/999",
        &[("harness.standalone", "tokens=42\n")],
    );
    let database = root.join("agenturm.sqlite");
    let output = collect_once(&config, &database);
    let error = std::str::from_utf8(&output.stderr).expect("collection error is UTF-8");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        error.contains("unsupported observation format for fixture.standalone-harness"),
        "{error}"
    );
    assert!(!database.exists());
    fs::remove_dir_all(root).expect("remove standalone fixture directory");
}

#[cfg(unix)]
#[test]
fn version_two_catalog_runs_a_pinned_real_pi_process() {
    let (Ok(plugin_host), Ok(pi_adapter)) = (
        std::env::var("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var("AGENTURM_RUNTIME_PI_ADAPTER"),
    ) else {
        eprintln!("skipping real catalog test: set plugin host and Pi adapter paths");
        return;
    };
    let hash = Command::new("sha256sum")
        .arg(&pi_adapter)
        .output()
        .expect("hash Pi adapter")
        .stdout;
    let sha256 = String::from_utf8(hash)
        .expect("sha256sum output is UTF-8")
        .split_whitespace()
        .next()
        .expect("sha256sum emits a digest")
        .to_owned();
    let root = temporary_directory();
    fs::create_dir_all(&root).expect("create fixture directory");
    let source = root.join("pi.jsonl");
    fs::write(
        &source,
        concat!(
            "{\"type\":\"session\",\"id\":\"catalog-session\"}\n",
            "{\"type\":\"message\",\"id\":\"user-1\",\"timestamp\":\"2026-11-20T11:59:59Z\",\"message\":{\"role\":\"user\",\"content\":\"private prompt must not reach the extension\"}}\n",
            "{\"type\":\"message\",\"id\":\"assistant-1\",\"timestamp\":\"2026-11-20T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"stopReason\":\"error\",\"usage\":{\"input\":8,\"output\":3,\"cost\":{\"output\":7,\"total\":0.001230000000000001}},\"content\":[{\"type\":\"toolCall\",\"id\":\"call-1\",\"name\":\"read\"}]}}\n"
        ),
    )
    .expect("write Pi fixture");
    let catalog_path = root.join("plugins.json");
    let catalog = serde_json::json!({
        "version": 1,
        "plugins": [{
            "plugin_id": "example.rust-pi-harness",
            "adapter": pi_adapter,
            "sha256": sha256,
            "protocol": 1,
            "features": ["source-adapter/1", "configured-root/1"]
        }]
    });
    fs::write(&catalog_path, serde_json::to_vec(&catalog).unwrap()).expect("write plugin catalog");
    let config_path = root.join("sources.json");
    let config = serde_json::json!({
        "version": 2,
        "plugin_host": plugin_host,
        "plugin_catalog": "plugins.json",
        "sources": [{
            "plugin_id": "example.rust-pi-harness",
            "source_root": source,
            "source_id": "harness.pi"
        }]
    });
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).expect("write config");
    let database = root.join("agenturm.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args([
            "collect",
            "--config",
            config_path.to_str().unwrap(),
            "--db",
            database.to_str().unwrap(),
        ])
        .output()
        .expect("run catalog-backed collection");
    assert!(
        output.status.success(),
        "catalog-backed process failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("harness.pi: committed 7 observation(s)")
    );
    let snapshot = execute_inspect(&database).expect("reopen catalog-backed history");
    assert_eq!(snapshot.observations.len(), 7);
    let cost = snapshot
        .observations
        .iter()
        .find(|row| matches!(row.kind, agenturm_kernel::ObservationKind::ReportedCost(_)))
        .expect("durable Pi assistant cost");
    let agenturm_kernel::ObservationKind::ReportedCost(amount) = &cost.kind else {
        unreachable!();
    };
    let origin = amount.native_origin.expect("native assistant provenance");
    assert_eq!(origin.basis, agenturm_kernel::NativeCostBasis::Estimate);
    assert!(origin.rounded);
    assert_eq!((&*amount.currency, amount.nanounits), ("USD", 1_230_000));
    assert_eq!(cost.identity.turn_id.as_deref(), Some("user-1"));
    assert!(snapshot.observations.iter().any(|row| {
        Some(row.event_id.as_str()) == cost.identity.operation_id.as_deref()
            && row.revision == cost.revision
            && row.identity.turn_id == cost.identity.turn_id
            && matches!(&row.kind, agenturm_kernel::ObservationKind::Message { role } if role == "assistant")
    }));
    assert!(snapshot.observations.iter().any(|observation| {
        observation.identity.session_id.as_deref() == Some("catalog-session")
            && observation.kind
                == agenturm_kernel::ObservationKind::Lifecycle {
                    state: "interrupted".into(),
                    reason_code: Some("pi_stop_error".into()),
                }
    }));
    assert!(
        !format!("{:?}", snapshot.observations)
            .contains("private prompt must not reach the extension")
    );
    drop(snapshot);
    let store = Store::open_read_only(&database).expect("open read-only durable store");
    let mut app = AgenturmApp::default_composition();
    app.install_plugin(LifecycleExtensionPlugin)
        .expect("install independent capability and pane package");
    let projected = app.refresh_history_query(&store, ObservationQuery::latest(32));
    assert!(projected.failures.is_empty());
    assert_eq!(
        projected.metrics
            [&agenturm_kernel::ExtensionId::parse("capability.example.lifecycle").unwrap()][1]
            .value,
        "1"
    );
    let pane =
        &projected.panes[&agenturm_kernel::ExtensionId::parse("pane.example.lifecycle").unwrap()];
    assert_eq!(pane.title, "Lifecycle by source");
    assert_eq!(pane.rows[0].label, "harness.pi");
    assert_eq!(pane.rows[0].value, "2 total · 1 interrupted");
    assert!(
        projected
            .panes
            .contains_key(&agenturm_kernel::ExtensionId::parse("pane.overview").unwrap())
    );
    let model = UiModel::from_snapshot(projected);
    assert_eq!(model.selected_title(), Some("Lifecycle by source"));
    fs::remove_dir_all(root).expect("remove fixture directory");
}

#[cfg(unix)]
#[test]
fn configured_watch_retries_startup_failure_and_shuts_down_on_interrupt() {
    let (Ok(plugin_host), Ok(pi_adapter)) = (
        std::env::var("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var("AGENTURM_RUNTIME_PI_ADAPTER"),
    ) else {
        eprintln!("skipping real watch test: set plugin host and Pi adapter paths");
        return;
    };
    let root = temporary_directory();
    fs::create_dir_all(&root).expect("create fixture directory");
    let source = root.join("pi.jsonl");
    fs::write(
        &source,
        concat!(
            "{\"type\":\"session\",\"id\":\"watch-session\"}\n",
            "{\"type\":\"message\",\"id\":\"assistant-1\",\"timestamp\":\"2026-11-20T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"stopReason\":\"stop\",\"usage\":{\"input\":8,\"output\":3},\"content\":[]}}\n"
        ),
    )
    .expect("write Pi watch fixture");
    let config_path = root.join("sources.json");
    let delayed_adapter = root.join("pi-adapter-delayed");
    let config = serde_json::json!({
        "version": 1,
        "plugin_host": plugin_host,
        "sources": [{
            "plugin_id": "example.rust-pi-harness",
            "adapter": delayed_adapter,
            "source_root": source,
            "source_id": "harness.pi"
        }]
    });
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).expect("write config");
    let database = root.join("agenturm.sqlite");
    let mut child = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args([
            "collect",
            "--config",
            config_path.to_str().unwrap(),
            "--db",
            database.to_str().unwrap(),
            "--watch",
            "--interval-ms",
            "100",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start foreground collector");

    let stdout = child.stdout.take().expect("capture collector stdout");
    let (line_sender, line_receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line_sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut report_lines = Vec::new();
    let mut installed_adapter = false;
    let mut committed_reports = 0;
    while committed_reports < 2 {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let line = if remaining.is_zero() {
            None
        } else {
            line_receiver.recv_timeout(remaining).ok()
        };
        let Some(line) = line else {
            let _ = child.kill();
            reader.join().expect("join collector output reader");
            let output = child
                .wait_with_output()
                .expect("read failed collector output");
            panic!(
                "collector did not produce two reports before deadline; lines={report_lines:?}; stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        if line.starts_with("harness.pi: startup failed:") && !installed_adapter {
            std::os::unix::fs::symlink(&pi_adapter, &delayed_adapter)
                .expect("make Pi adapter available after startup failure");
            installed_adapter = true;
        }
        if line.starts_with("harness.pi: committed ")
            || line.starts_with("harness.pi: replay confirmed ")
        {
            committed_reports += 1;
        }
        report_lines.push(line);
    }
    let signal = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status();
    if !signal.is_ok_and(|status| status.success()) {
        let _ = child.kill();
        let _ = child.wait();
        panic!("send interrupt signal to collector");
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while child.try_wait().expect("poll collector process").is_none()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if child
        .try_wait()
        .expect("check collector shutdown")
        .is_none()
    {
        let _ = child.kill();
        let _ = child.wait();
        panic!("collector did not stop after interrupt");
    }
    reader.join().expect("join collector output reader");
    report_lines.extend(line_receiver.try_iter());
    let output = child.wait_with_output().expect("read collector output");
    assert_eq!(output.status.code(), Some(1));
    let stdout = report_lines.join("\n");
    assert!(
        installed_adapter,
        "watch reported the initial startup failure"
    );
    assert!(stdout.contains("harness.pi: committed 3 observation(s)"));
    assert!(committed_reports >= 2);
    assert!(stdout.contains("Stopped after"));

    let snapshot = execute_inspect(&database).expect("reopen watch history");
    assert_eq!(snapshot.observations.len(), 3);
    assert_eq!(
        snapshot.observations[0].identity.session_id.as_deref(),
        Some("watch-session")
    );
    drop(snapshot);
    fs::remove_dir_all(root).expect("remove fixture directory");
}

#[cfg(unix)]
#[test]
fn configured_watch_exits_cleanly_on_terminate_between_cycles() {
    let (Ok(plugin_host), Ok(pi_adapter)) = (
        std::env::var("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var("AGENTURM_RUNTIME_PI_ADAPTER"),
    ) else {
        eprintln!("skipping real watch shutdown test: set plugin host and Pi adapter paths");
        return;
    };
    let root = temporary_directory();
    fs::create_dir_all(&root).expect("create fixture directory");
    let source = root.join("pi.jsonl");
    fs::write(
        &source,
        concat!(
            "{\"type\":\"session\",\"id\":\"terminate-session\"}\n",
            "{\"type\":\"message\",\"id\":\"assistant-1\",\"timestamp\":\"2026-11-20T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"stopReason\":\"stop\",\"usage\":{\"input\":8,\"output\":3},\"content\":[]}}\n"
        ),
    )
    .expect("write Pi fixture");
    let config_path = root.join("sources.json");
    let config = serde_json::json!({
        "version": 1,
        "plugin_host": plugin_host,
        "sources": [{
            "plugin_id": "example.rust-pi-harness",
            "adapter": pi_adapter,
            "source_root": source,
            "source_id": "harness.pi"
        }]
    });
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).expect("write config");
    let database = root.join("agenturm.sqlite");
    let mut child = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args([
            "collect",
            "--config",
            config_path.to_str().unwrap(),
            "--db",
            database.to_str().unwrap(),
            "--watch",
            "--interval-ms",
            "1000",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start foreground collector");

    let stdout = child.stdout.take().expect("capture collector stdout");
    let (line_sender, line_receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line_sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut report_lines = Vec::new();
    let mut successful_reports = 0;
    while successful_reports < 2 {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let line = if remaining.is_zero() {
            None
        } else {
            line_receiver.recv_timeout(remaining).ok()
        };
        let Some(line) = line else {
            let _ = child.kill();
            let _ = child.wait();
            reader.join().expect("join collector output reader");
            panic!("healthy collector did not produce two reports");
        };
        if line.starts_with("harness.pi: committed ")
            || line.starts_with("harness.pi: replay confirmed ")
        {
            successful_reports += 1;
        }
        report_lines.push(line);
    }
    #[cfg(target_os = "linux")]
    let competing_writer = collect_once(&config_path, &database);
    let signal = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status();
    if !signal.is_ok_and(|status| status.success()) {
        let _ = child.kill();
        let _ = child.wait();
        reader.join().expect("join collector output reader");
        panic!("send SIGTERM to collector");
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while child.try_wait().expect("poll collector process").is_none()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if child
        .try_wait()
        .expect("check collector shutdown")
        .is_none()
    {
        let _ = child.kill();
        let _ = child.wait();
        reader.join().expect("join collector output reader");
        panic!("collector did not stop after SIGTERM");
    }
    reader.join().expect("join collector output reader");
    report_lines.extend(line_receiver.try_iter());
    let output = child.wait_with_output().expect("read collector output");
    assert!(output.status.success());
    #[cfg(target_os = "linux")]
    {
        let error = String::from_utf8_lossy(&competing_writer.stderr);
        assert_eq!(competing_writer.status.code(), Some(1), "{error}");
        assert!(
            error.contains("collector writer lease unavailable"),
            "{error}"
        );
    }
    let stdout = report_lines.join("\n");
    assert!(stdout.contains("Stopped after"));
    assert!(!stdout.contains("cancelled by shutdown"));
    let snapshot = execute_inspect(&database).expect("reopen watch history");
    assert_eq!(snapshot.observations.len(), 3);
    assert_eq!(
        snapshot.observations[0].identity.session_id.as_deref(),
        Some("terminate-session")
    );
    drop(snapshot);
    fs::remove_dir_all(root).expect("remove fixture directory");
}

#[test]
fn configured_collection_admits_pi_and_codex_together_and_reopens_history() {
    let (Ok(plugin_host), Ok(pi_adapter), Ok(codex_adapter)) = (
        std::env::var("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var("AGENTURM_RUNTIME_PI_ADAPTER"),
        std::env::var("AGENTURM_RUNTIME_CODEX_ADAPTER"),
    ) else {
        eprintln!(
            "skipping configured collection process test: set plugin host, Pi adapter, and Codex adapter paths"
        );
        return;
    };
    let root = temporary_directory();
    fs::create_dir_all(&root).expect("create fixture directory");
    let pi_source = root.join("pi.jsonl");
    let codex_source = root.join("codex.jsonl");
    fs::write(
        &pi_source,
        concat!(
            "{\"type\":\"session\",\"id\":\"pi-session\"}\n",
            "{\"type\":\"message\",\"id\":\"assistant-1\",\"timestamp\":\"2026-11-20T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"stopReason\":\"toolUse\",\"usage\":{\"input\":8,\"output\":3},\"content\":[{\"type\":\"toolCall\",\"id\":\"call-1\",\"name\":\"read\"}]}}\n"
        ),
    )
    .expect("write Pi fixture");
    fs::write(
        &codex_source,
        concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-session\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-11-20T12:00:00Z\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"turn-1\"}}\n",
            "{\"type\":\"token_usage_record\",\"timestamp\":\"2026-11-20T12:00:02Z\",\"payload\":{\"type\":\"token_usage_record\",\"turn_id\":\"turn-1\",\"response_id\":\"response-1\",\"usage\":{\"input_tokens\":7,\"output_tokens\":3,\"total_tokens\":10}}}\n"
        ),
    )
    .expect("write Codex fixture");

    let config_path = root.join("sources.json");
    let config = serde_json::json!({
        "version": 1,
        "plugin_host": plugin_host,
        "sources": [
            {
                "plugin_id": "example.rust-pi-harness",
                "adapter": pi_adapter,
                "source_root": pi_source,
                "source_id": "harness.pi"
            },
            {
                "plugin_id": "example.rust-codex-harness",
                "adapter": codex_adapter,
                "source_root": codex_source,
                "source_id": "harness.codex"
            },
            {
                "plugin_id": "example.rust-missing-harness",
                "adapter": root.join("missing-adapter"),
                "source_root": root.join("missing-source"),
                "source_id": "harness.broken"
            }
        ]
    });
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).expect("write config");
    let database = root.join("agenturm.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args([
            "collect",
            "--config",
            config_path.to_str().unwrap(),
            "--db",
            database.to_str().unwrap(),
        ])
        .output()
        .expect("run configured collection CLI");
    assert_eq!(output.status.code(), Some(1));
    let summary = String::from_utf8(output.stdout).expect("collection output is UTF-8");
    assert!(summary.contains("3 sources checked; 1 source failure; 0 global failures"));
    assert!(summary.contains("harness.pi: committed 4 observation(s)"));
    assert!(summary.contains("harness.codex: committed 2 observation(s)"));
    assert!(summary.contains("harness.broken: startup failed:"));
    assert!(summary.contains("ACK sent=true"));

    let snapshot = execute_inspect(&database).expect("reopen committed collection history");
    assert_eq!(snapshot.observations.len(), 6);
    assert_eq!(
        snapshot
            .observations
            .iter()
            .filter(|row| matches!(row.kind, agenturm_kernel::ObservationKind::Tool { .. }))
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .observations
            .iter()
            .filter(|row| matches!(row.kind, agenturm_kernel::ObservationKind::Lifecycle { .. }))
            .count(),
        2
    );
    let session_ids = snapshot
        .observations
        .iter()
        .filter_map(|observation| observation.identity.session_id.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(session_ids, ["codex-session", "pi-session"].into());
    assert!(snapshot.failures.is_empty());

    drop(snapshot);
    fs::remove_dir_all(root).expect("remove fixture directory");
}
