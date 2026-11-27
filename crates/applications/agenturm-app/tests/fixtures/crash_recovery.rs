use super::{
    collect_once, standalone_collection_config, standalone_harness_paths, temporary_directory,
};
use agenturm_app::execute_inspect;
use agenturm_kernel::{
    ExtensionId, Kernel, Observation, ObservationPage, ObservationStore, StoreCommit,
};
use agenturm_runtime::{ProcessConfig, ProcessSource};
use agenturm_store::Store;
use std::fs::{self, File};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SOURCE_ID: &str = "harness.standalone";
const EXPECTED_CURSOR: &str = "standalone-record-1";
const ROOT_ENV: &str = "AGENTURM_TEST_CRASH_RECOVERY_ROOT";
const PHASE_ENV: &str = "AGENTURM_TEST_CRASH_RECOVERY_PHASE";

struct PausingStore {
    store: Store,
    checkpoint: PathBuf,
    before_commit: bool,
}

impl PausingStore {
    fn pause(&self) -> ! {
        File::create(&self.checkpoint).expect("signal commit checkpoint");
        loop {
            thread::park();
        }
    }
}

impl ObservationStore for PausingStore {
    fn source_cursor(&self, source_id: &str) -> Result<Option<String>, String> {
        ObservationStore::source_cursor(&self.store, source_id)
    }

    fn load_observations(&self, limit: usize) -> Result<ObservationPage, String> {
        ObservationStore::load_observations(&self.store, limit)
    }

    fn commit(
        &mut self,
        source_id: &str,
        expected_cursor: Option<&str>,
        next_cursor: &str,
        observations: &[Observation],
    ) -> Result<StoreCommit, String> {
        assert_eq!(source_id, SOURCE_ID);
        assert_eq!(expected_cursor, None);
        assert_eq!(next_cursor, EXPECTED_CURSOR);
        assert_eq!(observations.len(), 1);
        if self.before_commit {
            self.pause();
        }
        ObservationStore::commit(
            &mut self.store,
            source_id,
            expected_cursor,
            next_cursor,
            observations,
        )?;
        // The kernel sends ACK only after commit returns successfully.
        self.pause();
    }
}

fn collect_until_checkpoint(root: &Path) -> ! {
    let phase = std::env::var(PHASE_ENV).expect("commit checkpoint phase");
    let (host, adapter) = standalone_harness_paths().expect("real plugin binaries");
    let mut store = PausingStore {
        store: Store::open_collector(root.join("agenturm.sqlite")).expect("open writer"),
        checkpoint: root.join("checkpoint"),
        before_commit: match phase.as_str() {
            "before-commit" => true,
            "after-commit" => false,
            _ => panic!("unexpected commit checkpoint phase {phase}"),
        },
    };
    let source = ProcessSource::spawn(ProcessConfig::new(
        host,
        adapter,
        root.join(SOURCE_ID),
        SOURCE_ID,
        "fixture.standalone-harness",
    ))
    .expect("start supervised standalone adapter");
    let mut kernel = Kernel::new();
    kernel
        .register_source_adapter(ExtensionId::parse(SOURCE_ID).unwrap(), source)
        .expect("register adapter");
    let snapshot = kernel.refresh_with_store(&mut store);
    panic!(
        "collector did not reach commit checkpoint: {:?}",
        snapshot.failures
    );
}

struct CrashWorker(Child);

impl CrashWorker {
    fn kill_at_checkpoint(root: &Path, phase: &str) {
        let mut worker = Self(
            Command::new(std::env::current_exe().expect("locate current test binary"))
                .args([
                    "--exact",
                    "crash_recovery::collector_restarts_across_both_commit_boundaries",
                    "--nocapture",
                ])
                .env(ROOT_ENV, root)
                .env(PHASE_ENV, phase)
                .stdout(Stdio::null())
                .spawn()
                .expect("start crash recovery collector"),
        );
        let checkpoint = root.join("checkpoint");
        let deadline = Instant::now() + Duration::from_secs(15);
        while !checkpoint.is_file() {
            let status = worker.0.try_wait().expect("check worker process");
            assert!(status.is_none(), "worker exited before {phase}: {status:?}");
            assert!(Instant::now() < deadline, "worker did not reach {phase}");
            thread::sleep(Duration::from_millis(10));
        }
        worker
            .0
            .kill()
            .expect("kill collector without graceful shutdown");
        let status = worker.0.wait().expect("wait for killed collector");
        assert_eq!(status.signal(), Some(9));
    }
}

impl Drop for CrashWorker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn collector_restarts_across_both_commit_boundaries() {
    // Reinvoke this test in a separate process so SIGKILL bypasses destructors.
    if let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) {
        collect_until_checkpoint(&root);
    }
    let Some((host, adapter)) = standalone_harness_paths() else {
        return;
    };
    for (phase, committed_before_crash) in [("before-commit", false), ("after-commit", true)] {
        let root = temporary_directory();
        let config = standalone_collection_config(
            &root,
            &host,
            &adapter,
            "agenturm-observations/1",
            &[(SOURCE_ID, "tokens=42\n")],
        );
        let database = root.join("agenturm.sqlite");
        CrashWorker::kill_at_checkpoint(&root, phase);

        if committed_before_crash {
            let wal =
                fs::metadata(root.join("agenturm.sqlite-wal")).expect("recover uncheckpointed WAL");
            assert!(wal.len() > 32, "WAL must contain committed frames");
        }
        let reopened = Store::open_read_only(&database).expect("read killed writer database");
        assert_eq!(
            reopened.source_cursor(SOURCE_ID).unwrap().as_deref(),
            committed_before_crash.then_some(EXPECTED_CURSOR),
            "{phase}"
        );
        assert_eq!(
            reopened.observations().unwrap().len(),
            usize::from(committed_before_crash),
            "{phase}"
        );
        drop(reopened);

        let output = collect_once(&config, &database);
        let report = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{phase}: {report}; {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let inserted = usize::from(!committed_before_crash);
        assert!(
            report.contains(&format!("committed {inserted} observation(s)")),
            "{phase}: {report}"
        );
        assert!(report.contains("ACK sent=true"), "{phase}: {report}");

        let snapshot = execute_inspect(&database).expect("reopen recovered observations");
        assert_eq!(snapshot.observations.len(), 1, "{phase}");
        assert_eq!(snapshot.observations[0].event_id, "standalone-event-1");
        let store = Store::open_read_only(&database).expect("reopen recovered cursor");
        assert_eq!(
            store.source_cursor(SOURCE_ID).unwrap().as_deref(),
            Some(EXPECTED_CURSOR),
            "{phase}"
        );
        drop(store);
        fs::remove_dir_all(root).expect("remove crash recovery fixture directory");
    }
}
