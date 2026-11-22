#![cfg(target_os = "linux")]

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn preview_requires_a_current_grant_and_exact_committed_pi_bytes() {
    let (Some(plugin_host), Some(adapter)) = (
        std::env::var_os("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var_os("AGENTURM_RUNTIME_PI_ADAPTER"),
    ) else {
        eprintln!("skipping Pi content process test: set plugin host and Pi adapter paths");
        return;
    };
    let scratch = Scratch::new();
    let adapter = pin_fixture_adapter(&scratch, &adapter);
    let source = scratch.0.join("session.jsonl");
    let config = scratch.0.join("config.json");
    let database = scratch.0.join("history.sqlite");
    let catalog = scratch.0.join("catalog.json");
    let initial = concat!(
        r#"{"type":"session","id":"pi-fixture"}"#,
        "\n",
        r#"{"type":"message","id":"turn-1","timestamp":"2026-11-20T12:00:00Z","message":{"role":"user","content":[{"type":"text","text":"review message"},{"type":"image","data":"PRIVATE_IMAGE_BYTES"}]}}"#,
        "\n",
        r#"{"type":"message","id":"turn-2","timestamp":"2026-11-20T12:00:01Z","message":{"role":"user","content":"API_KEY=fixture-private-value"}}"#,
        "\n",
        r#"{"type":"message","id":"turn-safe","timestamp":"2026-11-20T12:00:01.500Z","message":{"role":"user","content":"second private prompt"}}"#,
        "\n",
    );
    fs::write(&source, initial).unwrap();
    let artifact_sha256 = format!("{:x}", Sha256::digest(fs::read(&adapter).unwrap()));
    fs::write(
        &catalog,
        serde_json::to_vec(&json!({
            "version": 1,
            "plugins": [{
                "plugin_id": "example.rust-pi-harness",
                "adapter": adapter,
                "sha256": artifact_sha256,
                "protocol": 1,
                "features": ["source-adapter/1", "configured-root/1"],
                "observation_format": "agenturm-observations/1",
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "version": 2,
            "plugin_host": PathBuf::from(plugin_host),
            "plugin_catalog": catalog,
            "sources": [{
                "plugin_id": "example.rust-pi-harness",
                "source_root": source,
                "source_id": "harness.pi",
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    let (config_arg, db_arg) = (config.to_str().unwrap(), database.to_str().unwrap());
    let collect = || run(&["collect", "--config", config_arg, "--db", db_arg]);
    assert!(checked(collect()).contains("ACK sent=true"));
    let query = checked(run(&["query", "--db", db_arg]));
    for private in [
        "review message",
        "PRIVATE_IMAGE_BYTES",
        "fixture-private-value",
        "second private prompt",
    ] {
        assert!(!query.contains(private));
    }
    let history: Value = serde_json::from_str(&query).unwrap();
    let event = |turn: &str| {
        history["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| {
                row["identity"]["turn_id"] == turn && row["observation"]["kind"] == "message"
            })
            .unwrap()["event_id"]
            .as_str()
            .unwrap()
    };
    let content = |action: &str, args: &[&str]| {
        let mut command = vec![
            "content",
            action,
            "--db",
            db_arg,
            "--source-id",
            "harness.pi",
        ];
        command.extend_from_slice(args);
        run(&command)
    };
    let list = |session_id: &str| {
        content(
            "list",
            &["--config", config_arg, "--session-id", session_id],
        )
    };
    let approve = || content("grant", &["--config", config_arg]);
    let preview = |event_id: &str| {
        content(
            "preview",
            &[
                "--config",
                config_arg,
                "--event-id",
                event_id,
                "--revision",
                "0",
            ],
        )
    };

    denied(list("pi-fixture"), "requires approval");
    denied(preview(event("turn-1")), "requires approval");
    checked(approve());
    let references = checked(list("pi-fixture"));
    assert_eq!(
        serde_json::from_str::<Value>(&references).unwrap(),
        json!([
            {
                "event_id": event("turn-1"),
                "revision": 0,
                "turn_id": "turn-1",
                "observed_at_unix_ns": 1795176000000000000u64,
            },
            {
                "event_id": event("turn-safe"),
                "revision": 0,
                "turn_id": "turn-safe",
                "observed_at_unix_ns": 1795176001500000000u64,
            },
        ])
    );
    assert_eq!(checked(list("another-session")), "[]\n");
    denied(
        content(
            "preview",
            &[
                "--config",
                config_arg,
                "--event-id",
                event("turn-1"),
                "--revision",
                "0",
                "--session-id",
                "another-session",
            ],
        ),
        "selected revision is unavailable",
    );
    assert_eq!(checked(preview(event("turn-1"))), "\"review message\"\n");
    assert_eq!(
        checked(content(
            "preview",
            &[
                "--config",
                config_arg,
                "--event-id",
                event("turn-safe"),
                "--revision",
                "0",
                "--session-id",
                "pi-fixture",
            ],
        )),
        "\"second private prompt\"\n"
    );
    denied(preview(event("turn-2")), "possible credential");
    denied(preview("pi:forged"), "selected revision is unavailable");

    fs::write(&source, initial.replace("review message", "changed source")).unwrap();
    denied(
        preview(event("turn-1")),
        "no longer matches the committed checkpoint",
    );
    denied(
        list("pi-fixture"),
        "no longer matches the committed checkpoint",
    );
    fs::write(&source, initial).unwrap();
    assert_eq!(checked(preview(event("turn-1"))), "\"review message\"\n");

    let holder = agenturm_store::Store::open(&database).unwrap();
    denied(list("pi-fixture"), "writer lease unavailable");
    denied(preview(event("turn-1")), "writer lease unavailable");
    drop(holder);
    checked(content("revoke", &[]));
    denied(list("pi-fixture"), "requires approval");
    denied(preview(event("turn-1")), "requires approval");
    checked(approve());

    fs::write(
        &source,
        format!(
            "{initial}{}\n",
            r#"{"type":"message","id":"turn-3","timestamp":"2026-11-20T12:00:02Z","message":{"role":"user","content":"later message"}}"#
        ),
    )
    .unwrap();
    assert_eq!(checked(preview(event("turn-1"))), "\"review message\"\n");
    checked(collect());
    assert_eq!(checked(preview(event("turn-1"))), "\"review message\"\n");
    let newer: Value = serde_json::from_str(&checked(list("pi-fixture"))).unwrap();
    assert_eq!(newer.as_array().unwrap().len(), 3);
    assert_eq!(newer[0]["turn_id"], "turn-1");
    assert_eq!(newer[2]["turn_id"], "turn-3");

    let replacement = scratch.0.join("replacement.jsonl");
    fs::copy(&source, &replacement).unwrap();
    fs::rename(replacement, &source).unwrap();
    denied(list("pi-fixture"), "requires approval");
    denied(preview(event("turn-1")), "requires approval");
}

#[test]
fn codex_preview_reveals_only_exact_completed_native_user_items() {
    let (Some(plugin_host), Some(adapter)) = (
        std::env::var_os("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var_os("AGENTURM_RUNTIME_CODEX_ADAPTER"),
    ) else {
        eprintln!("skipping Codex content process test: set plugin host and Codex adapter paths");
        return;
    };
    let scratch = Scratch::new();
    let adapter = pin_fixture_adapter(&scratch, &adapter);
    let source = scratch.0.join("session.jsonl");
    let config = scratch.0.join("config.json");
    let database = scratch.0.join("history.sqlite");
    let catalog = scratch.0.join("catalog.json");
    let plugin_id = agenturm_runtime::CODEX_PLUGIN_ID;
    let initial = concat!(
        r#"{"type":"session_meta","payload":{"id":"codex-fixture"}}"#,
        "\n",
        r#"{"type":"response_item","timestamp":"2026-11-20T12:00:00Z","payload":{"type":"message","role":"user","turn_id":"foreign","content":[{"type":"input_text","text":"synthetic inherited context"}]}}"#,
        "\n",
        r#"{"type":"event_msg","timestamp":"2026-11-20T12:00:01Z","payload":{"type":"item_completed","thread_id":"codex-fixture","turn_id":"turn-1","item":{"type":"UserMessage","content":[{"type":"text","text":"review recorded output"},{"type":"image","data":"SYNTHETIC_IMAGE"}]}}}"#,
        "\n",
        r#"{"type":"event_msg","timestamp":"2026-11-20T12:00:02Z","payload":{"type":"item_completed","thread_id":"other-thread","turn_id":"turn-2","item":{"type":"UserMessage","content":[{"type":"text","text":"synthetic wrong thread"}]}}}"#,
        "\n",
    );
    fs::write(&source, initial).unwrap();
    fs::write(
        &catalog,
        serde_json::to_vec(&json!({
            "version": 1,
            "plugins": [{
                "plugin_id": plugin_id,
                "adapter": &adapter,
                "sha256": format!("{:x}", Sha256::digest(fs::read(&adapter).unwrap())),
                "protocol": 1,
                "features": ["source-adapter/1", "configured-root/1"],
                "observation_format": "agenturm-observations/1",
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "version": 2,
            "plugin_host": PathBuf::from(plugin_host),
            "plugin_catalog": catalog,
            "sources": [{"plugin_id": plugin_id, "source_root": source, "source_id": "harness.codex"}],
        })).unwrap(),
    ).unwrap();
    let (config_arg, db_arg) = (config.to_str().unwrap(), database.to_str().unwrap());
    checked(run(&["collect", "--config", config_arg, "--db", db_arg]));
    let content = |action: &str, args: &[&str]| {
        let mut command = vec![
            "content",
            action,
            "--db",
            db_arg,
            "--source-id",
            "harness.codex",
        ];
        command.extend_from_slice(args);
        run(&command)
    };
    let list = || {
        content(
            "list",
            &["--config", config_arg, "--session-id", "codex-fixture"],
        )
    };
    denied(list(), "requires approval");
    checked(content("grant", &["--config", config_arg]));
    let references = checked(list());
    let references = serde_json::from_str::<Value>(&references).unwrap();
    let rows = references.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["turn_id"], "turn-1");
    let event_id = rows[0]["event_id"].as_str().unwrap();
    let preview = || {
        content(
            "preview",
            &[
                "--config",
                config_arg,
                "--event-id",
                event_id,
                "--revision",
                "0",
            ],
        )
    };
    assert_eq!(checked(preview()), "\"review recorded output\"\n");
    assert!(!checked(run(&["query", "--db", db_arg])).contains("review recorded output"));
    fs::write(
        &source,
        initial.replace("review recorded output", "edited recorded output"),
    )
    .unwrap();
    denied(preview(), "no longer matches the committed checkpoint");
    checked(content("revoke", &[]));
    denied(list(), "requires approval");
}

#[test]
fn claude_and_kiro_preview_the_exact_native_user_rows() {
    for (kind, session, member, rows) in [
        (
            "claude",
            "claude-fixture",
            "session.jsonl",
            concat!(
                r#"{"type":"summary","sessionId":"claude-fixture"}"#,
                "\n",
                r#"{"type":"user","sessionId":"claude-fixture","uuid":"prompt-1","timestamp":"2026-11-20T12:00:00Z","message":{"content":"review synthetic handoff"}}"#,
                "\n",
                r#"{"type":"user","sessionId":"claude-fixture","uuid":"meta","isMeta":true,"timestamp":"2026-11-20T12:00:01Z","message":{"content":"synthetic metadata"}}"#,
                "\n",
            ),
        ),
        (
            "kiro",
            "sess_kiro_fixture",
            "sess_kiro_fixture/messages.jsonl",
            concat!(
                r#"{"id":"prompt-1","timestamp":"2026-11-20T12:00:00Z","payload":{"type":"user","content":"review synthetic handoff"}}"#,
                "\n",
                r#"{"id":"steer","timestamp":"2026-11-20T12:00:01Z","payload":{"type":"user","source":"steer","content":"synthetic steering"}}"#,
                "\n",
            ),
        ),
    ] {
        let (Some(plugin_host), Some(adapter)) = (
            std::env::var_os("AGENTURM_RUNTIME_PLUGIN_HOST"),
            std::env::var_os(format!(
                "AGENTURM_RUNTIME_{}_ADAPTER",
                kind.to_ascii_uppercase()
            )),
        ) else {
            eprintln!("skipping {kind} prompt process test: set the plugin host and adapter paths");
            continue;
        };
        let scratch = Scratch::new();
        let adapter = pin_fixture_adapter(&scratch, &adapter);
        let source = scratch.0.join(member);
        let config = scratch.0.join("config.json");
        let database = scratch.0.join("history.sqlite");
        let catalog = scratch.0.join("catalog.json");
        let plugin_id = format!("example.rust-{kind}-harness");
        let source_id = format!("harness.{kind}");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, rows).unwrap();
        fs::write(
            &catalog,
            serde_json::to_vec(&json!({
                "version": 1,
                "plugins": [{
                    "plugin_id": plugin_id,
                    "adapter": &adapter,
                    "sha256": format!("{:x}", Sha256::digest(fs::read(&adapter).unwrap())),
                    "protocol": 1,
                    "features": ["source-adapter/1", "configured-root/1"],
                    "observation_format": "agenturm-observations/1",
                }],
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(&config, serde_json::to_vec(&json!({
            "version": 2,
            "plugin_host": PathBuf::from(plugin_host),
            "plugin_catalog": catalog,
            "sources": [{"plugin_id": plugin_id, "source_root": source, "source_id": source_id}],
        })).unwrap()).unwrap();
        let (config_arg, db_arg) = (config.to_str().unwrap(), database.to_str().unwrap());
        assert!(
            checked(run(&["collect", "--config", config_arg, "--db", db_arg]))
                .contains("ACK sent=true")
        );
        let content = |action: &str, args: &[&str]| {
            let mut command = vec!["content", action, "--db", db_arg, "--source-id", &source_id];
            command.extend_from_slice(args);
            run(&command)
        };
        let list = || content("list", &["--config", config_arg, "--session-id", session]);
        denied(list(), "requires approval");
        checked(content("grant", &["--config", config_arg]));
        let references = serde_json::from_str::<Value>(&checked(list())).unwrap();
        let rows = references.as_array().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "{kind} should show only the exact eligible user row"
        );
        let event_id = rows[0]["event_id"].as_str().unwrap();
        let revision = rows[0]["revision"].as_u64().unwrap();
        assert_eq!(revision > 0, kind == "claude");
        let preview = checked(content(
            "preview",
            &[
                "--config",
                config_arg,
                "--event-id",
                event_id,
                "--revision",
                &revision.to_string(),
            ],
        ));
        assert_eq!(preview, "\"review synthetic handoff\"\n");
        checked(content("revoke", &[]));
        denied(list(), "requires approval");
    }
}

fn pin_fixture_adapter(root: &Scratch, built: &std::ffi::OsStr) -> PathBuf {
    let adapter = root.0.join("adapter");
    fs::copy(built, &adapter).unwrap();
    adapter
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args(args)
        .output()
        .unwrap()
}

fn checked(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn denied(output: Output, expected: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let errors = String::from_utf8(output.stderr).unwrap();
    assert!(errors.contains(expected), "{errors}");
    assert!(!errors.contains("fixture-private-value"));
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agenturm-pi-preview-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
