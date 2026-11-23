use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationStore,
};
use agenturm_store::{Store, WorkspaceAnchor, WorkspaceLedger};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn tasks_share_the_cockpit_ledger_and_validate_exact_history_links() {
    let scratch = Scratch::new();
    let database = scratch.0.join("history.sqlite");
    let mut writer = Store::open(&database).unwrap();
    let codex = seed(&mut writer, "harness.codex");
    let pi = seed(&mut writer, "harness.pi");
    let ledger = WorkspaceLedger::open(&database).unwrap();

    let empty = json(&database, &["list"]);
    assert_eq!(empty["items"].as_array().unwrap().len(), 0);
    assert!(!scratch.0.join("history.sqlite.workspace.sqlite").exists());

    let initial = ledger.create_task(Some(&pi), "Cockpit task", "").unwrap();
    let cli = json(
        &database,
        &["add", "--title", "CLI task", "--notes", "Operator note"],
    );
    let task_id = cli["id"].as_u64().unwrap();
    let id = task_id.to_string();
    assert_eq!(cli["notes"], "Operator note");
    assert!(cli["anchor"].is_null());

    let newest = json(&database, &["list", "--limit", "1"]);
    assert_eq!(newest["items"][0], cli);
    assert_eq!(newest["next_before"], task_id);
    let older = json(&database, &["list", "--before", &id]);
    assert_eq!(older["items"][0]["id"], initial.id);
    assert_eq!(older["items"][0]["anchor"]["source_id"], "harness.pi");
    assert!(older["next_before"].is_null());

    let wrong_source = [
        "link",
        &id,
        "--source-id",
        "harness.unknown",
        "--session-id",
        "session",
    ];
    assert_failure(run(&database, &wrong_source));
    let exact = [
        &id,
        "--source-id",
        "harness.codex",
        "--session-id",
        "session",
        "--turn-id",
        "turn",
        "--worker-id",
        "worker",
    ];
    assert_eq!(
        json(&database, &[&["link"], exact.as_slice()].concat())["changed"],
        true
    );
    assert_eq!(
        json(&database, &[&["link"], exact.as_slice()].concat())["changed"],
        false
    );
    assert_eq!(ledger.task_links(task_id, 10).unwrap(), vec![codex.clone()]);

    let shown = json(&database, &["show", &id]);
    assert_eq!(shown["task"]["anchor"], shown["links"][0]);
    assert_eq!(shown["links"][0]["worker_id"], "worker");
    assert_eq!(shown["links"][0]["turn_id"], "turn");
    assert!(shown["git_snapshot"].is_null());
    assert_eq!(json(&database, &["complete", &id])["completed"], true);
    assert!(ledger.list_tasks(1, None).unwrap().items[0].completed);
    assert_eq!(json(&database, &["reopen", &id])["completed"], false);
    assert!(!ledger.list_tasks(1, None).unwrap().items[0].completed);
    assert_eq!(
        json(&database, &[&["unlink"], exact.as_slice()].concat())["changed"],
        true
    );
    assert!(ledger.task_links(task_id, 10).unwrap().is_empty());
    assert!(json(&database, &["show", &id])["task"]["anchor"].is_null());

    let handoff = ledger
        .create_handoff(&codex, &pi, "Continue <this>")
        .unwrap();
    let entries = json(&database, &["handoffs"]);
    assert_eq!(entries["items"][0]["id"], handoff.id);
    assert_eq!(entries["items"][0]["from"]["source_id"], "harness.codex");
    assert_eq!(entries["items"][0]["to"]["source_id"], "harness.pi");
    let exported = json(&database, &["export", &handoff.id.to_string()]);
    let markdown = Path::new(exported["markdown"].as_str().unwrap());
    let html = Path::new(exported["html"].as_str().unwrap());
    assert!(markdown.starts_with(database.parent().unwrap().join("handoffs")));
    assert!(
        std::fs::read_to_string(markdown)
            .unwrap()
            .contains("Continue <this>")
    );
    assert!(
        std::fs::read_to_string(html)
            .unwrap()
            .contains("Continue &lt;this&gt;")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(html).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_failure(run(&database, &["show", "999"]));
    assert_failure(run(&database, &["export", "999"]));
}

#[test]
#[cfg(target_os = "linux")]
fn source_backed_task_drafts_require_the_same_exact_live_grant_and_keep_authored_state() {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    let (Some(host), Some(adapter)) = (
        std::env::var_os("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var_os("AGENTURM_RUNTIME_PI_ADAPTER"),
    ) else {
        eprintln!("skipping source-backed task process test: set plugin host and Pi adapter paths");
        return;
    };
    let root = Scratch::new();
    let db = root.0.join("history.sqlite");
    let source = root.0.join("source.jsonl");
    let adapter_copy = root.0.join("adapter");
    let catalog = root.0.join("catalog.json");
    let config = root.0.join("sources.json");
    let draft = root.0.join("draft.json");
    let unbound = root.0.join("unbound.json");
    let review_file = root.0.join("review.json");
    let pending_review_draft = root.0.join("pending-review-draft.json");
    let content = "synthetic retained prompt excerpt";
    let rows = concat!(
        "{\"type\":\"session\",\"id\":\"pi-draft\"}\n",
        "{\"type\":\"message\",\"id\":\"turn-1\",\"timestamp\":\"2026-11-20T12:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"synthetic retained prompt excerpt\"}}\n",
    );
    std::fs::write(&source, rows).unwrap();
    std::fs::copy(adapter, &adapter_copy).unwrap();
    std::fs::write(&catalog, json!({"version":1,"plugins":[{
        "plugin_id":"example.rust-pi-harness","adapter":adapter_copy,
        "sha256":format!("{:x}", Sha256::digest(std::fs::read(&adapter_copy).unwrap())),
        "protocol":1,"features":["source-adapter/1","configured-root/1"],"observation_format":"agenturm-observations/1",
    }]}).to_string()).unwrap();
    std::fs::write(&config, json!({"version":2,"plugin_host":PathBuf::from(host),"plugin_catalog":catalog,
        "sources":[{"plugin_id":"example.rust-pi-harness","source_id":"harness.pi","source_root":source}],
    }).to_string()).unwrap();
    let (db_arg, config_arg) = (db.to_str().unwrap(), config.to_str().unwrap());
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_agenturm"))
            .args(args)
            .output()
            .unwrap()
    };
    let check = |output: Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        )
    };
    check(invoke(&["collect", "--db", db_arg, "--config", config_arg]));
    let history = invoke(&["query", "--db", db_arg]);
    assert!(!String::from_utf8_lossy(&history.stdout).contains(content));
    let history: Value = serde_json::from_slice(&history.stdout).unwrap();
    let event = history["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["observation"]["kind"] == "message")
        .unwrap()["event_id"]
        .as_str()
        .unwrap();
    let review = invoke(&[
        "review",
        "--db",
        db_arg,
        "--harness-id",
        "harness.pi",
        "--session-id",
        "pi-draft",
        "--turn-id",
        "turn-1",
        "--format",
        "json",
    ]);
    assert!(review.status.success());
    assert!(!String::from_utf8_lossy(&review.stdout).contains(content));
    let receipt: Value = serde_json::from_slice(&review.stdout).unwrap();
    assert!(receipt["history_identity"].is_string());
    std::fs::write(&review_file, &review.stdout).unwrap();
    let review_args = [
        "draft",
        "--config",
        config_arg,
        "--review-file",
        review_file.to_str().unwrap(),
    ];
    assert_failure(run(&db, &review_args));
    let task = self::json(
        &db,
        &[
            "add",
            "--title",
            "Authored title",
            "--notes",
            "Authored note",
        ],
    );
    let id = task["id"].as_u64().unwrap().to_string();
    self::json(&db, &["brief", &id, "--intent", "Authored intent"]);
    let draft_args = |turn, path| {
        vec![
            "draft",
            "--config",
            config_arg,
            "--source-id",
            "harness.pi",
            "--session-id",
            "pi-draft",
            "--turn-id",
            turn,
            "--event-id",
            event,
            "--revision",
            "0",
            "--output",
            path,
        ]
    };
    let acquire = || {
        invoke(&[
            "content",
            "grant",
            "--db",
            db_arg,
            "--source-id",
            "harness.pi",
            "--config",
            config_arg,
        ])
    };
    let draft_path = draft.to_str().unwrap();
    let unbound_path = unbound.to_str().unwrap();
    let prepare = |turn| {
        let mut args = draft_args(turn, draft_path);
        args.extend(["--task-id", &id]);
        run(&db, &args)
    };
    assert_failure(prepare("turn-1"));
    assert!(!draft.exists());
    check(acquire());
    let mut forged_review = receipt.clone();
    for (name, altered) in [
        ("history_identity", json!("1".repeat(32))),
        ("history_identity", Value::Null),
    ] {
        forged_review[name] = altered;
        std::fs::write(&review_file, forged_review.to_string()).unwrap();
        assert_failure(run(&db, &review_args));
    }
    let mut forged_review = receipt.clone();
    let selected = forged_review["observations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["event_id"] == event)
        .unwrap();
    selected["observed_at_unix_ns"] = json!("1");
    std::fs::write(&review_file, forged_review.to_string()).unwrap();
    assert_failure(run(&db, &review_args));
    std::fs::write(&review_file, &review.stdout).unwrap();
    let pinned = self::json(&db, &review_args);
    assert_eq!(
        pinned["review"]["history_identity"],
        receipt["history_identity"]
    );
    assert_eq!(pinned["excerpt"], content);
    assert_eq!(pinned["source"]["event_id"], event);
    let restored_copy = root.0.join("restored-copy.sqlite");
    Store::open_read_only(&db)
        .unwrap()
        .backup_to(&restored_copy)
        .unwrap();
    check(invoke(&[
        "content",
        "grant",
        "--db",
        restored_copy.to_str().unwrap(),
        "--source-id",
        "harness.pi",
        "--config",
        config_arg,
    ]));
    assert_failure(run(&restored_copy, &review_args));
    assert_failure(prepare("another-turn"));
    check(prepare("turn-1"));
    assert_failure(prepare("turn-1"));
    assert_eq!(
        std::fs::metadata(&draft).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(self::json(&db, &["show", &id])["task"]["revision"], 2);
    let approve = |path, db: &Path| run(db, &["approve-draft", path, "--config", config_arg]);
    let mut editable: Value = serde_json::from_slice(&std::fs::read(&draft).unwrap()).unwrap();
    assert_eq!(editable["excerpt"], content);
    editable["excerpt"] = json!("forged excerpt");
    std::fs::write(&draft, editable.to_string()).unwrap();
    assert_failure(approve(draft_path, &db));
    editable["excerpt"] = json!(content);
    editable["continuation"] = json!("Operator-authored next step");
    std::fs::write(&draft, editable.to_string()).unwrap();
    std::fs::write(&source, rows.replace(content, "changed source bytes")).unwrap();
    assert_failure(approve(draft_path, &db));
    std::fs::write(&source, rows).unwrap();
    let approved = self::json(
        &db,
        &[
            "approve-draft",
            draft_path,
            "--config",
            config_arg,
            "--expected-revision",
            "2",
        ],
    );
    assert_eq!(approved["task"]["title"], "Authored title");
    assert_eq!(approved["task"]["notes"], "Authored note");
    assert_eq!(approved["task"]["revision"], 3);
    assert_eq!(
        self::json(&db, &["brief", &id])["brief"]["intent"],
        "Authored intent"
    );
    assert_eq!(
        self::json(&db, &["show", &id])["links"][0]["turn_id"],
        "turn-1"
    );
    assert_eq!(approve(draft_path, &db).status.code(), Some(3));
    let approval_id = approved["approval"]["id"].as_u64().unwrap().to_string();
    let reveal_args = [
        "continuity",
        &id,
        "--config",
        config_arg,
        "--approval-id",
        &approval_id,
    ];
    assert_eq!(self::json(&db, &reveal_args)["excerpt"], content);

    check(run(
        &db,
        &[
            &review_args[..],
            &[
                "--event-id",
                event,
                "--revision",
                "0",
                "--output",
                unbound_path,
            ],
        ]
        .concat(),
    ));
    let decision_args = [
        "decide",
        &id,
        "--kind",
        "accepted",
        "--text",
        "Operator decision",
        "--source-file",
        unbound_path,
        "--config",
        config_arg,
    ];
    assert_failure(run(&db, &decision_args));
    let decision_args = [&decision_args[..], &["--expected-revision", "3"]].concat();
    let decision = self::json(&db, &decision_args);
    assert_eq!(decision["decision"]["source"]["event_id"], event);
    assert_eq!(run(&db, &decision_args).status.code(), Some(3));
    let new_task = self::json(
        &db,
        &[
            "approve-draft",
            unbound_path,
            "--config",
            config_arg,
            "--title",
            "Next task",
        ],
    );
    assert_eq!(new_task["task"]["title"], "Next task");
    assert_failure(approve(unbound_path, &db));
    assert_eq!(
        self::json(&db, &["list"])["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    check(run(
        &db,
        &[
            &review_args[..],
            &["--output", pending_review_draft.to_str().unwrap()],
        ]
        .concat(),
    ));
    check(invoke(&[
        "content",
        "revoke",
        "--db",
        db_arg,
        "--source-id",
        "harness.pi",
    ]));
    assert_failure(run(&db, &reveal_args));
    assert_failure(run(&db, &review_args));
    assert_failure(approve(pending_review_draft.to_str().unwrap(), &db));
    check(acquire());
    assert_failure(run(&db, &reveal_args));
    assert_failure(approve(pending_review_draft.to_str().unwrap(), &db));
    let metadata = self::json(&db, &["continuity", &id]);
    assert_eq!(
        metadata["approvals"][0]["continuation"],
        "Operator-authored next step"
    );
    let handoff = self::json(&db, &["handoff", &id, "--format", "json"]);
    assert!(!handoff.to_string().contains(content));
    assert_eq!(handoff["continuity"][0], metadata["approvals"][0]);
    assert!(
        !std::fs::read(root.0.join("history.sqlite.workspace.sqlite"))
            .unwrap()
            .windows(content.len())
            .any(|bytes| bytes == content.as_bytes())
    );

    let backup = root.0.join("backup");
    let restored = root.0.join("restored");
    Store::backup_workspace_bundle(&db, &backup).unwrap();
    Store::restore_workspace_bundle(&backup, &restored).unwrap();
    let restored_db = restored.join("history.sqlite");
    assert_failure(approve(unbound_path, &restored_db));
    assert_failure(run(&restored_db, &reveal_args));
    assert_eq!(self::json(&restored_db, &["continuity", &id]), metadata);
}

#[test]
fn task_planning_refuses_stale_edits_and_preserves_history_across_restore() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let mut writer = Store::open(&db).unwrap();
    seed(&mut writer, "harness.codex");
    drop(writer);
    let created = json(
        &db,
        &["create", "--title", "First", "--notes", "Keep this note"],
    );
    let id = created["id"].as_u64().unwrap().to_string();
    assert_eq!(created["revision"], 1);
    let changed = json(
        &db,
        &[
            "update",
            &id,
            "--expected-revision",
            "1",
            "--title",
            "Renamed",
        ],
    );
    assert_eq!(changed["revision"], 2);
    assert_eq!(changed["notes"], "Keep this note");
    let stale = run(
        &db,
        &[
            "update",
            &id,
            "--expected-revision",
            "1",
            "--notes",
            "Stale text",
        ],
    );
    assert_eq!(stale.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("expected 1, current 2"));
    assert_failure(stale);

    let evidence = [
        "--source-id",
        "harness.codex",
        "--session-id",
        "session",
        "--turn-id",
        "turn",
        "--worker-id",
        "worker",
    ];
    let linked = json(
        &db,
        &[
            &["link", &id, "--expected-revision", "2"],
            evidence.as_slice(),
        ]
        .concat(),
    );
    assert_eq!(linked["revision"], 3);
    let duplicate = json(
        &db,
        &[
            &["link", &id, "--expected-revision", "3"],
            evidence.as_slice(),
        ]
        .concat(),
    );
    assert_eq!(duplicate["revision"], 3);
    assert_eq!(duplicate["changed"], false);

    let brief_input = scratch.0.join("brief.json");
    std::fs::write(
        &brief_input,
        r#"{"intent":"Keep the public API","next_action":"Write a regression"}"#,
    )
    .unwrap();
    let brief = json(
        &db,
        &[
            "brief",
            &id,
            "--file",
            brief_input.to_str().unwrap(),
            "--expected-revision",
            "3",
        ],
    );
    assert_eq!(brief["revision"], 4);
    assert_eq!(brief["brief"]["next_action"], "Write a regression");
    std::fs::write(&brief_input, r#"{"intent":null}"#).unwrap();
    assert_failure(run(
        &db,
        &["brief", &id, "--file", brief_input.to_str().unwrap()],
    ));
    let updated = json(
        &db,
        &[
            "brief",
            &id,
            "--next-action",
            "",
            "--expected-revision",
            "4",
        ],
    );
    assert_eq!(updated["revision"], 5);
    assert_eq!(updated["brief"]["intent"], "Keep the public API");
    assert_eq!(updated["brief"]["next_action"], "");

    let decision = json(
        &db,
        &[
            "decide",
            &id,
            "--kind",
            "accepted",
            "--text",
            "Keep API",
            "--scope",
            "Public callers",
            "--expected-revision",
            "5",
        ],
    );
    let original = decision["decision"]["id"].as_u64().unwrap().to_string();
    assert_eq!(decision["revision"], 6);
    let decision_file = scratch.0.join("decision.txt");
    std::fs::write(&decision_file, "Use a staged deprecation").unwrap();
    let revision = json(
        &db,
        &[
            "decide",
            &id,
            "--kind",
            "rejected",
            "--text-file",
            decision_file.to_str().unwrap(),
            "--supersedes",
            &original,
            "--expected-revision",
            "6",
        ],
    );
    assert_eq!(revision["revision"], 7);
    assert_failure(run(
        &db,
        &[
            "decide",
            &id,
            "--kind",
            "accepted",
            "--text",
            "Replace it twice",
            "--supersedes",
            &original,
        ],
    ));
    let history = json(&db, &["decisions", &id]);
    assert_eq!(history["decisions"][0]["current"], false);
    assert_eq!(history["decisions"][1]["current"], true);
    assert_eq!(history["decisions"][1]["text"], "Use a staged deprecation");

    let markdown = run(&db, &["handoff", &id]);
    assert!(markdown.status.success());
    let markdown = String::from_utf8(markdown.stdout).unwrap();
    assert!(markdown.contains("Revision 7"));
    assert!(markdown.contains("Use a staged deprecation"));
    assert!(!markdown.contains("Keep API"));
    let handoff_json = json(&db, &["handoff", &id, "--format", "json"]);
    assert_eq!(handoff_json["links"][0]["worker_id"], "worker");
    assert_eq!(handoff_json["decisions"].as_array().unwrap().len(), 1);
    let export = scratch.0.join("task-handoff");
    let report = json(
        &db,
        &[
            "handoff",
            &id,
            "--output",
            export.to_str().unwrap(),
            "--include-decision-history",
        ],
    );
    for member in ["markdown", "html"] {
        let text = std::fs::read_to_string(report[member].as_str().unwrap()).unwrap();
        assert!(text.contains("Use a staged deprecation"));
        assert!(text.contains("Keep API"));
        assert!(text.contains("superseded"));
    }
    assert_failure(run(
        &db,
        &["handoff", &id, "--output", export.to_str().unwrap()],
    ));

    assert_eq!(
        json(&db, &["archive", &id, "--expected-revision", "7"])["archived"],
        true
    );
    assert_eq!(json(&db, &["list"])["items"].as_array().unwrap().len(), 0);
    assert_eq!(json(&db, &["list", "--all"])["items"][0]["archived"], true);
    assert_eq!(json(&db, &["show", &id])["task"]["revision"], 8);
    #[cfg(target_os = "linux")]
    {
        let bundle = scratch.0.join("backup");
        let restored = scratch.0.join("restored");
        Store::backup_workspace_bundle(&db, &bundle).unwrap();
        Store::restore_workspace_bundle(&bundle, &restored).unwrap();
        let db = restored.join("history.sqlite");
        assert_eq!(
            json(&db, &["brief", &id])["brief"]["intent"],
            "Keep the public API"
        );
        assert_eq!(
            json(&db, &["decisions", &id])["decisions"],
            history["decisions"]
        );
        let reopened = json(
            &db,
            &["archive", &id, "--restore", "--expected-revision", "8"],
        );
        assert_eq!(reopened["archived"], false);
        assert_eq!(json(&db, &["list"])["items"][0]["revision"], 9);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn task_workspace_capture_is_revision_checked_and_cleared_when_remapped() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    Store::open(&db).unwrap();
    let workspace = scratch.0.join("worktree");
    std::fs::create_dir(&workspace).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&workspace)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(workspace.join("new.txt"), "untracked fixture").unwrap();
    let saved = json(
        &db,
        &[
            "add",
            "--title",
            "Track a checkout",
            "--workspace",
            workspace.to_str().unwrap(),
        ],
    );
    let id = saved["id"].as_u64().unwrap().to_string();
    let captured = json(&db, &["snapshot", &id, "--expected-revision", "1"]);
    assert_eq!(captured["task"]["revision"], 2);
    assert_eq!(captured["git_snapshot"]["untracked_files"], 1);
    assert_eq!(
        run(&db, &["snapshot", &id, "--expected-revision", "1"])
            .status
            .code(),
        Some(3)
    );
    let moved = json(
        &db,
        &["update", &id, "--workspace", "", "--expected-revision", "2"],
    );
    assert!(moved["workspace_directory"].is_null());
    assert!(json(&db, &["show", &id])["git_snapshot"].is_null());
}

#[cfg(target_os = "linux")]
#[test]
fn git_patches_are_revision_checked_private_and_explicitly_exported() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    Store::open(&db).unwrap();
    let workspace = scratch.0.join("checkout");
    std::fs::create_dir(&workspace).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&workspace)
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
    };
    git(&["init", "--quiet"]);
    std::fs::write(workspace.join("tracked.txt"), "staged-private <tag>\n").unwrap();
    git(&["add", "tracked.txt"]);
    std::fs::write(workspace.join("tracked.txt"), "unstaged-private <tag>\n").unwrap();
    std::fs::write(workspace.join("untracked.txt"), "untracked-private-body\n").unwrap();
    let task = json(
        &db,
        &[
            "add",
            "--title",
            "Patch review",
            "--workspace",
            workspace.to_str().unwrap(),
        ],
    );
    let id = task["id"].as_u64().unwrap().to_string();
    let capture = json(
        &db,
        &[
            "snapshot",
            &id,
            "--expected-revision",
            "1",
            "--include-diff",
        ],
    );
    assert_eq!(capture["task"]["revision"], 2);
    assert!(capture["git_diff_bytes"].as_u64().unwrap() > 0);
    assert!(!capture.to_string().contains("staged-private"));
    assert!(
        !json(&db, &["show", &id])
            .to_string()
            .contains("staged-private")
    );
    assert!(
        !json(&db, &["handoff", &id, "--format", "json"])
            .to_string()
            .contains("staged-private")
    );
    let reveal_args = ["handoff", &id, "--format", "json", "--include-diff"];
    let included = json(&db, &reveal_args);
    assert!(
        included["git_diff"]["staged"]
            .as_str()
            .unwrap()
            .contains("+staged-private <tag>")
    );
    assert!(
        included["git_diff"]["unstaged"]
            .as_str()
            .unwrap()
            .contains("+unstaged-private <tag>")
    );
    assert!(!included.to_string().contains("untracked-private-body"));

    let export = scratch.0.join("private-export");
    json(
        &db,
        &[
            "handoff",
            &id,
            "--include-diff",
            "--output",
            export.to_str().unwrap(),
        ],
    );
    assert_eq!(
        export.metadata().unwrap().permissions().mode() & 0o777,
        0o700
    );
    for (filename, expected) in [
        ("handoff.md", "+unstaged-private <tag>"),
        ("handoff.html", "+unstaged-private &lt;tag&gt;"),
    ] {
        let file = export.join(filename);
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        assert!(std::fs::read_to_string(file).unwrap().contains(expected));
    }

    std::fs::write(
        workspace.join("tracked.txt"),
        format!("{}\n", "changed".repeat(12_000)),
    )
    .unwrap();
    let stale = run(
        &db,
        &[
            "snapshot",
            &id,
            "--expected-revision",
            "1",
            "--include-diff",
        ],
    );
    assert_eq!(stale.status.code(), Some(3));
    assert_failure(run(
        &db,
        &[
            "snapshot",
            &id,
            "--expected-revision",
            "2",
            "--include-diff",
        ],
    ));
    assert_eq!(json(&db, &reveal_args), included);

    std::fs::write(workspace.join("tracked.txt"), "small fixture\n").unwrap();
    let binary = workspace.join("large.bin");
    std::fs::File::create(&binary)
        .unwrap()
        .set_len(9 * 1024 * 1024)
        .unwrap();
    git(&["add", "large.bin"]);
    let input_limit = run(&db, &["snapshot", &id, "--include-diff"]);
    assert!(!input_limit.status.success());
    assert!(String::from_utf8_lossy(&input_limit.stderr).contains("patch inputs exceed 8 MiB"));
    assert_eq!(json(&db, &reveal_args), included);
    git(&["rm", "--cached", "large.bin"]);
    std::fs::remove_file(binary).unwrap();

    let metadata_only = json(&db, &["snapshot", &id, "--expected-revision", "2"]);
    assert_eq!(metadata_only["task"]["revision"], 3);
    assert_failure(run(&db, &reveal_args));
    std::fs::write(workspace.join("tracked.txt"), "restored small fixture\n").unwrap();
    json(
        &db,
        &[
            "snapshot",
            &id,
            "--expected-revision",
            "3",
            "--include-diff",
        ],
    );
    json(
        &db,
        &["update", &id, "--expected-revision", "4", "--workspace", ""],
    );
    assert_failure(run(&db, &reveal_args));
    assert!(json(&db, &["show", &id])["git_snapshot"].is_null());
}

#[test]
fn tasks_refuse_missing_or_unrelated_history_without_creating_workspace() {
    let scratch = Scratch::new();
    let missing = scratch.0.join("missing.sqlite");
    assert_failure(run(&missing, &["list"]));
    assert_failure(run(&missing, &["add", "--title", "New task"]));
    let foreign = scratch.0.join("foreign.sqlite");
    std::fs::write(&foreign, "not an Agenturm database").unwrap();
    assert_failure(run(&foreign, &["add", "--title", "New task"]));
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);

    let help = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args(["task", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("agenturm task add")
    );
}

fn seed(writer: &mut Store, source: &str) -> WorkspaceAnchor {
    let harness_id = ExtensionId::parse(source).unwrap();
    let mut identity = ObservationIdentity::session("session");
    identity.worker_id = Some("worker".into());
    identity.turn_id = Some("turn".into());
    ObservationStore::commit(
        writer,
        source,
        None,
        "first",
        &[Observation {
            harness_id: harness_id.clone(),
            event_id: "turn-start".into(),
            revision: 0,
            identity,
            observed_at_unix_ns: 1,
            kind: ObservationKind::Lifecycle {
                state: "running".into(),
                reason_code: None,
            },
        }],
    )
    .unwrap();
    WorkspaceAnchor {
        harness_id,
        session_id: "session".into(),
        worker_id: Some("worker".into()),
        turn_id: Some("turn".into()),
    }
}

fn run(database: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .arg("task")
        .args(args)
        .arg("--db")
        .arg(database)
        .output()
        .unwrap()
}

fn json(database: &Path, args: &[&str]) -> Value {
    let output = run(database, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_failure(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agenturm-task-cli-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
