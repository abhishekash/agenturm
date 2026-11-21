use agenturm_app::execute_inspect;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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
    assert!(summary.contains("harness.pi: committed 1 usage observation(s)"));
    assert!(summary.contains("harness.codex: committed 1 usage observation(s)"));
    assert!(summary.contains("harness.broken: startup failed:"));
    assert!(summary.contains("ACK sent=true"));

    let snapshot = execute_inspect(&database).expect("reopen committed collection history");
    assert_eq!(snapshot.observations.len(), 2);
    let session_ids = snapshot
        .observations
        .iter()
        .map(|observation| observation.identity.session_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(session_ids, ["codex-session", "pi-session"].into());
    assert!(snapshot.failures.is_empty());

    drop(snapshot);
    fs::remove_dir_all(root).expect("remove fixture directory");
}
