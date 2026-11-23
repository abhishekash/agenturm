//! Human display context accompanies, without replacing, exact inspector identity.
use agenturm_kernel::{ExtensionId, MetricRow, Snapshot};
use agenturm_ui::{ProjectOrigin, RecordedSession};

pub(super) fn add_context(snapshot: &mut Snapshot, pane: &ExtensionId, session: &RecordedSession) {
    let Some(pane) = snapshot.panes.get_mut(pane) else {
        return;
    };
    let mut rows = vec![MetricRow {
        label: if session.task_title.is_some() {
            "Task name"
        } else {
            "Session name"
        }
        .into(),
        value: session.display_name().into_owned(),
    }];
    if let Some(context) = &session.native_context {
        for (label, value) in [
            ("Agent name", context.agent_name.as_deref()),
            ("Agent role", context.agent_role.as_deref()),
            ("Agent path", context.agent_path.as_deref()),
            ("Model provider", context.model_provider.as_deref()),
            ("Last recorded activity", context.last_activity.as_deref()),
        ] {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                rows.push(MetricRow {
                    label: label.into(),
                    value: value.into(),
                });
            }
        }
    }
    if let (Some(path), Some(origin)) = (&session.project_path, session.project_origin) {
        rows.push(MetricRow {
            label: "Project".into(),
            value: path.to_string(),
        });
        rows.push(MetricRow {
            label: "Project source".into(),
            value: match origin {
                ProjectOrigin::OperatorTaskWorkspace => "Operator task workspace",
                ProjectOrigin::NativeRecordedWorkspace => "Native session workspace",
            }
            .into(),
        });
    }
    // Keep exact identity reachable even when a long display name wraps at 60 columns.
    let position = pane
        .rows
        .iter()
        .position(|row| row.label == "Session")
        .map_or(0, |index| index + 1);
    pane.rows.splice(position..position, rows);
}
