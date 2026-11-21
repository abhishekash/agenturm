use agenturm_app::AgenturmApp;
use agenturm_kernel::{
    Capability, ExtensionId, ExtensionPlugin, Harness, Kernel, MetricRow, ObservationIdentity,
    ObservationKind, ObservationScope, RawObservation, RegistrationError,
};
use agenturm_lifecycle_extension_example::LifecycleExtensionPlugin;
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

struct FailsAfterRegisteringCapability;

impl ExtensionPlugin for FailsAfterRegisteringCapability {
    fn id(&self) -> ExtensionId {
        id("example.atomic-failure")
    }

    fn register(&self, kernel: &mut Kernel) -> Result<(), String> {
        kernel
            .register_capability(id("capability.must-not-leak"), BrokenCapability)
            .map_err(|error| error.to_string())?;
        Err("injected late registration failure".into())
    }
}

struct ConflictingPackage;

impl ExtensionPlugin for ConflictingPackage {
    fn id(&self) -> ExtensionId {
        id("example.id-conflict")
    }

    fn register(&self, kernel: &mut Kernel) -> Result<(), String> {
        kernel
            .register_capability(id("capability.conflict-must-not-leak"), BrokenCapability)
            .map_err(|error| error.to_string())?;
        kernel
            .register_ui_contribution(id("pane.conflict"), LifecyclePaneFixture)
            .map_err(|error| error.to_string())
    }
}

struct NestedCapabilityPackage(&'static str);

impl ExtensionPlugin for NestedCapabilityPackage {
    fn id(&self) -> ExtensionId {
        id("example.nested-package")
    }

    fn register(&self, kernel: &mut Kernel) -> Result<(), String> {
        kernel
            .register_capability(id(self.0), BrokenCapability)
            .map_err(|error| error.to_string())
    }
}

struct ParentWithNestedPackage;

impl ExtensionPlugin for ParentWithNestedPackage {
    fn id(&self) -> ExtensionId {
        id("example.parent-package")
    }

    fn register(&self, kernel: &mut Kernel) -> Result<(), String> {
        kernel
            .register_plugin(NestedCapabilityPackage(
                "capability.nested-package-second-contribution",
            ))
            .map_err(|error| error.to_string())
    }
}

struct PanicsWhileReportingId;

impl ExtensionPlugin for PanicsWhileReportingId {
    fn id(&self) -> ExtensionId {
        panic!("injected plugin ID panic")
    }

    fn register(&self, _kernel: &mut Kernel) -> Result<(), String> {
        unreachable!("ID failure must prevent registration")
    }
}

struct PanicsDuringRegistration;

impl ExtensionPlugin for PanicsDuringRegistration {
    fn id(&self) -> ExtensionId {
        id("example.registration-panic")
    }

    fn register(&self, kernel: &mut Kernel) -> Result<(), String> {
        kernel
            .register_capability(id("capability.panic-must-not-leak"), BrokenCapability)
            .unwrap();
        panic!("injected registration panic")
    }
}

struct LifecyclePaneFixture;

impl agenturm_kernel::UiContribution for LifecyclePaneFixture {
    fn build_pane(
        &self,
        snapshot: &agenturm_kernel::Snapshot,
    ) -> Result<agenturm_kernel::PaneModel, String> {
        Ok(agenturm_kernel::PaneModel {
            title: "occupied".into(),
            rows: vec![MetricRow {
                label: snapshot.observations.len().to_string(),
                value: "occupied".into(),
            }],
        })
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
    app.install_plugin(LifecycleExtensionPlugin).unwrap();
    assert!(matches!(
        app.install_plugin(LifecycleExtensionPlugin),
        Err(RegistrationError::DuplicateId(plugin_id)) if plugin_id == "example.lifecycle"
    ));
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
    app.register_ui_contribution(id("pane.example.lifecycle"), LifecyclePaneFixture)
        .unwrap();
    assert!(matches!(
        app.register_ui_contribution(id("pane.example.lifecycle"), LifecyclePaneFixture),
        Err(RegistrationError::DuplicateId(_))
    ));
}

#[test]
fn failed_package_install_does_not_leave_partial_contributions() {
    let mut app = AgenturmApp::empty_composition();
    assert!(matches!(
        app.install_plugin(FailsAfterRegisteringCapability),
        Err(RegistrationError::ExtensionInstallFailed { .. })
    ));
    let snapshot = app.refresh();
    assert!(
        !snapshot
            .metrics
            .contains_key(&id("capability.must-not-leak"))
    );
    assert!(snapshot.panes.is_empty());
}

#[test]
fn contribution_id_conflict_rejects_the_entire_package_atomically() {
    let mut app = AgenturmApp::empty_composition();
    app.register_ui_contribution(id("pane.conflict"), LifecyclePaneFixture)
        .unwrap();
    assert!(matches!(
        app.install_plugin(ConflictingPackage),
        Err(RegistrationError::DuplicateId(id)) if id == "pane.conflict"
    ));
    let snapshot = app.refresh();
    assert!(
        !snapshot
            .metrics
            .contains_key(&id("capability.conflict-must-not-leak"))
    );
    assert_eq!(snapshot.panes.len(), 1);
    assert!(snapshot.panes.contains_key(&id("pane.conflict")));
}

#[test]
fn nested_package_id_conflicts_are_rejected_without_partial_registration() {
    let mut app = AgenturmApp::empty_composition();
    app.install_plugin(NestedCapabilityPackage(
        "capability.nested-package-first-contribution",
    ))
    .unwrap();
    assert!(matches!(
        app.install_plugin(ParentWithNestedPackage),
        Err(RegistrationError::DuplicateId(id)) if id == "example.nested-package"
    ));
    let snapshot = app.refresh();
    assert!(
        !snapshot
            .metrics
            .contains_key(&id("capability.nested-package-second-contribution"))
    );
}

#[test]
fn plugin_identity_panics_are_reported_without_unwinding_the_app() {
    let mut app = AgenturmApp::empty_composition();
    assert!(matches!(
        app.install_plugin(PanicsWhileReportingId),
        Err(RegistrationError::ExtensionIdentityPanicked)
    ));
}

#[test]
fn plugin_registration_panics_are_reported_without_partial_contributions() {
    let mut app = AgenturmApp::empty_composition();
    assert!(matches!(
        app.install_plugin(PanicsDuringRegistration),
        Err(RegistrationError::ExtensionInstallPanicked { plugin_id })
            if plugin_id == "example.registration-panic"
    ));
    let snapshot = app.refresh();
    assert!(
        !snapshot
            .metrics
            .contains_key(&id("capability.panic-must-not-leak"))
    );
}
