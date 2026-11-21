use agenturm_app::AgenturmApp;
use agenturm_kernel::{
    Capability, ExtensionId, Harness, MetricRow, ObservationIdentity, ObservationKind,
    ObservationScope, RawObservation, RegistrationError,
};
use agenturm_lifecycle_extension_example::{LifecycleCounts, LifecyclePane};
use agenturm_ui::UiModel;

struct InterruptedSession;

impl Harness for InterruptedSession {
    fn poll(&mut self) -> Result<Vec<RawObservation>, String> {
        Ok(vec![RawObservation {
            event_id: "lifecycle:interrupted".into(),
            revision: 0,
            identity: ObservationIdentity {
                scope: ObservationScope::Session,
                session_id: Some("session-1".into()),
                worker_id: None,
                turn_id: None,
                operation_id: None,
            },
            observed_at_unix_ns: 1,
            kind: ObservationKind::Lifecycle {
                state: "interrupted".into(),
                reason_code: Some("adapter_stopped".into()),
            },
        }])
    }
}

struct BrokenCapability;

impl Capability for BrokenCapability {
    fn evaluate(
        &self,
        _observations: &[agenturm_kernel::Observation],
    ) -> Result<Vec<MetricRow>, String> {
        Err("fixture capability failure".into())
    }
}

fn id(value: &str) -> ExtensionId {
    ExtensionId::parse(value).unwrap()
}

#[test]
fn separately_packaged_capability_and_pane_compose_without_dispatch_changes() {
    let mut app = AgenturmApp::default_composition();
    app.register_harness(id("harness.fixture"), InterruptedSession)
        .unwrap();
    app.register_capability(id("capability.example.lifecycle"), LifecycleCounts)
        .unwrap();
    app.register_ui_contribution(id("pane.example.lifecycle"), LifecyclePane)
        .unwrap();
    app.register_capability(id("capability.fixture.broken"), BrokenCapability)
        .unwrap();

    let snapshot = app.refresh();
    assert_eq!(snapshot.observations.len(), 1);
    assert_eq!(
        snapshot.metrics[&id("capability.example.lifecycle")][1].value,
        "1"
    );
    let pane = &snapshot.panes[&id("pane.example.lifecycle")];
    assert_eq!(pane.title, "Lifecycle by source");
    assert_eq!(pane.rows[0].label, "harness.fixture");
    assert_eq!(pane.rows[0].value, "1 total · 1 interrupted");
    assert!(snapshot.panes.contains_key(&id("pane.overview")));
    assert!(snapshot.metrics.contains_key(&id("capability.overview")));
    assert_eq!(snapshot.failures.len(), 1);
    assert_eq!(
        snapshot.failures[0].extension_id,
        id("capability.fixture.broken")
    );

    let mut model = UiModel::from_snapshot(snapshot);
    assert_eq!(model.selected_title(), Some("Lifecycle by source"));
    model.select_next();
    assert_eq!(model.selected_title(), Some("Overview"));
    model.select_next();
    assert_eq!(model.selected_title(), Some("Metrics"));
    model.select_next();
    assert_eq!(model.selected_title(), Some("Attention"));
}

#[test]
fn fixture_registration_keeps_duplicate_ids_rejected() {
    let mut app = AgenturmApp::empty_composition();
    app.register_ui_contribution(id("pane.example.lifecycle"), LifecyclePane)
        .unwrap();
    assert!(matches!(
        app.register_ui_contribution(id("pane.example.lifecycle"), LifecyclePane),
        Err(RegistrationError::DuplicateId(_))
    ));
}
