use agenturm_kernel::{
    ExtensionId, Kernel, Observation, ObservationPage, ObservationStore, PreparedSourceBatch,
    StoreCommit,
};
use agenturm_runtime::{
    BundledObservationDecoder, ProcessConfig, ProcessSource, SourceBatchDecoder,
};
use agenturm_store::Store;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct FailOnceStore {
    inner: Store,
    fail_next_commit: bool,
}

impl ObservationStore for FailOnceStore {
    fn source_cursor(&self, source_id: &str) -> std::result::Result<Option<String>, String> {
        ObservationStore::source_cursor(&self.inner, source_id)
    }

    fn load_observations(&self, limit: usize) -> std::result::Result<ObservationPage, String> {
        ObservationStore::load_observations(&self.inner, limit)
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
            return Err("injected first Pi commit failure".into());
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

struct ResetOnceObservationDecoder(Arc<AtomicUsize>);

impl SourceBatchDecoder for ResetOnceObservationDecoder {
    fn decode_batch(
        &self,
        encoded: &str,
        expected_source_id: &str,
    ) -> agenturm_runtime::Result<PreparedSourceBatch> {
        assert_eq!(expected_source_id, "harness.pi");
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(agenturm_runtime::RuntimeError::new(
                "injected decoder reset probe",
            ));
        }
        BundledObservationDecoder.decode_batch(encoded, expected_source_id)
    }
}

fn temporary_directory() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agenturm-runtime-pi-e2e-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn pi_process_uses_injected_decoder_across_commit_retry_and_restart() {
    let (Ok(plugin_host), Ok(adapter)) = (
        std::env::var("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var("AGENTURM_RUNTIME_PI_ADAPTER"),
    ) else {
        eprintln!(
            "skipping real Pi process test: set AGENTURM_RUNTIME_PLUGIN_HOST and AGENTURM_RUNTIME_PI_ADAPTER"
        );
        return;
    };
    let root = temporary_directory();
    fs::create_dir_all(&root).expect("create fixture directory");
    let source_path = root.join("session.jsonl");
    fs::write(
        &source_path,
        concat!(
            "{\"type\":\"session\",\"id\":\"pi-session\"}\n",
            "{\"type\":\"message\",\"id\":\"assistant-1\",\"timestamp\":\"2026-11-20T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"stopReason\":\"toolUse\",\"usage\":{\"input\":8,\"output\":3},\"content\":[{\"type\":\"toolCall\",\"id\":\"call-1\",\"name\":\"read\"}]}}\n"
        ),
    )
    .expect("write Pi fixture");

    let database = root.join("agenturm.sqlite");
    let store = Store::open(&database).expect("open store");
    let mut store = FailOnceStore {
        inner: store,
        fail_next_commit: true,
    };
    let mut config = ProcessConfig::new(
        PathBuf::from(plugin_host),
        PathBuf::from(adapter),
        source_path,
        "harness.pi",
        "example.rust-pi-harness",
    );
    config.timeout = Duration::from_secs(5);
    let decode_count = Arc::new(AtomicUsize::new(0));
    let source = ProcessSource::spawn_with_decoder(
        config,
        ResetOnceObservationDecoder(decode_count.clone()),
    )
    .expect("start Rust Pi adapter");
    let source_id = ExtensionId::parse("harness.pi").unwrap();
    let mut kernel = Kernel::new();
    kernel
        .register_source_adapter(source_id.clone(), source)
        .expect("register supervised Pi source");

    let reset = kernel.refresh_with_store(&mut store);
    assert!(reset.admissions.is_empty());
    assert_eq!(reset.failures.len(), 1);
    assert_eq!(
        reset.failures[0].stage,
        agenturm_kernel::ExtensionStage::AdmissionPoll
    );
    assert_eq!(decode_count.load(Ordering::SeqCst), 1);

    let failed = kernel.refresh_with_store(&mut store);
    assert!(failed.admissions.is_empty());
    assert_eq!(failed.failures.len(), 1);
    assert_eq!(
        failed.failures[0].stage,
        agenturm_kernel::ExtensionStage::AdmissionCommit
    );
    assert_eq!(decode_count.load(Ordering::SeqCst), 2);

    let snapshot = kernel.refresh_with_store(&mut store);
    let admission = snapshot
        .admissions
        .get(&source_id)
        .expect("Pi admission status");
    assert_eq!(admission.inserted, 2);
    assert_eq!(admission.skipped_unprojected_facts, 2);
    assert!(admission.source_ack_sent);
    assert_eq!(decode_count.load(Ordering::SeqCst), 2);
    assert_eq!(snapshot.observations.len(), 2);
    let tool = snapshot
        .observations
        .iter()
        .find(|row| matches!(row.kind, agenturm_kernel::ObservationKind::Tool { .. }))
        .unwrap();
    assert_eq!(tool.identity.operation_id.as_deref(), Some("call-1"));
    assert_eq!(
        tool.kind,
        agenturm_kernel::ObservationKind::Tool {
            category: "file".into(),
            result: "started".into()
        }
    );
    assert!(!format!("{:?}", snapshot.observations).contains("arguments"));
    let usage = snapshot
        .observations
        .iter()
        .find(|row| matches!(row.kind, agenturm_kernel::ObservationKind::Usage { .. }))
        .unwrap();
    assert_eq!(
        usage.kind,
        agenturm_kernel::ObservationKind::Usage {
            input_tokens: Some(8),
            output_tokens: Some(3),
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: None,
        }
    );

    let idle = kernel.refresh_with_store(&mut store);
    assert_eq!(idle.observations.len(), 2);
    assert_eq!(idle.admissions[&source_id].inserted, 0);
    assert!(idle.admissions[&source_id].source_ack_sent);
    drop(kernel);
    drop(store);

    let mut store = Store::open(&database).expect("reopen Pi store");
    let restored = Kernel::new().refresh_with_store(&mut store);
    assert_eq!(restored.observations.len(), 2);
    assert_eq!(restored.observations[0].identity.session_id, "pi-session");
    assert!(restored.failures.is_empty());
    assert!(
        restored
            .observations
            .iter()
            .any(|row| matches!(row.kind, agenturm_kernel::ObservationKind::Tool { .. }))
    );
    assert!(
        restored
            .observations
            .iter()
            .any(|row| matches!(row.kind, agenturm_kernel::ObservationKind::Usage { .. }))
    );

    drop(store);
    fs::remove_dir_all(root).expect("remove fixture directory");
}
