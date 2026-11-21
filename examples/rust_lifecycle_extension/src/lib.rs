//! Separately packaged example capability and data-only UI contribution.

#![forbid(unsafe_code)]

use agenturm_kernel::{
    Capability, MetricRow, Observation, ObservationKind, PaneModel, Snapshot, UiContribution,
};
use std::collections::BTreeMap;

/// Counts lifecycle facts without retaining message or process content.
pub struct LifecycleCounts;

impl Capability for LifecycleCounts {
    fn evaluate(&self, observations: &[Observation]) -> Result<Vec<MetricRow>, String> {
        let lifecycle = observations
            .iter()
            .filter(|row| matches!(row.kind, ObservationKind::Lifecycle { .. }))
            .count();
        let interrupted = observations
            .iter()
            .filter(|row| {
                matches!(
                    row.kind,
                    ObservationKind::Lifecycle { ref state, .. } if state == "interrupted"
                )
            })
            .count();
        Ok(vec![
            MetricRow {
                label: "Lifecycle events".into(),
                value: lifecycle.to_string(),
            },
            MetricRow {
                label: "Interrupted events".into(),
                value: interrupted.to_string(),
            },
        ])
    }
}

/// Summarizes lifecycle evidence as a data-only pane for the host renderer.
pub struct LifecyclePane;

impl UiContribution for LifecyclePane {
    fn build_pane(&self, snapshot: &Snapshot) -> Result<PaneModel, String> {
        let mut by_source = BTreeMap::<&str, (usize, usize)>::new();
        for row in &snapshot.observations {
            if let ObservationKind::Lifecycle { state, .. } = &row.kind {
                let counts = by_source.entry(row.harness_id.as_str()).or_default();
                counts.0 += 1;
                counts.1 += usize::from(state == "interrupted");
            }
        }
        Ok(PaneModel {
            title: "Lifecycle by source".into(),
            rows: by_source
                .into_iter()
                .map(|(source, (total, interrupted))| MetricRow {
                    label: source.into(),
                    value: format!("{total} total · {interrupted} interrupted"),
                })
                .collect(),
        })
    }
}
