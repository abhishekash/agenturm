use agenturm_kernel::{ExtensionId, Kernel, Observation, ObservationStore, StoreCommit};
use agenturm_runtime::{ProcessConfig, ProcessSource};
use agenturm_store::{ObservationKind, Store};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct FailOnceStore {
    inner: Store,
    fail_next_commit: bool,
}

impl ObservationStore for FailOnceStore {
    fn source_cursor(&self, source_id: &str) -> std::result::Result<Option<String>, String> {
        ObservationStore::source_cursor(&self.inner, source_id)
    }

    fn commit(
        &mut self,
        source_id: &str,
        expected_cursor: Option<&str>,
        next_cursor: &str,
        observations: &[Observation],
    ) -> std::result::Result<StoreCommit, String> {
        if self.fail_next_commit {
            self.fail_next_commit = false;
            return Err("injected first commit failure".into());
        }
        ObservationStore::commit(
            &mut self.inner,
            source_id,
            expected_cursor,
            next_cursor,
            observations,
        )
    }
}

fn temporary_directory() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agenturm-runtime-e2e-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn codex_process_commits_metadata_usage_then_acknowledges() {
    let (Ok(plugin_host), Ok(adapter)) = (
        std::env::var("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var("AGENTURM_RUNTIME_CODEX_ADAPTER"),
    ) else {
        eprintln!(
            "skipping real-process test: set AGENTURM_RUNTIME_PLUGIN_HOST and AGENTURM_RUNTIME_CODEX_ADAPTER"
        );
        return;
    };
    let root = temporary_directory();
    fs::create_dir_all(&root).expect("create fixture directory");
    let source_path = root.join("session.jsonl");
    fs::write(
        &source_path,
        concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session-1\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-11-20T12:00:00Z\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"turn-1\"}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-11-20T12:00:01Z\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"private text never stored\"}}\n",
            "{\"type\":\"token_usage_record\",\"timestamp\":\"2026-11-20T12:00:02Z\",\"payload\":{\"type\":\"token_usage_record\",\"turn_id\":\"turn-1\",\"response_id\":\"response-1\",\"usage\":{\"input_tokens\":7,\"output_tokens\":3,\"cached_input_tokens\":2,\"cache_write_input_tokens\":1,\"total_tokens\":10}}}\n"
        ),
    )
    .expect("write Codex fixture");

    let database = root.join("agenturm.sqlite");
    let store = Store::open(&database).expect("open store");
    let mut store = FailOnceStore {
        inner: store,
        fail_next_commit: true,
    };
    let mut config = ProcessConfig::codex(
        PathBuf::from(plugin_host),
        PathBuf::from(adapter),
        source_path,
        "harness.codex",
    );
    config.timeout = Duration::from_secs(5);
    let source = ProcessSource::spawn(config).expect("start Rust Codex adapter");
    let source_id = ExtensionId::parse("harness.codex").unwrap();
    let mut kernel = Kernel::new();
    kernel
        .register_source_adapter(source_id.clone(), source)
        .expect("register supervised source");
    let failed = kernel.refresh_with_store(&mut store);
    assert!(failed.admissions.is_empty());
    assert_eq!(failed.failures.len(), 1);
    assert_eq!(
        failed.failures[0].stage,
        agenturm_kernel::ExtensionStage::AdmissionCommit
    );

    // The same kernel and live child re-offer the cached, unacknowledged batch.
    let snapshot = kernel.refresh_with_store(&mut store);
    let admission = snapshot
        .admissions
        .get(&source_id)
        .expect("admission status");
    assert_eq!(admission.inserted, 1);
    assert!(!admission.replay);
    assert_eq!(admission.skipped_non_usage_facts, 2);
    assert!(admission.source_ack_sent);

    let idle = kernel.refresh_with_store(&mut store);
    let idle_admission = idle.admissions.get(&source_id).expect("idle admission");
    assert_eq!(idle_admission.inserted, 0);
    assert_eq!(idle_admission.skipped_non_usage_facts, 0);
    assert!(idle_admission.source_ack_sent);
    drop(kernel);
    drop(store);

    let store = Store::open(&database).expect("reopen committed store");
    let cursor = store
        .source_cursor("harness.codex")
        .expect("read source cursor")
        .expect("durable cursor committed");
    assert!(!cursor.is_empty());
    let observations = store.observations().expect("read committed observations");
    assert_eq!(observations.len(), 1);
    let (source_id, observation) = &observations[0];
    assert_eq!(source_id, "harness.codex");
    assert!(observation.event_id.len() > 32);
    assert_eq!(observation.revision, 0);
    assert_eq!(observation.observed_at_unix_ns, 1_795_176_002_000_000_000);
    assert_eq!(observation.identity.session_id, "session-1");
    assert_eq!(observation.identity.turn_id.as_deref(), Some("turn-1"));
    assert_eq!(
        observation.kind,
        ObservationKind::Usage {
            input_tokens: Some(7),
            output_tokens: Some(3),
            cache_read_tokens: Some(2),
            cache_write_tokens: Some(1),
            total_tokens: Some(10),
        }
    );
    fs::remove_dir_all(root).expect("remove fixture directory");
}
