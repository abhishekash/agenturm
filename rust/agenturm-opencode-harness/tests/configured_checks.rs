use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn configured_opencode_evidence_survives_ack_reopen_and_restart() {
    let (Some(app), Some(host)) = (
        std::env::var_os("AGENTURM_RUNTIME_APP"),
        std::env::var_os("AGENTURM_RUNTIME_PLUGIN_HOST"),
    ) else {
        eprintln!("skipping configured checks: set Rust app and plugin host paths");
        return;
    };

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("agenturm-opencode-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let source = root.join("native.sqlite");
    let database = root.join("history.sqlite");
    let config = root.join("collection.json");
    make_native_fixture(&source);
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "version": 1,
            "plugin_host": Path::new(&host),
            "sources": [{
                "plugin_id": "example.rust-opencode-harness",
                "adapter": env!("CARGO_BIN_EXE_agenturm-opencode-harness"),
                "source_root": source,
                "source_id": "harness.opencode",
            }],
        }))
        .unwrap(),
    )
    .unwrap();

    let mut collect = Command::new(&app);
    collect
        .arg("collect")
        .arg("--config")
        .arg(&config)
        .arg("--db")
        .arg(&database);
    let mut query = Command::new(&app);
    query
        .args(["query", "--harness-id", "harness.opencode", "--db"])
        .arg(&database);
    let first = run(&mut collect);
    assert!(first.contains("ACK sent=true"), "{first}");
    let durable = run(&mut query);
    let page: Value = serde_json::from_str(&durable).unwrap();
    let rows = page["observations"].as_array().unwrap();
    assert_eq!(rows.len(), 6);
    for (category, result) in [("test", "passed"), ("lint", "failed")] {
        let check = rows
            .iter()
            .find(|row| {
                row["observation"]
                    == json!({
                        "kind": "check", "category": category, "result": result,
                    })
            })
            .expect("typed durable check");
        assert_eq!(check["identity"]["session_id"], "ses_schema_fixture");
        assert!(check["identity"]["operation_id"].is_string());
        assert!(rows.iter().any(|row| row["observation"]["kind"] == "tool"
            && row["identity"]["operation_id"] == check["identity"]["operation_id"]));
    }
    let mut relationship_ids = Vec::new();
    for (kind, parent) in [("delegated", "ses_parent"), ("forked", "ses_original")] {
        let relation = rows
            .iter()
            .find(|row| {
                row["observation"]
                    == json!({
                        "kind":"relationship","relationship_kind":kind,"parent_session_id":parent
                    })
            })
            .expect("typed durable relationship");
        assert_eq!(relation["identity"]["session_id"], "ses_schema_fixture");
        assert!(relation["identity"]["turn_id"].is_null());
        assert!(relation["identity"]["operation_id"].is_null());
        relationship_ids.push(relation["event_id"].as_str().unwrap());
    }
    assert_ne!(relationship_ids[0], relationship_ids[1]);
    for private in [
        "pytest tests",
        "ruff check",
        "Synthetic recorded",
        "/synthetic/project",
        "/private/parent-path",
    ] {
        assert!(!durable.contains(private));
    }
    run(&mut collect);
    assert_eq!(run(&mut query), durable);
    verify_idle_correction(&source, &mut collect, &mut query);
    fs::remove_dir_all(root).unwrap();
}

fn verify_idle_correction(source: &Path, collect: &mut Command, query: &mut Command) {
    let native = Connection::open(source).unwrap();
    native
        .execute(
            "INSERT INTO session_message VALUES (
                'msg_assistant', 'ses_schema_fixture', 'assistant', 10,
                1794650400030, 1794650400030, ?1
            )",
            [r#"{"finish":"stop"}"#],
        )
        .unwrap();
    collect_lifecycle(collect, query, "running", "opencode_latest_assistant");

    native
        .execute(
            "INSERT INTO session_message VALUES (
                'msg_idle', 'ses_schema_fixture', 'idle', 20,
                1794650400040, 1794650400040, ?1
            )",
            [r#"{"outcome":"succeeded"}"#],
        )
        .unwrap();
    let completed = collect_lifecycle(collect, query, "completed", "opencode_idle_succeeded");

    native
        .execute(
            "UPDATE session_message SET data=?1, time_updated=time_updated+1 WHERE id='msg_idle'",
            [r#"{"outcome":"failed"}"#],
        )
        .unwrap();
    let corrected = collect_lifecycle(collect, query, "failed", "opencode_idle_failed");
    assert_eq!(corrected["event_id"], completed["event_id"]);
    assert!(corrected["revision"].as_u64().unwrap() > completed["revision"].as_u64().unwrap());
    assert_eq!(corrected["identity"]["session_id"], "ses_schema_fixture");
    assert!(corrected["identity"]["turn_id"].is_null());
}

fn collect_lifecycle(
    collect: &mut Command,
    query: &mut Command,
    state: &str,
    reason: &str,
) -> Value {
    let expected = json!({"kind":"lifecycle","state":state,"reason_code":reason});
    // A poll can finish an earlier bounded sweep before observing the write.
    for _ in 0..10 {
        let report = run(collect);
        assert!(report.contains("ACK sent=true"), "{report}");
        let page: Value = serde_json::from_str(&run(query)).unwrap();
        if let Some(row) = page["observations"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|row| row["observation"]["kind"] == "lifecycle")
            && row["observation"] == expected
        {
            return row.clone();
        }
    }
    panic!(
        "no latest lifecycle {expected} after ten polls: {}",
        run(query)
    );
}

fn make_native_fixture(path: &Path) {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/native_review/opencode-2.2.5.synthetic.json"
    ))
    .unwrap();
    let db = Connection::open(path).unwrap();
    for statement in fixture["schema"].as_array().unwrap() {
        db.execute_batch(statement.as_str().unwrap()).unwrap();
    }
    db.execute_batch(
        "ALTER TABLE session_v2 ADD COLUMN parent_id TEXT;
         ALTER TABLE session_v2 ADD COLUMN fork_session_id TEXT;",
    )
    .unwrap();
    for (child, delegated, forked) in [
        ("ses_schema_fixture", "ses_parent", "ses_original"),
        ("ses_self", "ses_self", "/private/parent-path"),
    ] {
        db.execute(
            "INSERT INTO session_v2
             (id, project_id, slug, directory, version, time_created, time_updated, parent_id, fork_session_id)
             VALUES (?1,'project','fixture','/synthetic/project','2.2.5',1794650400000,1794650400020,?2,?3)",
            params![child, delegated, forked],
        )
        .unwrap();
    }
    let pass = &fixture["rows"]["session_part"][1]["data"];
    let mut fail = pass.clone();
    fail["state"]["input"]["command"] = json!("ruff check src");
    fail["state"]["metadata"]["exit"] = json!(1);
    for (ordinal, data) in [pass, &fail].into_iter().enumerate() {
        db.execute(
            "INSERT INTO session_part VALUES (
                'msg_check', 'ses_schema_fixture', ?1, 'tool', NULL,
                1794650400010, 1794650400020, ?2
            )",
            params![i64::try_from(ordinal).unwrap(), data.to_string()],
        )
        .unwrap();
    }
}

fn run(command: &mut Command) -> String {
    let Output {
        status,
        stdout,
        stderr,
    } = command.output().unwrap();
    let stdout = String::from_utf8(stdout).unwrap();
    assert!(
        status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&stderr)
    );
    stdout
}

#[test]
fn opencode_turns_and_granted_prompts_survive_collection_and_reopen() {
    use sha2::{Digest, Sha256};

    let (Some(app), Some(host)) = (
        std::env::var_os("AGENTURM_RUNTIME_APP"),
        std::env::var_os("AGENTURM_RUNTIME_PLUGIN_HOST"),
    ) else {
        eprintln!("skipping OpenCode prompt proof: set Rust app and plugin host paths");
        return;
    };
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("agenturm-opencode-proof-{unique}"));
    fs::create_dir(&root).unwrap();
    let source = root.join("native.sqlite");
    let history = root.join("history.sqlite");
    let config = root.join("sources.json");
    let catalog = root.join("catalog.json");
    let adapter = root.join("adapter");
    fs::copy(env!("CARGO_BIN_EXE_agenturm-opencode-harness"), &adapter).unwrap();
    make_native_fixture(&source);
    let native = Connection::open(&source).unwrap();
    for (seq, id, role, payload) in [
        (1, "msg_before", "idle", r#"{"outcome":"succeeded"}"#),
        (
            2,
            "msg_request",
            "user",
            r#"{"text":"review fixture scenario"}"#,
        ),
        (3, "msg_end", "idle", r#"{"outcome":"failed"}"#),
        (
            4,
            "msg_next",
            "user",
            r#"{"text":"check next fixture scenario"}"#,
        ),
        (
            5,
            "msg_inline",
            "assistant",
            r#"{"content":[{"type":"tool","name":"mcp__fixture__lookup","id":"native-op","time":{"ran":30,"completed":34},"state":{"status":"completed","input":{"text":"synthetic private tool input"}}}]}"#,
        ),
    ] {
        native
            .execute(
                "INSERT INTO session_message VALUES
                 (?1,'ses_schema_fixture',?2,?3,1794650400000+?3,1794650400000+?3,?4)",
                params![id, role, seq, payload],
            )
            .unwrap();
    }
    drop(native);
    fs::write(
        &catalog,
        serde_json::to_vec(&json!({
            "version":1,
            "plugins":[{
                "plugin_id":"example.rust-opencode-harness", "adapter":adapter,
                "sha256":format!("{:x}", Sha256::digest(fs::read(&adapter).unwrap())),
                "protocol":1, "features":["source-adapter/1","configured-root/1"],
                "observation_format":"agenturm-observations/1"
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(&config, serde_json::to_vec(&json!({
        "version":2, "plugin_host":Path::new(&host), "plugin_catalog":catalog,
        "sources":[{"plugin_id":"example.rust-opencode-harness","source_root":source,"source_id":"harness.opencode"}]
    })).unwrap()).unwrap();
    let args = ["--db", history.to_str().unwrap()];
    run(Command::new(&app)
        .arg("collect")
        .args(args)
        .arg("--config")
        .arg(&config));
    let query = run(Command::new(&app).arg("query").args(args));
    assert!(!query.contains("review fixture scenario"));
    assert!(!query.contains("synthetic private tool input"));
    let json: Value = serde_json::from_str(&query).unwrap();
    let rows = json["observations"].as_array().unwrap();
    let failed = rows
        .iter()
        .find(|row| {
            row["observation"]["state"] == "failed" && row["identity"]["turn_id"].is_string()
        })
        .unwrap();
    let mcp = rows
        .iter()
        .find(|row| row["observation"]["kind"] == "mcp_call")
        .unwrap();
    assert_eq!(mcp["observation"]["server"], "fixture");
    assert_eq!(mcp["observation"]["tool"], "lookup");
    assert_eq!(mcp["observation"]["duration_ns"], "4000000");
    assert!(rows.iter().any(|row| row["observation"]["kind"] == "tool"
        && row["identity"]["operation_id"] == mcp["identity"]["operation_id"]));
    let request = |action| {
        let mut command = Command::new(&app);
        command
            .args(["content", action])
            .args(args)
            .args(["--source-id", "harness.opencode"]);
        if action != "revoke" {
            command.arg("--config").arg(&config);
        }
        command
    };
    assert!(
        !request("list")
            .args(["--session-id", "ses_schema_fixture"])
            .output()
            .unwrap()
            .status
            .success()
    );
    run(&mut request("grant"));
    let listed = run(request("list").args(["--session-id", "ses_schema_fixture"]));
    let listed: Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 2);
    assert_eq!(listed[0]["turn_id"], failed["identity"]["turn_id"]);
    assert_ne!(listed[0]["turn_id"], listed[1]["turn_id"]);
    assert_eq!(listed[1]["turn_id"], mcp["identity"]["turn_id"]);
    let mut preview = request("preview");
    let event = listed[0]["event_id"].as_str().unwrap();
    preview.args(["--event-id", event, "--revision", "0"]);
    assert_eq!(run(&mut preview).trim(), r#""review fixture scenario""#);

    let collect = || {
        run(Command::new(&app)
            .arg("collect")
            .args(args)
            .arg("--config")
            .arg(&config));
    };
    let mutate = |id, data| {
        Connection::open(&source)
            .unwrap()
            .execute(
                "UPDATE session_message SET data=?1 WHERE id=?2",
                params![data, id],
            )
            .unwrap();
    };
    mutate("msg_end", r#"{"outcome":"interrupted"}"#);
    collect();
    assert_eq!(run(&mut preview).trim(), r#""review fixture scenario""#);
    let current = run(request("list").args(["--session-id", "ses_schema_fixture"]));
    let current: Value = serde_json::from_str(&current).unwrap();
    assert_eq!(current.as_array().unwrap().len(), 2);
    assert_eq!(current[0]["event_id"], event);
    assert_eq!(current[0]["revision"], 1);

    let bundle = root.join("bundle");
    let restored = root.join("restored");
    run(Command::new(&app)
        .arg("backup")
        .args(args)
        .arg("--bundle")
        .arg(&bundle));
    run(Command::new(&app)
        .arg("restore")
        .arg("--bundle")
        .arg(&bundle)
        .arg("--output")
        .arg(&restored));
    assert!(
        !Command::new(&app)
            .args(["content", "preview", "--db"])
            .arg(restored.join("history.sqlite"))
            .args([
                "--source-id",
                "harness.opencode",
                "--event-id",
                event,
                "--revision",
                "0",
                "--config"
            ])
            .arg(&config)
            .output()
            .unwrap()
            .status
            .success()
    );

    run(&mut request("revoke"));
    assert!(!preview.output().unwrap().status.success());
    run(&mut request("grant"));
    assert_eq!(run(&mut preview).trim(), r#""review fixture scenario""#);

    // Updating native bytes without changing the recorded timestamp must
    // fail even after the collector has committed a current sweep again.
    mutate("msg_request", r#"{"text":"modified fixture scenario"}"#);
    assert!(!preview.output().unwrap().status.success());
    collect();
    assert!(!preview.output().unwrap().status.success());
    mutate("msg_request", r#"{"text":"review fixture scenario"}"#);
    collect();
    assert_eq!(run(&mut preview).trim(), r#""review fixture scenario""#);

    let replacement = root.join("replacement.sqlite");
    fs::rename(&source, &replacement).unwrap();
    fs::copy(&replacement, &source).unwrap();
    assert!(!preview.output().unwrap().status.success());
    fs::rename(&replacement, &source).unwrap();
    collect();
    assert_eq!(run(&mut preview).trim(), r#""review fixture scenario""#);

    run(Command::new(&app)
        .arg("prune")
        .args(args)
        .args(["--before-unix-ns", "1794650400003000000"]));
    assert!(!preview.output().unwrap().status.success());
    let newer = run(request("list").args(["--session-id", "ses_schema_fixture"]));
    let newer: Value = serde_json::from_str(&newer).unwrap();
    assert_eq!(newer.as_array().unwrap().len(), 1);
    fs::remove_dir_all(root).unwrap();
}
