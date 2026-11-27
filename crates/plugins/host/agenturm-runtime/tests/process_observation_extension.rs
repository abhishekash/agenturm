use agenturm_kernel::{
    ExtensionId, Observation, ObservationExtension, ObservationExtensionInput, ObservationIdentity,
    ObservationKind, ObservationScope, ObservationSelectionScope,
};
use agenturm_runtime::{
    ExtensionDataGrant, ProcessObservationExtension, ProcessObservationExtensionConfig,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn executable_hash() -> (PathBuf, String) {
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_agenturm-extension-fixture"));
    let digest = Sha256::digest(std::fs::read(&executable).unwrap());
    let pin = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    (executable, pin)
}

fn config(plugin_id: &str) -> ProcessObservationExtensionConfig {
    let (executable, sha256) = executable_hash();
    let grant = if plugin_id == "example.no-read" {
        ExtensionDataGrant::Lifecycle
    } else {
        ExtensionDataGrant::Usage
    };
    ProcessObservationExtensionConfig {
        plugin_id: ExtensionId::parse(plugin_id).unwrap(),
        executable,
        sha256,
        features: vec!["capability/1".into(), "ui-contribution/1".into()],
        grants: BTreeSet::from([grant]),
    }
}

fn input() -> Observation {
    Observation {
        harness_id: ExtensionId::parse("harness.pi").unwrap(),
        event_id: "must-not-cross-process".into(),
        revision: 3,
        identity: ObservationIdentity {
            scope: ObservationScope::Session,
            session_id: Some("must-not-cross-process".into()),
            worker_id: None,
            turn_id: None,
            operation_id: None,
        },
        observed_at_unix_ns: 123,
        kind: ObservationKind::Usage {
            input_tokens: Some(9),
            output_tokens: Some(2),
            cache_read_tokens: None,
            cache_write_tokens: None,
            total_tokens: Some(11),
        },
    }
}

#[test]
fn process_extension_handshake_projection_and_atomic_outputs_work() {
    let plugin = ProcessObservationExtension::new(config("example.extension")).unwrap();
    let observations = [input()];
    let output = plugin
        .evaluate(&ObservationExtensionInput {
            observations: &observations,
            snapshot_id: 41,
            observations_truncated: false,
            selection_scope: ObservationSelectionScope::Filtered,
        })
        .unwrap();
    assert_eq!(output.metrics[0].label, "fixture metric");
    assert_eq!(output.metrics[0].value, "1");
    assert_eq!(output.pane.unwrap().title, "Fixture pane");
}

#[test]
fn process_extension_rejects_extra_frame_and_cleans_child() {
    let plugin = ProcessObservationExtension::new(config("example.extra")).unwrap();
    let observations = [input()];
    let error = plugin
        .evaluate(&ObservationExtensionInput {
            observations: &observations,
            snapshot_id: 1,
            observations_truncated: false,
            selection_scope: ObservationSelectionScope::Latest,
        })
        .unwrap_err();
    assert!(error.contains("extra frame"));
}

#[test]
fn process_extension_deadline_kills_hanging_child() {
    let plugin = ProcessObservationExtension::new(config("example.hanging")).unwrap();
    let observations = [input()];
    let error = plugin
        .evaluate(&ObservationExtensionInput {
            observations: &observations,
            snapshot_id: 1,
            observations_truncated: false,
            selection_scope: ObservationSelectionScope::Latest,
        })
        .unwrap_err();
    assert!(error.contains("deadline"));
}

#[test]
fn escaped_descendant_retaining_stdout_cannot_block_evaluation_cleanup() {
    let plugin = ProcessObservationExtension::new(config("example.descendant")).unwrap();
    let observations = [input()];
    let started = std::time::Instant::now();
    let error = plugin
        .evaluate(&ObservationExtensionInput {
            observations: &observations,
            snapshot_id: 1,
            observations_truncated: false,
            selection_scope: ObservationSelectionScope::Latest,
        })
        .unwrap_err();
    assert!(error.contains("deadline"));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn process_extension_rejects_bad_handshake_oversize_and_nonzero_exit() {
    for (plugin_id, expected) in [
        ("example.bad-handshake", "handshake mismatch"),
        ("example.oversized", "frame length is invalid"),
        ("example.nonzero", "exited unsuccessfully"),
    ] {
        let plugin = ProcessObservationExtension::new(config(plugin_id)).unwrap();
        let observations = [input()];
        let error = plugin
            .evaluate(&ObservationExtensionInput {
                observations: &observations,
                snapshot_id: 1,
                observations_truncated: false,
                selection_scope: ObservationSelectionScope::Latest,
            })
            .unwrap_err();
        assert!(error.contains(expected), "{plugin_id}: {error}");
    }
}

#[test]
fn process_extension_request_write_obeys_deadline_when_child_stops_reading() {
    let plugin = ProcessObservationExtension::new(config("example.no-read")).unwrap();
    let observations = (0..500)
        .map(|_| {
            let mut observation = input();
            observation.kind = ObservationKind::Lifecycle {
                state: "s".repeat(64),
                reason_code: Some("r".repeat(64)),
            };
            observation
        })
        .collect::<Vec<_>>();
    let error = plugin
        .evaluate(&ObservationExtensionInput {
            observations: &observations,
            snapshot_id: 1,
            observations_truncated: false,
            selection_scope: ObservationSelectionScope::Latest,
        })
        .unwrap_err();
    assert!(error.contains("deadline"));
}
