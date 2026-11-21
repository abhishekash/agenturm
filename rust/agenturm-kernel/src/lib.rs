//! Small composition kernel for Agenturm's Rust application.
//!
//! Harnesses contribute observations, capabilities turn those observations
//! into typed metric rows, and UI contributions provide data-only panes. The
//! kernel owns extension identity, execution order, attribution, and bounded
//! snapshot validation. These trait objects are trusted in-process extensions,
//! not a sandbox or resource-isolation boundary. Calls are synchronous,
//! validation happens after extensions allocate their results, and the default
//! panic hook can still report caught panics. Independently installed or
//! untrusted extensions must use the supervised process boundary rather than
//! implementing these traits in the application process. The kernel has no
//! terminal, filesystem, process, policy, or storage authority.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

const MAX_EXTENSIONS_PER_KIND: usize = 64;
const MAX_OBSERVATIONS_PER_HARNESS: usize = 2_048;
const MAX_METRIC_ROWS: usize = 256;
const MAX_SESSION_KEY_BYTES: usize = 512;
const MAX_EVENT_ID_BYTES: usize = 256;
const MAX_CATEGORY_BYTES: usize = 64;
const MAX_LABEL_BYTES: usize = 128;
const MAX_VALUE_BYTES: usize = 1_024;
const MAX_TITLE_BYTES: usize = 128;
const MAX_FAILURE_BYTES: usize = 256;

/// Stable package identity for one registered extension.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExtensionId(String);

impl ExtensionId {
    pub fn parse(value: impl Into<String>) -> Result<Self, RegistrationError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 96
            && value.as_bytes()[0].is_ascii_lowercase()
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_/".contains(&byte)
            });
        if !valid {
            return Err(RegistrationError::InvalidId(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistrationError {
    InvalidId(String),
    DuplicateId(String),
    RegistryFull,
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(f, "invalid extension id: {id:?}"),
            Self::DuplicateId(id) => write!(f, "extension id already registered: {id}"),
            Self::RegistryFull => write!(f, "extension registry limit reached"),
        }
    }
}

impl std::error::Error for RegistrationError {}

/// Harness facts are intentionally source-neutral. The host supplies the
/// harness identity so an adapter cannot attribute its output to a sibling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    pub harness_id: ExtensionId,
    pub event_id: String,
    pub revision: u32,
    pub identity: ObservationIdentity,
    pub observed_at_unix_ns: u64,
    pub kind: ObservationKind,
}

/// Source-native identity components preserved across collection and storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationIdentity {
    pub session_id: String,
    pub worker_id: Option<String>,
    pub turn_id: Option<String>,
    pub operation_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservationKind {
    SessionStarted,
    SessionEnded,
    Activity {
        category: String,
    },
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_read_tokens: Option<u64>,
        cache_write_tokens: Option<u64>,
        total_tokens: Option<u64>,
    },
}

/// Adapter port. Implementations own harness-specific discovery and parsing;
/// the kernel stamps the registered identity on returned facts.
pub trait Harness: Send {
    fn poll(&mut self) -> Result<Vec<RawObservation>, String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawObservation {
    pub event_id: String,
    pub revision: u32,
    pub identity: ObservationIdentity,
    pub observed_at_unix_ns: u64,
    pub kind: ObservationKind,
}

/// Capability extension port. A capability cannot mutate observations and
/// returns only bounded, render-neutral rows for the host to expose.
pub trait Capability: Send + Sync {
    fn evaluate(&self, observations: &[Observation]) -> Result<Vec<MetricRow>, String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricRow {
    pub label: String,
    pub value: String,
}

/// UI contributions are data, not widgets. The application owns layout,
/// navigation, focus and terminal lifecycle and renders each pane generically.
pub trait UiContribution: Send + Sync {
    fn build_pane(&self, snapshot: &Snapshot) -> Result<PaneModel, String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneModel {
    pub title: String,
    pub rows: Vec<MetricRow>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionFailure {
    pub extension_id: ExtensionId,
    pub stage: ExtensionStage,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionStage {
    Poll,
    Capability,
    UiContribution,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub observations: Vec<Observation>,
    pub metrics: BTreeMap<ExtensionId, Vec<MetricRow>>,
    pub panes: BTreeMap<ExtensionId, PaneModel>,
    pub failures: Vec<ExtensionFailure>,
}

#[derive(Default)]
pub struct Kernel {
    harnesses: BTreeMap<ExtensionId, Box<dyn Harness>>,
    capabilities: BTreeMap<ExtensionId, Box<dyn Capability>>,
    ui_contributions: BTreeMap<ExtensionId, Box<dyn UiContribution>>,
}

impl Kernel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_harness(
        &mut self,
        id: ExtensionId,
        harness: impl Harness + 'static,
    ) -> Result<(), RegistrationError> {
        insert_unique(&mut self.harnesses, id, Box::new(harness))
    }

    pub fn register_capability(
        &mut self,
        id: ExtensionId,
        capability: impl Capability + 'static,
    ) -> Result<(), RegistrationError> {
        insert_unique(&mut self.capabilities, id, Box::new(capability))
    }

    pub fn register_ui_contribution(
        &mut self,
        id: ExtensionId,
        contribution: impl UiContribution + 'static,
    ) -> Result<(), RegistrationError> {
        insert_unique(&mut self.ui_contributions, id, Box::new(contribution))
    }

    /// Polls trusted in-process extensions in stable ID order. Returned errors
    /// and unwind panics are recorded so healthy extensions still contribute;
    /// calls remain synchronous and may block refresh.
    pub fn refresh(&mut self) -> Snapshot {
        let mut snapshot = Snapshot::default();
        for (id, harness) in &mut self.harnesses {
            let result = catch_unwind(AssertUnwindSafe(|| harness.poll()));
            match result {
                Ok(Ok(rows)) => match validate_raw_observations(rows) {
                    Ok(rows) => {
                        snapshot
                            .observations
                            .extend(rows.into_iter().map(|row| Observation {
                                harness_id: id.clone(),
                                event_id: row.event_id,
                                revision: row.revision,
                                identity: row.identity,
                                observed_at_unix_ns: row.observed_at_unix_ns,
                                kind: row.kind,
                            }))
                    }
                    Err(message) => {
                        snapshot
                            .failures
                            .push(failure(id, ExtensionStage::Poll, message))
                    }
                },
                Ok(Err(message)) => {
                    snapshot
                        .failures
                        .push(failure(id, ExtensionStage::Poll, message))
                }
                Err(_) => {
                    snapshot
                        .failures
                        .push(failure(id, ExtensionStage::Poll, "extension panicked"))
                }
            }
        }
        for (id, capability) in &self.capabilities {
            let result = catch_unwind(AssertUnwindSafe(|| {
                capability.evaluate(&snapshot.observations)
            }));
            match result {
                Ok(Ok(rows)) => match validate_metric_rows(rows) {
                    Ok(rows) => {
                        snapshot.metrics.insert(id.clone(), rows);
                    }
                    Err(message) => {
                        snapshot
                            .failures
                            .push(failure(id, ExtensionStage::Capability, message))
                    }
                },
                Ok(Err(message)) => {
                    snapshot
                        .failures
                        .push(failure(id, ExtensionStage::Capability, message))
                }
                Err(_) => snapshot.failures.push(failure(
                    id,
                    ExtensionStage::Capability,
                    "extension panicked",
                )),
            }
        }
        for (id, contribution) in &self.ui_contributions {
            let result = catch_unwind(AssertUnwindSafe(|| contribution.build_pane(&snapshot)));
            match result {
                Ok(Ok(pane)) => match validate_pane(pane) {
                    Ok(pane) => {
                        snapshot.panes.insert(id.clone(), pane);
                    }
                    Err(message) => {
                        snapshot
                            .failures
                            .push(failure(id, ExtensionStage::UiContribution, message))
                    }
                },
                Ok(Err(message)) => {
                    snapshot
                        .failures
                        .push(failure(id, ExtensionStage::UiContribution, message))
                }
                Err(_) => snapshot.failures.push(failure(
                    id,
                    ExtensionStage::UiContribution,
                    "extension panicked",
                )),
            }
        }
        snapshot
    }
}

fn insert_unique<T>(
    registry: &mut BTreeMap<ExtensionId, T>,
    id: ExtensionId,
    extension: T,
) -> Result<(), RegistrationError> {
    if registry.contains_key(&id) {
        return Err(RegistrationError::DuplicateId(id.0));
    }
    if registry.len() >= MAX_EXTENSIONS_PER_KIND {
        return Err(RegistrationError::RegistryFull);
    }
    registry.insert(id, extension);
    Ok(())
}

fn failure(id: &ExtensionId, stage: ExtensionStage, message: impl AsRef<str>) -> ExtensionFailure {
    ExtensionFailure {
        extension_id: id.clone(),
        stage,
        message: bounded(message.as_ref(), MAX_FAILURE_BYTES),
    }
}

fn bounded(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn validate_raw_observations(rows: Vec<RawObservation>) -> Result<Vec<RawObservation>, String> {
    if rows.len() > MAX_OBSERVATIONS_PER_HARNESS {
        return Err("observation count exceeds limit".into());
    }
    let mut event_revisions = BTreeSet::new();
    for row in &rows {
        if row.event_id.is_empty()
            || row.event_id.len() > MAX_EVENT_ID_BYTES
            || !row.event_id.as_bytes()[0].is_ascii_alphanumeric()
            || !row
                .event_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
        {
            return Err("event identity is invalid or too long".into());
        }
        if !event_revisions.insert((row.event_id.as_str(), row.revision)) {
            return Err("duplicate event identity and revision".into());
        }
        if !valid_identity_part(&row.identity.session_id, MAX_SESSION_KEY_BYTES)
            || !row
                .identity
                .worker_id
                .as_ref()
                .is_none_or(|value| valid_identity_part(value, MAX_SESSION_KEY_BYTES))
            || !row
                .identity
                .turn_id
                .as_ref()
                .is_none_or(|value| valid_identity_part(value, MAX_SESSION_KEY_BYTES))
            || !row
                .identity
                .operation_id
                .as_ref()
                .is_none_or(|value| valid_identity_part(value, MAX_SESSION_KEY_BYTES))
        {
            return Err("native session, worker, turn, or operation identity is invalid".into());
        }
        if let ObservationKind::Activity { category } = &row.kind
            && (category.is_empty() || category.len() > MAX_CATEGORY_BYTES)
        {
            return Err("activity category is empty or exceeds limit".into());
        }
    }
    Ok(rows)
}

fn valid_identity_part(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

fn validate_metric_rows(rows: Vec<MetricRow>) -> Result<Vec<MetricRow>, String> {
    if rows.len() > MAX_METRIC_ROWS {
        return Err("metric row count exceeds limit".into());
    }
    validate_metric_row_text(&rows)?;
    Ok(rows)
}

fn validate_metric_row_text(rows: &[MetricRow]) -> Result<(), String> {
    for row in rows {
        if row.label.is_empty()
            || row.label.len() > MAX_LABEL_BYTES
            || row.value.len() > MAX_VALUE_BYTES
        {
            return Err("metric row text is empty or exceeds limit".into());
        }
    }
    Ok(())
}

fn validate_pane(pane: PaneModel) -> Result<PaneModel, String> {
    if pane.title.is_empty() || pane.title.len() > MAX_TITLE_BYTES {
        return Err("pane title is empty or exceeds limit".into());
    }
    if pane.rows.len() > MAX_METRIC_ROWS {
        return Err("metric row count exceeds limit".into());
    }
    validate_metric_row_text(&pane.rows)?;
    Ok(pane)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedHarness(Result<Vec<RawObservation>, &'static str>);

    impl Harness for FixedHarness {
        fn poll(&mut self) -> Result<Vec<RawObservation>, String> {
            self.0.clone().map_err(str::to_owned)
        }
    }

    struct ActivityCount;

    impl Capability for ActivityCount {
        fn evaluate(&self, observations: &[Observation]) -> Result<Vec<MetricRow>, String> {
            let count = observations
                .iter()
                .filter(|row| matches!(row.kind, ObservationKind::Activity { .. }))
                .count();
            Ok(vec![MetricRow {
                label: "activity events".into(),
                value: count.to_string(),
            }])
        }
    }

    struct ActivityPane;

    impl UiContribution for ActivityPane {
        fn build_pane(&self, snapshot: &Snapshot) -> Result<PaneModel, String> {
            let count = snapshot.observations.len();
            Ok(PaneModel {
                title: "Activity".into(),
                rows: vec![MetricRow {
                    label: "observations".into(),
                    value: count.to_string(),
                }],
            })
        }
    }

    fn id(value: &str) -> ExtensionId {
        ExtensionId::parse(value).unwrap()
    }

    fn activity(session_key: &str) -> RawObservation {
        RawObservation {
            event_id: format!("fixture:{session_key}"),
            revision: 0,
            identity: ObservationIdentity {
                session_id: session_key.into(),
                worker_id: Some("worker-1".into()),
                turn_id: Some("turn-1".into()),
                operation_id: Some("operation-1".into()),
            },
            observed_at_unix_ns: 123,
            kind: ObservationKind::Activity {
                category: "tool".into(),
            },
        }
    }

    #[test]
    fn host_stamps_identity_and_builds_capability_and_pane_contributions() {
        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.codex"), FixedHarness(Ok(vec![activity("s1")])))
            .unwrap();
        kernel
            .register_capability(id("capability.activity-count"), ActivityCount)
            .unwrap();
        kernel
            .register_ui_contribution(id("pane.activity"), ActivityPane)
            .unwrap();

        let snapshot = kernel.refresh();
        assert_eq!(snapshot.observations.len(), 1);
        assert_eq!(snapshot.observations[0].event_id, "fixture:s1");
        assert_eq!(snapshot.observations[0].revision, 0);
        assert_eq!(snapshot.observations[0].identity.session_id, "s1");
        assert_eq!(
            snapshot.observations[0].identity.worker_id.as_deref(),
            Some("worker-1")
        );
        assert_eq!(
            snapshot.observations[0].harness_id.as_str(),
            "harness.codex"
        );
        assert_eq!(
            snapshot.metrics[&id("capability.activity-count")][0].value,
            "1"
        );
        assert_eq!(snapshot.panes[&id("pane.activity")].title, "Activity");
        assert!(snapshot.failures.is_empty());
    }

    #[test]
    fn failing_harness_does_not_hide_healthy_harness_output() {
        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.a-fails"), FixedHarness(Err("offline")))
            .unwrap();
        kernel
            .register_harness(
                id("harness.b-works"),
                FixedHarness(Ok(vec![activity("s2")])),
            )
            .unwrap();

        let snapshot = kernel.refresh();
        assert_eq!(snapshot.observations.len(), 1);
        assert_eq!(
            snapshot.observations[0].harness_id.as_str(),
            "harness.b-works"
        );
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::Poll);
    }

    #[test]
    fn invalid_and_duplicate_extension_ids_are_rejected() {
        assert!(matches!(
            ExtensionId::parse("Upper"),
            Err(RegistrationError::InvalidId(_))
        ));
        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.pi"), FixedHarness(Ok(vec![])))
            .unwrap();
        assert_eq!(
            kernel
                .register_harness(id("harness.pi"), FixedHarness(Ok(vec![])))
                .unwrap_err(),
            RegistrationError::DuplicateId("harness.pi".into()),
        );
    }

    #[test]
    fn duplicate_event_identity_in_one_harness_batch_is_rejected() {
        let mut kernel = Kernel::new();
        kernel
            .register_harness(
                id("harness.duplicate"),
                FixedHarness(Ok(vec![activity("same"), activity("same")])),
            )
            .unwrap();
        let snapshot = kernel.refresh();
        assert!(snapshot.observations.is_empty());
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::Poll);
    }

    #[test]
    fn invalid_optional_native_identity_rejects_the_harness_batch() {
        let mut row = activity("session");
        row.identity.turn_id = Some("turn with spaces".into());
        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.invalid-identity"), FixedHarness(Ok(vec![row])))
            .unwrap();

        let snapshot = kernel.refresh();
        assert!(snapshot.observations.is_empty());
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::Poll);
    }

    #[test]
    fn capability_failure_is_visible_and_other_extension_results_survive() {
        struct BrokenCapability;
        impl Capability for BrokenCapability {
            fn evaluate(&self, _: &[Observation]) -> Result<Vec<MetricRow>, String> {
                Err("broken extension".into())
            }
        }

        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.pi"), FixedHarness(Ok(vec![activity("s1")])))
            .unwrap();
        kernel
            .register_capability(id("capability.broken"), BrokenCapability)
            .unwrap();
        kernel
            .register_capability(id("capability.healthy"), ActivityCount)
            .unwrap();
        let snapshot = kernel.refresh();
        assert!(snapshot.metrics.contains_key(&id("capability.healthy")));
        assert!(!snapshot.metrics.contains_key(&id("capability.broken")));
        assert_eq!(snapshot.failures[0].stage, ExtensionStage::Capability);
    }

    #[test]
    fn harnesses_are_polled_in_stable_extension_id_order() {
        use std::sync::{Arc, Mutex};

        struct RecordingHarness(Arc<Mutex<Vec<String>>>, &'static str);
        impl Harness for RecordingHarness {
            fn poll(&mut self) -> Result<Vec<RawObservation>, String> {
                self.0.lock().unwrap().push(self.1.into());
                Ok(vec![])
            }
        }

        let order = Arc::new(Mutex::new(Vec::new()));
        let mut kernel = Kernel::new();
        kernel
            .register_harness(
                id("harness.z-last"),
                RecordingHarness(order.clone(), "z-last"),
            )
            .unwrap();
        kernel
            .register_harness(
                id("harness.a-first"),
                RecordingHarness(order.clone(), "a-first"),
            )
            .unwrap();
        kernel.refresh();
        assert_eq!(*order.lock().unwrap(), ["a-first", "z-last"]);
    }

    #[test]
    fn panics_and_unbounded_extension_output_become_visible_failures() {
        struct PanickingHarness;
        impl Harness for PanickingHarness {
            fn poll(&mut self) -> Result<Vec<RawObservation>, String> {
                panic!("broken adapter")
            }
        }

        struct OversizedCapability;
        impl Capability for OversizedCapability {
            fn evaluate(&self, _: &[Observation]) -> Result<Vec<MetricRow>, String> {
                Ok(vec![MetricRow {
                    label: "x".repeat(MAX_LABEL_BYTES + 1),
                    value: "1".into(),
                }])
            }
        }

        let mut kernel = Kernel::new();
        kernel
            .register_harness(id("harness.panic"), PanickingHarness)
            .unwrap();
        kernel
            .register_capability(id("capability.oversized"), OversizedCapability)
            .unwrap();
        let snapshot = kernel.refresh();
        assert_eq!(snapshot.failures.len(), 2);
        assert_eq!(snapshot.failures[0].message, "extension panicked");
        assert!(snapshot.failures[1].message.contains("metric row text"));
    }
}
