use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn native_mcp_outcome_survives_adapter_restart_without_private_tool_data() {
    let (Some(app), Some(host)) = (
        std::env::var_os("AGENTURM_RUNTIME_APP"),
        std::env::var_os("AGENTURM_RUNTIME_PLUGIN_HOST"),
    ) else {
        eprintln!("skipping configured Claude process test: set Rust app and plugin host paths");
        return;
    };
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "agenturm-claude-mcp-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    let scratch = Scratch(directory);
    let source = scratch.0.join("transcript.jsonl");
    let config = scratch.0.join("sources.json");
    let db = scratch.0.join("history.sqlite");
    let available = [
        json!({"type":"user","sessionId":"claude-session","uuid":"turn","agentId":"worker",
            "timestamp":"2026-11-20T12:00:00Z","message":{"content":"private prompt"}}),
        json!({"type":"assistant","sessionId":"claude-session","uuid":"call","agentId":"worker",
        "timestamp":"2026-11-20T12:00:01Z","message":{"stop_reason":"tool_use","content":[
          {"type":"tool_use","id":"mcp-operation","name":"mcp__github__search_code","input":{"query":"private invocation"}},
          {"type":"tool_use","id":"bad-operation","name":"mcp__server__/private-path","input":{"key":"private argument"}}
        ]}}),
    ];
    let mut file = fs::File::create(&source).unwrap();
    for record in available {
        writeln!(file, "{record}").unwrap();
    }
    drop(file);
    fs::write(
        &config,
        json!({
            "version": 1,
            "plugin_host": PathBuf::from(host),
            "sources": [{
                "plugin_id": "example.rust-claude-harness",
                "adapter": env!("CARGO_BIN_EXE_agenturm-claude-harness"),
                "source_root": source,
                "source_id": "harness.claude.fixture"
            }]
        })
        .to_string(),
    )
    .unwrap();

    let collect = || {
        check(
            Command::new(&app)
                .args(["collect", "--config"])
                .arg(&config)
                .arg("--db")
                .arg(&db),
        )
    };
    let query = || {
        check(
            Command::new(&app)
                .args(["query", "--harness-id", "harness.claude.fixture", "--db"])
                .arg(&db),
        )
    };
    assert!(collect().contains("ACK sent=true"));
    let before: Value = serde_json::from_str(&query()).unwrap();
    assert_eq!(calls(&before).len(), 1);
    assert_eq!(calls(&before)[0]["observation"]["result"], "started");

    let mut file = OpenOptions::new().append(true).open(&source).unwrap();
    for record in [
        json!({"type":"user","sessionId":"claude-session","uuid":"unmatched","agentId":"other-worker",
        "timestamp":"2026-11-20T12:00:02Z","message":{"content":[
            {"type":"tool_result","tool_use_id":"mcp-operation","content":"private cross-worker output"}
        ]}}),
        json!({"type":"user","sessionId":"claude-session","uuid":"failure","agentId":"worker",
        "timestamp":"2026-11-20T12:00:03Z","message":{"content":[
            {"type":"tool_result","tool_use_id":"mcp-operation","is_error":true,"content":"private MCP result"}
        ]}}),
    ] {
        writeln!(file, "{record}").unwrap();
    }
    drop(file);
    assert!(collect().contains("ACK sent=true"));
    let durable = query();
    let after: Value = serde_json::from_str(&durable).unwrap();
    let observations = calls(&after);
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0]["identity"], observations[1]["identity"]);
    assert_eq!(observations[0]["identity"]["session_id"], "claude-session");
    assert_eq!(observations[0]["identity"]["worker_id"], "worker");
    assert!(observations[0]["identity"]["turn_id"].is_string());
    assert!(observations[0]["identity"]["operation_id"].is_string());
    for (call, result) in observations.into_iter().zip(["started", "failed"]) {
        for (key, value) in [
            ("server", "github"),
            ("tool", "search_code"),
            ("method", "tools/call"),
            ("result", result),
        ] {
            assert_eq!(call["observation"][key], value);
        }
        assert!(call["observation"]["duration_ns"].is_null());
    }
    assert!(!durable.contains("private"));
    collect();
    assert_eq!(query(), durable);
}

fn calls(page: &Value) -> Vec<&Value> {
    page["observations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["observation"]["kind"] == "mcp_call")
        .collect()
}

fn check(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
