use agenturm_runtime::{ProcessConfig, ProcessSource, ingest_once};
use agenturm_store::{CommitOutcome, ObservationKind, Store};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    let mut store = Store::open(&database).expect("open store");
    let mut config = ProcessConfig::codex(
        PathBuf::from(plugin_host),
        PathBuf::from(adapter),
        source_path,
        "harness.codex",
    );
    config.timeout = Duration::from_secs(5);
    let mut source = ProcessSource::spawn(config).expect("start Rust Codex adapter");
    let result = ingest_once("harness.codex", &mut source, &mut store).expect("ingest fixture");
    assert_eq!(result.commit, CommitOutcome::Committed { inserted: 1 });
    assert_eq!(result.skipped_non_usage_facts, 2);
    let idle = ingest_once("harness.codex", &mut source, &mut store)
        .expect("poll after source ACK succeeds");
    assert!(matches!(
        idle.commit,
        CommitOutcome::Committed { inserted: 0 }
    ));
    assert_eq!(idle.skipped_non_usage_facts, 0);
    drop(source);
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
