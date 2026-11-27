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

#[test]
fn standalone_example_runs_through_the_agenturm_host_protocol() {
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_agenturm-prompt-signals-extension"));
    let sha256 = Sha256::digest(std::fs::read(&executable).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let extension = ProcessObservationExtension::new(ProcessObservationExtensionConfig {
        plugin_id: ExtensionId::parse("example.prompt-signals").unwrap(),
        executable,
        sha256,
        features: vec!["capability/1".into(), "ui-contribution/1".into()],
        grants: BTreeSet::from([ExtensionDataGrant::Usage, ExtensionDataGrant::MessageRole]),
    })
    .unwrap();
    let observations = [
        Observation {
            harness_id: ExtensionId::parse("harness.pi").unwrap(),
            event_id: "private-event".into(),
            revision: 0,
            identity: ObservationIdentity {
                scope: ObservationScope::Session,
                session_id: Some("private-session".into()),
                worker_id: None,
                turn_id: None,
                operation_id: None,
            },
            observed_at_unix_ns: 10,
            kind: ObservationKind::Message {
                role: "user".into(),
            },
        },
        Observation {
            harness_id: ExtensionId::parse("harness.pi").unwrap(),
            event_id: "private-event-2".into(),
            revision: 0,
            identity: ObservationIdentity {
                scope: ObservationScope::Session,
                session_id: Some("private-session".into()),
                worker_id: None,
                turn_id: None,
                operation_id: None,
            },
            observed_at_unix_ns: 11,
            kind: ObservationKind::Usage {
                input_tokens: Some(31),
                output_tokens: Some(8),
                cache_read_tokens: None,
                cache_write_tokens: None,
                total_tokens: Some(39),
            },
        },
    ];
    let output = extension
        .evaluate(&ObservationExtensionInput {
            observations: &observations,
            snapshot_id: 3,
            observations_truncated: false,
            selection_scope: ObservationSelectionScope::Latest,
        })
        .unwrap();
    assert_eq!(output.metrics[0].value, "1");
    assert_eq!(output.metrics[2].value, "31");
    assert_eq!(output.pane.unwrap().title, "Prompt signals");
}
