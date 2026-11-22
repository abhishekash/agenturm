#![cfg(unix)]
use agenturm_kernel::ObservationStore;

use agenturm_connector_api::{
    CONNECTOR_API_VERSION, ConnectorDescriptor, ConnectorDirection, ConnectorId, OutboundDelivery,
    request_sha256,
};
use agenturm_runtime::connector::{
    ConnectorLaunch, ConnectorRuntime, ConnectorSpec, DeliveryState,
};
use agenturm_store::Store;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct Fixture {
    path: PathBuf,
    spec: ConnectorSpec,
}

impl Fixture {
    fn new(direction: ConnectorDirection, mode: &str) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let target = format!(
            "connector-fixture-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let path = PathBuf::from("/tmp").join(&target);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("mode"), mode).unwrap();
        let binary = PathBuf::from(env!("CARGO_BIN_EXE_agenturm-connector-fixture"));
        let sha256 = format!("{:x}", Sha256::digest(fs::read(&binary).unwrap()));
        let spec = ConnectorSpec {
            descriptor: ConnectorDescriptor {
                api_version: CONNECTOR_API_VERSION,
                id: ConnectorId::parse("fixture.runtime").unwrap(),
                direction,
                features: Default::default(),
            },
            executable: binary,
            target,
            sha256,
            launch: ConnectorLaunch::Trusted,
        };
        Self { path, spec }
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::open(&self.path.join("connector.sqlite")).unwrap()
    }

    fn history(&self) -> Store {
        Store::open(self.path.join("history.sqlite")).unwrap()
    }

    fn delivery(&self) -> OutboundDelivery {
        let payload = json!("completed");
        OutboundDelivery {
            delivery_id: "work-one".into(),
            idempotency_key: "stable-key".into(),
            destination_id: self.spec.target.clone(),
            action: "write_status".into(),
            request_sha256: request_sha256(&self.spec.target, "write_status", &payload).unwrap(),
            payload,
        }
    }

    fn attempts(&self) -> String {
        fs::read_to_string(self.path.join("attempts")).unwrap()
    }

    fn watch(&self, inbound: bool) -> Watcher {
        let config = self.path.join("config.json");
        let log = self.path.join("watch.log");
        fs::write(&config, serde_json::to_vec(&self.spec).unwrap()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_agenturm-connector"));
        command
            .args([self.path.join("connector.sqlite"), config])
            .arg("watch")
            .stdout(fs::File::create(&log).unwrap())
            .stderr(Stdio::null());
        if inbound {
            command.arg(self.path.join("history.sqlite"));
        }
        Watcher {
            child: command.spawn().unwrap(),
            log,
        }
    }
}

struct Watcher {
    child: Child,
    log: PathBuf,
}

impl Watcher {
    fn wait_for(&mut self, status: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            let report = fs::read_to_string(&self.log).unwrap();
            if report.contains(status) {
                return report;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "watch stopped early: {report}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "watch state missing: {status}, log: {}",
            fs::read_to_string(&self.log).unwrap()
        );
    }

    fn stop(mut self) {
        // SAFETY: PID belongs to this still supervised test child.
        assert_eq!(
            unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) },
            0
        );
        self.wait_for("watch_stopped");
        assert!(self.child.wait().unwrap().success());
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn inbound_denies_without_grant_and_recovers_after_commit_before_ack() {
    let fixture = Fixture::new(ConnectorDirection::Inbound, "recover-ack");
    let mut runtime = fixture.runtime();
    let mut history = fixture.history();
    assert_eq!(
        runtime.collect(&fixture.spec, &mut history),
        Err("connector_permission_denied")
    );
    assert!(!fixture.path.join("launched").exists());
    runtime.grant(&fixture.spec).unwrap();
    assert_eq!(
        runtime.collect(&fixture.spec, &mut history),
        Err("connector_protocol_error")
    );
    assert_eq!(
        history
            .source_cursor(&fixture.spec.target)
            .unwrap()
            .as_deref(),
        Some("one")
    );
    drop(runtime);
    drop(history);

    let mut runtime = fixture.runtime();
    let mut history = fixture.history();
    assert_eq!(runtime.collect(&fixture.spec, &mut history), Ok(0));
    assert_eq!(
        fs::read_to_string(fixture.path.join("acked")).unwrap(),
        "one"
    );
    assert_eq!(history.load_observations(10).unwrap().observations.len(), 1);
    assert!(runtime.revoke(&fixture.spec).unwrap());
    assert_eq!(
        runtime.collect(&fixture.spec, &mut history),
        Err("connector_permission_denied")
    );
}

#[test]
fn outbound_timeout_recovers_accepted_receipt_without_resend() {
    let fixture = Fixture::new(ConnectorDirection::Outbound, "accepted-then-timeout");
    let delivery = fixture.delivery();
    let mut runtime = fixture.runtime();
    assert_eq!(
        runtime.enqueue(&fixture.spec, &delivery),
        Err("connector_permission_denied")
    );
    runtime.grant(&fixture.spec).unwrap();
    runtime.enqueue(&fixture.spec, &delivery).unwrap();
    runtime.enqueue(&fixture.spec, &delivery).unwrap();
    let mut conflict = delivery.clone();
    conflict.payload = json!("running");
    conflict.request_sha256 = request_sha256(
        &conflict.destination_id,
        &conflict.action,
        &conflict.payload,
    )
    .unwrap();
    assert_eq!(
        runtime.enqueue(&fixture.spec, &conflict),
        Err("connector_idempotency_conflict")
    );
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 0),
        Ok(DeliveryState::Unknown)
    );
    assert_eq!(fixture.attempts(), "1");
    drop(runtime);

    let mut runtime = fixture.runtime();
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 100),
        Ok(DeliveryState::Waiting)
    );
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 5_000),
        Ok(DeliveryState::Accepted {
            receipt_id: "receipt-one".into()
        })
    );
    assert_eq!(fixture.attempts(), "1");
}

#[test]
fn confirmed_absence_retries_but_revoke_and_regrant_do_not_reactivate_work() {
    let fixture = Fixture::new(ConnectorDirection::Outbound, "missing-then-retry");
    let delivery = fixture.delivery();
    let mut runtime = fixture.runtime();
    runtime.grant(&fixture.spec).unwrap();
    runtime.enqueue(&fixture.spec, &delivery).unwrap();
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 0),
        Ok(DeliveryState::Unknown)
    );
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 5_000),
        Ok(DeliveryState::Pending)
    );
    assert_eq!(fixture.attempts(), "1");
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 10_000),
        Ok(DeliveryState::Accepted {
            receipt_id: "receipt-one".into()
        })
    );
    assert_eq!(fixture.attempts(), "2");
    assert!(runtime.revoke(&fixture.spec).unwrap());
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 20_000),
        Err("connector_permission_denied")
    );
    runtime.grant(&fixture.spec).unwrap();
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 20_000),
        Err("connector_delivery_unavailable")
    );
}

#[test]
fn standalone_driver_enqueues_and_reports_receipt() {
    let fixture = Fixture::new(ConnectorDirection::Outbound, "immediate");
    let config = fixture.path.join("config.json");
    let request = fixture.path.join("request.json");
    let ledger = fixture.path.join("connector.sqlite");
    fs::write(&config, serde_json::to_vec(&fixture.spec).unwrap()).unwrap();
    fs::write(&request, serde_json::to_vec(&fixture.delivery()).unwrap()).unwrap();
    let run = |args: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_agenturm-connector"))
            .args([&ledger, &config])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap()
    };
    run(&["grant"]);
    run(&["enqueue-status", "work-one", "stable-key", "completed"]);
    run(&["enqueue", request.to_str().unwrap()]);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&run(&["drive", "work-one"])).unwrap(),
        json!({"state":"accepted","receipt_id":"receipt-one"})
    );
}

#[test]
fn outbound_watch_reconciles_due_work_and_stops_gracefully() {
    let fixture = Fixture::new(ConnectorDirection::Outbound, "accepted-then-timeout");
    let delivery = fixture.delivery();
    let mut runtime = fixture.runtime();
    runtime.grant(&fixture.spec).unwrap();
    runtime.enqueue(&fixture.spec, &delivery).unwrap();
    let mut watcher = fixture.watch(false);
    let report = watcher.wait_for(r#""state":"accepted""#);
    assert!(report.contains(r#""state":"unknown""#));
    for private in [
        "work-one",
        "stable-key",
        "receipt-one",
        fixture.spec.target.as_str(),
    ] {
        assert!(!report.contains(private));
    }
    watcher.stop();
    assert_eq!(fixture.attempts(), "1");
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 100_000),
        Ok(DeliveryState::Accepted {
            receipt_id: "receipt-one".into()
        })
    );
}

#[test]
fn inbound_watch_commits_and_releases_history_on_signal() {
    let fixture = Fixture::new(ConnectorDirection::Inbound, "immediate");
    fixture.runtime().grant(&fixture.spec).unwrap();
    let mut watcher = fixture.watch(true);
    watcher.wait_for("inbound_commit");
    watcher.stop();
    let history = fixture.history();
    assert_eq!(
        history
            .source_cursor(&fixture.spec.target)
            .unwrap()
            .as_deref(),
        Some("one")
    );
    assert_eq!(history.load_observations(10).unwrap().observations.len(), 1);
}

#[test]
fn unknown_lookup_exhaustion_is_durable_and_never_resends() {
    let fixture = Fixture::new(ConnectorDirection::Outbound, "always-unknown");
    let delivery = fixture.delivery();
    let mut runtime = fixture.runtime();
    runtime.grant(&fixture.spec).unwrap();
    runtime.enqueue(&fixture.spec, &delivery).unwrap();
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 0),
        Ok(DeliveryState::Unknown)
    );
    for tick in 1..=8 {
        drop(runtime);
        runtime = fixture.runtime();
        assert_eq!(
            runtime.drive(&fixture.spec, &delivery.delivery_id, tick * 100_000),
            Ok(DeliveryState::Unknown)
        );
    }
    assert_eq!(
        runtime.drive(&fixture.spec, &delivery.delivery_id, 1_000_000),
        Ok(DeliveryState::Unknown)
    );
    assert_eq!(
        fs::read_to_string(fixture.path.join("lookups")).unwrap(),
        "8"
    );
    assert_eq!(fixture.attempts(), "1");
}

#[test]
fn wrong_scope_and_modified_artifact_fail_before_process_launch() {
    let mut fixture = Fixture::new(ConnectorDirection::Inbound, "immediate");
    let copy = fixture.path.join("connector");
    fs::copy(&fixture.spec.executable, &copy).unwrap();
    fixture.spec.executable = copy.clone();
    let mut runtime = fixture.runtime();
    let mut history = fixture.history();
    runtime.grant(&fixture.spec).unwrap();
    let mut wrong = fixture.spec.clone();
    wrong.target.push_str("-other");
    assert_eq!(
        runtime.collect(&wrong, &mut history),
        Err("connector_permission_denied")
    );

    fs::write(copy, b"changed after approval").unwrap();
    assert_eq!(
        runtime.collect(&fixture.spec, &mut history),
        Err("connector_protocol_error")
    );
    assert!(!fixture.path.join("launched").exists());
    assert!(
        history
            .source_cursor(&fixture.spec.target)
            .unwrap()
            .is_none()
    );
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn isolated_policy_rejects_interpreter_bearing_artifact() {
    let mut fixture = Fixture::new(ConnectorDirection::Inbound, "immediate");
    fixture.spec.launch = ConnectorLaunch::IsolatedLinux;
    assert_eq!(
        fixture.runtime().grant(&fixture.spec),
        Err("connector_isolation_artifact_unsupported")
    );
    assert!(!fixture.path.join("launched").exists());
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn isolated_connector_has_only_framed_io_and_commits_before_ack() {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let mut fixture = Fixture::new(ConnectorDirection::Inbound, "isolated");
    let executable = fixture.path.join("static-connector");
    let build = Command::new("cc")
        .args(["-static", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&executable)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/isolated_connector.c"
        ))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    fixture.spec.launch = ConnectorLaunch::IsolatedLinux;
    fixture.spec.sha256 = format!("{:x}", Sha256::digest(fs::read(&executable).unwrap()));
    fixture.spec.executable = executable;
    let config = fixture.path.join("config.json");
    let ledger = fixture.path.join("connector.sqlite");
    let history = fixture.path.join("history.sqlite");
    let private = fixture.path.join("unrelated-user-file");
    fs::write(&config, serde_json::to_vec(&fixture.spec).unwrap()).unwrap();
    fs::write(&private, b"synthetic fixture credential").unwrap();
    let private_file = fs::File::open(private).unwrap();
    let mut runtime = fixture.runtime();
    runtime.grant(&fixture.spec).unwrap();
    let mut wrong_launch = fixture.spec.clone();
    wrong_launch.launch = ConnectorLaunch::Trusted;
    assert_eq!(
        runtime.check_grant(&wrong_launch),
        Err("connector_permission_denied")
    );
    let collect = |deny_setup| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agenturm-connector"));
        command
            .args([&ledger, &config])
            .arg("collect")
            .arg(&history)
            .env("AGENTURM_CONNECTOR_TEST_CREDENTIAL", "synthetic");
        let private_fd = private_file.as_raw_fd();
        // SAFETY: child-only dup2 preserves a fixture descriptor through the
        // driver's exec, so the connector has to remove it at its own exec.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(private_fd, 199) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if deny_setup {
                    deny_close_range()?;
                }
                Ok(())
            });
        }
        command.output().unwrap()
    };
    let failed_setup = collect(true);
    assert!(!failed_setup.status.success());
    assert_eq!(failed_setup.stderr, b"connector_isolation_unavailable\n");
    assert!(
        fixture
            .history()
            .load_observations(10)
            .unwrap()
            .observations
            .is_empty()
    );

    let result = collect(false);
    // SAFETY: version query takes no ruleset or userspace pointers.
    let landlock_abi = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, 0, 0, 1) };
    if landlock_abi < 1 {
        assert!(!result.status.success());
        assert_eq!(result.stderr, b"connector_isolation_unavailable\n");
        assert!(
            fixture
                .history()
                .load_observations(10)
                .unwrap()
                .observations
                .is_empty()
        );
        return;
    }
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"1\n");
    let stored = fixture.history();
    assert_eq!(
        stored
            .source_cursor(&fixture.spec.target)
            .unwrap()
            .as_deref(),
        Some("sandbox-1")
    );
    assert_eq!(stored.load_observations(10).unwrap().observations.len(), 1);
    drop(stored);
    assert!(runtime.revoke(&fixture.spec).unwrap());
    let denied = collect(false);
    assert!(!denied.status.success());
    assert_eq!(denied.stderr, b"connector_permission_denied\n");
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn deny_close_range() -> std::io::Result<()> {
    // Simulate a kernel or administrator denying one of the required launch
    // controls even when Landlock is present.
    let instruction = |code, jt, jf, k| libc::sock_filter { code, jt, jf, k };
    let ret = (libc::BPF_RET | libc::BPF_K) as u16;
    let mut filter = [
        instruction((libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16, 0, 0, 0),
        instruction(
            (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            0,
            1,
            libc::SYS_close_range as u32,
        ),
        instruction(ret, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        instruction(ret, 0, 0, libc::SECCOMP_RET_ALLOW),
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    // SAFETY: pre_exec context. The filter only denies close_range and the
    // kernel copies these stack instructions before returning.
    if unsafe {
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
            || libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) != 0
    } {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
