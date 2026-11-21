use agenturm_kernel::{ExtensionId, Kernel, ObservationKind, RelationshipKind};
use agenturm_runtime::{ProcessConfig, ProcessSource};
use agenturm_store::Store;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn kiro_process_preserves_explicit_failed_turn_after_reopen() {
    let (Some(plugin_host), Some(adapter)) = (
        std::env::var_os("AGENTURM_RUNTIME_PLUGIN_HOST"),
        std::env::var_os("AGENTURM_RUNTIME_KIRO_ADAPTER"),
    ) else {
        eprintln!("skipping real-process test: set plugin host and Kiro adapter paths");
        return;
    };
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("agenturm-kiro-{}-{unique}", std::process::id()));
    let session = root.join("sess_failed");
    let workers = session.join("sub-executions");
    fs::create_dir_all(&workers).unwrap();
    let source_path = session.join("messages.jsonl");
    fs::write(
        &source_path,
        concat!(
            r#"{"id":"turn-1","timestamp":"2026-11-20T12:00:00Z","payload":{"type":"user"}}"#,
            "\n",
            r#"{"id":"spawn-1","timestamp":"2026-11-20T12:00:00.5Z","payload":{"type":"sub_agent_start","subSessionId":"worker-1","parentExecutionId":"turn-1","prompt":"private task text"}}"#,
            "\n",
            r#"{"id":"spawn-invalid","timestamp":"2026-11-20T12:00:00.6Z","payload":{"type":"sub_agent_start","subSessionId":"../private-path"}}"#,
            "\n",
            r#"{"id":"end-1","timestamp":"2026-11-20T12:00:01Z","payload":{"type":"turn_end","stopReason":"error"}}"#,
            "\n",
            r#"{"id":"usage-1","timestamp":"2026-11-20T12:00:01.1Z","payload":{"type":"usage_summary","turnId":"turn-1","promptTurnSummaries":[{"unit":"credit","usage":0.123456789},{"unit":"credit","usage":0.2}]}}"#,
            "\n",
            r#"{"id":"usage-zero","timestamp":"2026-11-20T12:00:01.2Z","payload":{"type":"usage_summary","promptTurnSummaries":[{"unit":"credit","usage":0}]}}"#,
            "\n",
            r#"{"id":"usage-unknown","timestamp":"2026-11-20T12:00:01.3Z","payload":{"type":"usage_summary","promptTurnSummaries":[{"unit":"unknown","usage":0}]}}"#,
            "\n"
        ),
    )
    .unwrap();
    let worker_path = workers.join("worker-1.jsonl");
    fs::write(
        &worker_path,
        concat!(
            r#"{"id":"spawn-nested","timestamp":"2026-11-20T12:00:02Z","payload":{"type":"sub_agent_start","subSessionId":"worker-2","parentExecutionId":"worker-turn"}}"#,
            "\n"
        ),
    )
    .unwrap();

    let database = root.join("agenturm.sqlite");
    let source_id = ExtensionId::parse("harness.kiro").unwrap();
    let worker_source_id = ExtensionId::parse("harness.kiro.worker").unwrap();
    {
        let mut store = Store::open(&database).unwrap();
        let mut kernel = Kernel::new();
        let source = ProcessSource::spawn(ProcessConfig::new(
            plugin_host.clone(),
            adapter.clone(),
            source_path,
            source_id.as_str(),
            "example.rust-kiro-harness",
        ))
        .unwrap();
        kernel
            .register_source_adapter(source_id.clone(), source)
            .unwrap();
        let worker = ProcessSource::spawn(ProcessConfig::new(
            plugin_host,
            adapter,
            worker_path,
            worker_source_id.as_str(),
            "example.rust-kiro-harness",
        ))
        .unwrap();
        kernel
            .register_source_adapter(worker_source_id.clone(), worker)
            .unwrap();
        let snapshot = kernel.refresh_with_store(&mut store);
        assert!(snapshot.failures.is_empty(), "{:?}", snapshot.failures);
        assert_eq!(snapshot.admissions[&source_id].inserted, 8);
        assert!(snapshot.admissions[&source_id].source_ack_sent);
        assert_eq!(snapshot.admissions[&worker_source_id].inserted, 1);
        assert!(snapshot.admissions[&worker_source_id].source_ack_sent);
    }

    let mut store = Store::open_read_only(&database).unwrap();
    let reopened = Kernel::new().refresh_with_store(&mut store);
    assert_eq!(reopened.observations.len(), 9);
    let failed = reopened
        .observations
        .iter()
        .find(|row| {
            row.kind
                == ObservationKind::Lifecycle {
                    state: "failed".into(),
                    reason_code: Some("kiro_error".into()),
                }
        })
        .expect("failed lifecycle evidence");
    assert_eq!(failed.harness_id, source_id);
    assert_eq!(failed.identity.session_id.as_deref(), Some("sess_failed"));
    assert_eq!(failed.identity.turn_id.as_deref(), Some("turn-1"));
    let credits = reopened
        .observations
        .iter()
        .filter_map(|row| match row.kind {
            ObservationKind::CreditUsage { nanocredits } => {
                Some((nanocredits, row.identity.turn_id.as_deref()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(credits, vec![(323_456_789, Some("turn-1")), (0, None)]);
    assert!(reopened.observations.iter().any(|row| {
        row.kind
            == ObservationKind::Diagnostic {
                code: "kiro_credit_usage_unrepresented".into(),
                severity: "warning".into(),
            }
    }));
    for (source, child, parent) in [
        (&source_id, "worker-1", "sess_failed"),
        (&worker_source_id, "worker-2", "worker-1"),
    ] {
        let relation = reopened
            .observations
            .iter()
            .find(|row| {
                row.harness_id == *source
                    && row.kind
                        == ObservationKind::Relationship {
                            relationship_kind: RelationshipKind::Delegated,
                            parent_session_id: parent.into(),
                        }
            })
            .expect("durable explicit worker relationship");
        assert_eq!(relation.identity.session_id.as_deref(), Some(child));
        assert_eq!(relation.identity.worker_id.as_deref(), Some(child));
        assert!(relation.identity.turn_id.is_none());
    }
    for private in ["private task text", "../private-path"] {
        assert!(!format!("{:?}", reopened.observations).contains(private));
    }
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
