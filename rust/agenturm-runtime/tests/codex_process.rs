use agenturm_kernel::{
    ExtensionId, Kernel, Observation, ObservationPage, ObservationStore, StoreCommit,
};
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
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-11-20T12:00:03Z\",\"payload\":{\"type\":\"task_aborted\",\"turn_id\":\"turn-1\"}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-11-20T12:00:01Z\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"private text never stored\"}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-11-20T12:00:01Z\",\"payload\":{\"type\":\"function_call\",\"call_id\":\"call-1\",\"name\":\"exec_command\",\"arguments\":\"private command never stored\"}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-11-20T12:00:01Z\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call-1\",\"isError\":false,\"output\":\"private output never stored\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-11-20T12:00:01Z\",\"payload\":{\"type\":\"item_completed\",\"turn_id\":\"turn-1\",\"item\":{\"type\":\"CommandExecution\",\"id\":\"check-1\",\"command\":[\"/usr/bin/zsh\",\"-lc\",\"npm run check\"],\"status\":\"completed\",\"exit_code\":0,\"aggregated_output\":\"private check output never stored\"}}}\n",
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
    assert_eq!(admission.inserted, 7);
    assert!(!admission.replay);
    assert_eq!(admission.skipped_unprojected_facts, 1);
    assert!(admission.source_ack_sent);

    let idle = kernel.refresh_with_store(&mut store);
    let idle_admission = idle.admissions.get(&source_id).expect("idle admission");
    assert_eq!(idle_admission.inserted, 0);
    assert_eq!(idle_admission.skipped_unprojected_facts, 0);
    assert!(idle_admission.source_ack_sent);
    drop(kernel);
    drop(store);

    let mut store = Store::open(&database).expect("reopen committed store");
    let cursor = store
        .source_cursor("harness.codex")
        .expect("read source cursor")
        .expect("durable cursor committed");
    assert!(!cursor.is_empty());
    let observations = store.observations().expect("read committed observations");
    assert_eq!(observations.len(), 7);
    let mut restarted_kernel = Kernel::new();
    let restored = restarted_kernel.refresh_with_store(&mut store);
    assert_eq!(restored.observations.len(), 7);
    assert_eq!(
        restored
            .observations
            .iter()
            .map(|row| row.event_id.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        observations
            .iter()
            .map(|(_, row)| row.event_id.as_str())
            .collect::<std::collections::BTreeSet<_>>()
    );
    assert!(restored.failures.is_empty());
    let (source_id, observation) = &observations[0];
    assert_eq!(source_id, "harness.codex");
    assert!(observation.event_id.len() > 32);
    assert_eq!(observation.revision, 0);
    let usage = observations
        .iter()
        .find(|(_, row)| matches!(row.kind, ObservationKind::Usage { .. }))
        .unwrap();
    assert_eq!(usage.1.observed_at_unix_ns, 1_795_176_002_000_000_000);
    assert_eq!(usage.1.identity.session_id, "session-1");
    assert_eq!(usage.1.identity.turn_id.as_deref(), Some("turn-1"));
    assert_eq!(
        usage.1.kind,
        ObservationKind::Usage {
            input_tokens: Some(7),
            output_tokens: Some(3),
            cache_read_tokens: Some(2),
            cache_write_tokens: Some(1),
            total_tokens: Some(10),
        }
    );
    assert!(observations.iter().any(|(_, row)| {
        row.kind
            == ObservationKind::Lifecycle {
                state: "interrupted".into(),
                reason_code: Some("codex_task_aborted".into()),
            }
    }));
    let tools = observations
        .iter()
        .filter(|(_, row)| matches!(row.kind, ObservationKind::Tool { .. }))
        .collect::<Vec<_>>();
    assert_eq!(tools.len(), 3);
    assert_eq!(
        observations
            .iter()
            .filter(|(_, row)| matches!(row.kind, ObservationKind::Check { .. }))
            .count(),
        1
    );
    assert!(tools.iter().all(|(_, row)| {
        row.identity
            .operation_id
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    }));
    assert!(tools.iter().any(|(_, row)| row.kind
        == ObservationKind::Tool {
            category: "shell".into(),
            result: "started".into()
        }));
    assert!(tools.iter().any(|(_, row)| row.kind
        == ObservationKind::Tool {
            category: "other".into(),
            result: "succeeded".into()
        }));
    assert!(observations.iter().any(|(_, row)| {
        row.kind
            == ObservationKind::Check {
                category: "lint".into(),
                result: "passed".into(),
            }
    }));
    let encoded = serde_json::to_string(&observations).unwrap();
    assert!(!encoded.contains("private command never stored"));
    assert!(!encoded.contains("private output never stored"));
    assert!(!encoded.contains("private check output never stored"));
    assert!(!encoded.contains("npm run check"));
    fs::remove_dir_all(root).expect("remove fixture directory");
}
