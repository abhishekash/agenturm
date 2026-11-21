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
    );
    fs::write(&source, initial).unwrap();
    let artifact_sha256 = format!("{:x}", Sha256::digest(fs::read(&adapter).unwrap()));
    fs::write(
        &catalog,
        serde_json::to_vec(&json!({
            "version": 1,
            "plugins": [{
                "plugin_id": "example.rust-pi-harness",
                "adapter": PathBuf::from(adapter),
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

    denied(preview(event("turn-1")), "requires approval");
    checked(approve());
    assert_eq!(checked(preview(event("turn-1"))), "\"review message\"\n");
    denied(preview(event("turn-2")), "possible credential");
    denied(preview("pi:forged"), "selected revision is unavailable");

    fs::write(&source, initial.replace("review message", "changed source")).unwrap();
    denied(
        preview(event("turn-1")),
        "no longer matches the committed checkpoint",
    );
    fs::write(&source, initial).unwrap();
    assert_eq!(checked(preview(event("turn-1"))), "\"review message\"\n");

    let holder = agenturm_store::Store::open(&database).unwrap();
    denied(preview(event("turn-1")), "writer lease unavailable");
    drop(holder);
    checked(content("revoke", &[]));
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
    denied(preview(event("turn-1")), "selected revision is unavailable");

    let replacement = scratch.0.join("replacement.jsonl");
    fs::copy(&source, &replacement).unwrap();
    fs::rename(replacement, &source).unwrap();
    denied(preview(event("turn-1")), "requires approval");
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
