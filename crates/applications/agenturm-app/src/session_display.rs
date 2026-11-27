//! Human display context accompanies, without replacing, exact inspector identity.
use agenturm_kernel::{ExtensionId, MetricRow, Snapshot};
use agenturm_ui::{ProjectOrigin, RecordedSession};

pub(super) fn add_context(snapshot: &mut Snapshot, pane: &ExtensionId, session: &RecordedSession) {
    let Some(pane) = snapshot.panes.get_mut(pane) else {
        return;
    };
    let mut rows = vec![MetricRow {
        label: if session
            .task_title
            .as_deref()
            .is_some_and(|title| !title.trim().is_empty())
        {
            "Task name"
        } else {
            "Session name"
        }
        .into(),
        value: session.display_name().into_owned(),
    }];
    rows.push(MetricRow {
        label: "Provider".into(),
        value: session.provider_label().into(),
    });
    if let Some(context) = &session.native_context {
        for (label, value) in [
            ("Agent name", context.agent_name.as_deref()),
            ("Agent role", context.agent_role.as_deref()),
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
    } else {
        rows.push(MetricRow {
            label: "Project".into(),
            value: "No recorded project".into(),
        });
    }
    rows.push(MetricRow {
        label: "Recorded state".into(),
        value: session.state.as_deref().unwrap_or("Unknown").into(),
    });
    if let Some(reason) = session
        .reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
    {
        rows.push(MetricRow {
            label: "Recorded reason".into(),
            value: reason.into(),
        });
    }
    // Human context leads the Overview; exact source, session and worker
    // identity stays in Details.
    rows.append(&mut pane.rows);
    pane.rows = rows;
}
