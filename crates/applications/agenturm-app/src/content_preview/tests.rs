use super::*;
use agenturm_content::{PromptListing, ResolvedSource};
use std::fs;

struct Fixture {
    root: PathBuf,
    resolved: ResolvedSource,
    automatic: bool,
}

impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "agenturm-default-prompts-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let source_root = root.join("session.jsonl");
        fs::write(&source_root, "{\"type\":\"session\",\"id\":\"fixture\"}\n").unwrap();
        Self {
            resolved: ResolvedSource {
                source_id: "harness.pi".into(),
                plugin_id: "example.rust-pi-harness".into(),
                plugin_host: root.join("plugin-host"),
                adapter: root.join("adapter"),
                artifact_sha256: Some("a".repeat(64)),
                features: Some(vec!["source-adapter/1".into(), "configured-root/1".into()]),
                source_root,
            },
            root,
            automatic: true,
        }
    }

    fn list(&self, store: Option<&Store>, session: &str, worker: Option<&str>) -> PromptListing {
        list_user_prompts_with_access(
            &self.root.join("history.sqlite"),
            self,
            &ExtensionId::parse("harness.pi").unwrap(),
            session,
            worker,
            store,
        )
        .unwrap()
    }

    fn scope(&self) -> agenturm_store::ContentGrantScope {
        agenturm_content::bind(
            &linux::resolver(self),
            &ExtensionId::parse("harness.pi").unwrap(),
            None,
        )
        .unwrap()
        .scope()
        .clone()
    }
}

impl ContentSourceConfig for Fixture {
    fn default_prompt_history(&self) -> bool {
        self.automatic
    }

    fn resolve_content_source(&self, id: &ExtensionId) -> Result<ResolvedSource, String> {
        if id.as_str() != self.resolved.source_id {
            return Err("Source is not configured".into());
        }
        Ok(self.resolved.clone())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn live_prompt_listing_defaults_on_but_saved_history_and_revocations_stay_explicit() {
    let mut fixture = Fixture::new();
    let db = fixture.root.join("history.sqlite");
    let mut store = Store::open_collector(&db).unwrap();
    let scope = fixture.scope();
    fixture.automatic = false;
    assert!(matches!(
        fixture.list(Some(&store), "fixture", None),
        PromptListing::ApprovalRequired(_)
    ));
    assert!(!db.with_extension("sqlite.workspace.sqlite").exists());
    drop(store);
    fixture.automatic = true;
    // A file-backed view without the active writer does not acquire a default.
    assert!(matches!(
        fixture.list(None, "fixture", None),
        PromptListing::ApprovalRequired(_)
    ));
    store = Store::open_collector(&db).unwrap();
    let result = crate::cockpit_workflows::execute_prompt_with_config(
        &db,
        Some(&store),
        agenturm_ui::WorkCommand::Prompts(agenturm_ui::WorkAnchor {
            source: "harness.pi".into(),
            session: "fixture".into(),
            worker: None,
            turn: None,
        }),
        Some(&fixture),
    )
    .unwrap();
    let agenturm_ui::WorkResult::PromptHistory(page) = result else {
        panic!("prompt page")
    };
    assert_eq!(page.availability, agenturm_ui::PromptAvailability::Granted);
    assert_eq!(
        fixture.list(Some(&store), "fixture", None),
        PromptListing::Granted(vec![])
    );
    let grant = store.current_content_grant(&scope).unwrap().unwrap();
    assert_eq!(
        fixture.list(Some(&store), "fixture", None),
        PromptListing::Granted(vec![])
    );
    assert_eq!(store.current_content_grant(&scope).unwrap(), Some(grant));
    // A changed binding is not silently reapproved.
    fixture.resolved.artifact_sha256 = Some("b".repeat(64));
    assert!(matches!(
        fixture.list(Some(&store), "fixture", None),
        PromptListing::ApprovalRequired(_)
    ));
    fixture.resolved.artifact_sha256 = Some("a".repeat(64));
    assert!(store.revoke_source_content(scope.source_id()).unwrap());
    assert!(matches!(
        fixture.list(Some(&store), "fixture", None),
        PromptListing::ApprovalRequired(_)
    ));
    store.grant_content(&scope).unwrap();
    assert_eq!(
        fixture.list(Some(&store), "fixture", None),
        PromptListing::Granted(vec![])
    );
}

#[test]
fn default_prompt_access_still_proves_exact_committed_bytes_and_scope() {
    use sha2::{Digest, Sha256};
    let (Some(host), Some(adapter)) = (
        std::env::var_os("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var_os("AGENTURM_RUNTIME_PI_ADAPTER"),
    ) else {
        eprintln!("set plugin host and Pi adapter paths to run real default-prompt proof test");
        return;
    };
    let mut fixture = Fixture::new();
    fs::copy(adapter, &fixture.resolved.adapter).unwrap();
    fixture.resolved.plugin_host = host.into();
    fixture.resolved.artifact_sha256 = Some(format!(
        "{:x}",
        Sha256::digest(fs::read(&fixture.resolved.adapter).unwrap())
    ));
    fs::write(&fixture.resolved.source_root, concat!(
        "{\"type\":\"session\",\"id\":\"fixture\"}\n",
        "{\"type\":\"message\",\"id\":\"turn-safe\",\"timestamp\":\"2026-11-20T12:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"review synthetic changes\"}}\n",
        "{\"type\":\"message\",\"id\":\"turn-secret\",\"timestamp\":\"2026-11-20T12:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"API_KEY=fixture-private-value\"}}\n",
    )).unwrap();
    let catalog = fixture.root.join("catalog.json");
    fs::write(&catalog, serde_json::to_vec(&serde_json::json!({
        "version": 1, "plugins": [{
            "plugin_id": fixture.resolved.plugin_id, "adapter": fixture.resolved.adapter,
            "sha256": fixture.resolved.artifact_sha256, "protocol": 1,
            "features": fixture.resolved.features, "observation_format": "agenturm-observations/1",
        }],
    })).unwrap()).unwrap();
    let config = fixture.root.join("config.json");
    fs::write(&config, serde_json::to_vec(&serde_json::json!({
        "version": 2, "plugin_host": fixture.resolved.plugin_host, "plugin_catalog": catalog,
        "sources": [{"plugin_id": fixture.resolved.plugin_id, "source_root": fixture.resolved.source_root, "source_id": fixture.resolved.source_id}],
    })).unwrap()).unwrap();
    let db = fixture.root.join("history.sqlite");
    let report = crate::execute_collect(&crate::ConfiguredCollectOptions {
        config,
        db: db.clone(),
        watch_interval: None,
    })
    .unwrap();
    assert!(!report.has_failures(), "{report:?}");
    let mut store = Store::open_collector(&db).unwrap();
    let PromptListing::Granted(references) = fixture.list(Some(&store), "fixture", None) else {
        panic!("default grant")
    };
    assert_eq!(references.len(), 1, "credential-like text remains filtered");
    assert_eq!(references[0].turn_id, "turn-safe");
    assert_eq!(
        fixture.list(Some(&store), "other-session", None),
        PromptListing::Granted(vec![])
    );
    assert_eq!(
        fixture.list(Some(&store), "fixture", Some("other-worker")),
        PromptListing::Granted(vec![])
    );
    let scope = fixture.scope();
    let reference = &references[0];
    let mut output = Vec::new();
    let reveal = |output: &mut Vec<u8>, store: &Store| {
        reveal_user_prompt(
            &db,
            &fixture,
            scope.source_id(),
            PromptSelection {
                session_id: "fixture",
                worker_id: None,
                event_id: &reference.event_id,
                revision: reference.revision,
                snapshot_id: reference.snapshot_id,
                history_identity: reference.history_identity.as_deref(),
            },
            output,
            Some(store),
        )
    };
    reveal(&mut output, &store).unwrap();
    assert!(
        String::from_utf8(output)
            .unwrap()
            .contains("review synthetic changes")
    );
    store
        .revoke_content(scope.source_id(), scope.kind())
        .unwrap();
    assert!(matches!(
        fixture.list(Some(&store), "fixture", None),
        PromptListing::ApprovalRequired(_)
    ));
    let mut denied_output = Vec::new();
    assert!(reveal(&mut denied_output, &store).is_err());
    assert!(denied_output.is_empty());
    store.grant_content(&scope).unwrap();
    let bytes = fs::read_to_string(&fixture.resolved.source_root).unwrap();
    fs::write(
        &fixture.resolved.source_root,
        bytes.replace("review synthetic changes", "review synthetic updates"),
    )
    .unwrap();
    let mut changed_output = Vec::new();
    assert!(reveal(&mut changed_output, &store).is_err());
    assert!(
        changed_output.is_empty(),
        "a default grant never bypasses the committed-byte proof"
    );
}
