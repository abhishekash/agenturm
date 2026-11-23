use super::{describe_observation, duration_millis, metric, progress_description, timestamp};
use agenturm_kernel::{MetricRow, Observation, ObservationKind, TraceSpan};
use agenturm_ui::{EvidenceRow, EvidenceSpec, EvidenceTable};

pub(super) const ACTIVITY: EvidenceSpec = EvidenceSpec {
    title: "Activity",
    inspector: "Event detail",
    columns: &[("Time", 3), ("Type", 2), ("Result", 2), ("Action", 4)],
    empty: "No recorded activity on this page.",
};

pub(super) const FINDINGS: EvidenceSpec = EvidenceSpec {
    title: "Findings",
    inspector: "Evidence inspector",
    columns: &[("Severity", 2), ("Finding", 5), ("Last seen", 3)],
    empty: "No recorded diagnostics on this page.",
};

pub(super) const USAGE: EvidenceSpec = EvidenceSpec {
    title: "Usage",
    inspector: "Metric inspector",
    columns: &[
        ("Instrument", 3),
        ("Source", 3),
        ("Reported value", 5),
        ("Unit", 2),
    ],
    empty: "No reported usage on this page.",
};

pub(super) const TRACES: EvidenceSpec = EvidenceSpec {
    title: "Traces",
    inspector: "Execution tree",
    columns: &[
        ("Root result", 3),
        ("Trace", 5),
        ("Spans", 2),
        ("Root tok in/out", 4),
    ],
    empty: "No trace evidence on this page.",
};

pub(super) fn activity(evidence: &[&Observation]) -> EvidenceTable {
    ACTIVITY.table(
        evidence
            .iter()
            .map(|row| {
                let (kind, description) = describe_observation(&row.kind);
                let (result, action) = match &row.kind {
                    ObservationKind::SessionStarted => ("started", "Session started"),
                    ObservationKind::SessionEnded => ("ended", "Session ended"),
                    ObservationKind::Tool { category, result }
                    | ObservationKind::Check { category, result } => {
                        (result.as_str(), category.as_str())
                    }
                    ObservationKind::McpCall { method, result, .. } => {
                        (result.as_str(), method.as_str())
                    }
                    ObservationKind::Lifecycle { state, .. } => (state.as_str(), "Lifecycle"),
                    ObservationKind::TraceSpan(span) => {
                        (span.status.as_str(), span.span_id.as_str())
                    }
                    ObservationKind::Diagnostic { code, severity } => {
                        (severity.as_str(), code.as_str())
                    }
                    ObservationKind::WorkerSpawn(spawn) => {
                        ("declared", spawn.child_worker_id.as_str())
                    }
                    _ => ("—", description.as_str()),
                };
                let mut details = vec![match &row.kind {
                    ObservationKind::WorkerSpawn(spawn) => {
                        metric("Declared child worker", &spawn.child_worker_id)
                    }
                    _ => metric(kind, description.as_str()),
                }];
                details.extend(identity(row));
                details.push(metric("Time", timestamp(row)));
                EvidenceRow {
                    cells: vec![clock(row), kind.into(), result.into(), action.into()],
                    details,
                }
            })
            .collect(),
    )
}

pub(super) fn findings(evidence: &[&Observation]) -> EvidenceTable {
    FINDINGS.table(
        evidence
            .iter()
            .filter_map(|row| {
                let ObservationKind::Diagnostic { code, severity } = &row.kind else {
                    return None;
                };
                let time = timestamp(row);
                let mut details = vec![
                    metric("Severity", severity),
                    metric("Diagnostic", code),
                    metric("Time", &time),
                ];
                details.extend(identity(row));
                Some(EvidenceRow {
                    cells: vec![severity.clone(), code.clone(), clock(row)],
                    details,
                })
            })
            .collect(),
    )
}

pub(super) fn usage(evidence: &[&Observation]) -> EvidenceTable {
    USAGE.table(
        evidence
            .iter()
            .filter_map(|row| {
                let unit = match &row.kind {
                    ObservationKind::Usage { .. } | ObservationKind::SpanTokenUsage(_) => "tokens",
                    ObservationKind::CreditUsage { .. } => "credits",
                    ObservationKind::ReportedCost(cost) => cost.currency.as_str(),
                    _ => return None,
                };
                let (instrument, value) = describe_observation(&row.kind);
                let value = if value.is_empty() {
                    "Unavailable".into()
                } else {
                    value
                };
                let mut details = vec![metric(instrument, &value), metric("Unit", unit)];
                details.extend(identity(row));
                details.push(metric("Time", timestamp(row)));
                Some(EvidenceRow {
                    cells: vec![
                        instrument.into(),
                        row.harness_id.as_str().into(),
                        value,
                        unit.into(),
                    ],
                    details,
                })
            })
            .collect(),
    )
}

fn clock(row: &Observation) -> String {
    let nanos = row.observed_at_unix_ns;
    if nanos == 0 && matches!(row.kind, ObservationKind::Diagnostic { .. }) {
        return "Unknown".into();
    }
    #[cfg(target_os = "linux")]
    if let Ok(time) = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(nanos)) {
        return format!(
            "{:02}:{:02}:{:02}Z",
            time.hour(),
            time.minute(),
            time.second()
        );
    }
    format!("Unix {}", nanos / 1_000_000_000)
}

fn identity(row: &Observation) -> impl Iterator<Item = MetricRow> + '_ {
    let worker = row.identity.worker_id.as_deref();
    let worker = if matches!(row.kind, ObservationKind::WorkerSpawn(_)) {
        ("Parent worker", Some(worker.unwrap_or("Unreported")))
    } else {
        ("Worker", worker)
    };
    [
        ("Source", Some(row.harness_id.as_str())),
        ("Session", row.identity.session_id.as_deref()),
        ("Turn", row.identity.turn_id.as_deref()),
        worker,
        ("Operation", row.identity.operation_id.as_deref()),
        ("Event", Some(row.event_id.as_str())),
    ]
    .into_iter()
    .filter_map(|(label, value)| value.map(|value| metric(label, value)))
    .chain(std::iter::once_with(|| {
        metric("Revision", row.revision.to_string())
    }))
}

pub(super) fn trace(
    source: &str,
    session: &str,
    trace: &str,
    evidence: &super::TraceEvidence<'_>,
) -> EvidenceRow {
    let mut root_spans = evidence
        .spans
        .iter()
        .filter(|(_, span)| span.parent_span_id.is_none());
    let root = match (root_spans.next(), root_spans.next()) {
        (Some(&(row, span)), None) => Some((row, span)),
        _ => None,
    };
    let status = root.map_or("Unknown", |(_, span)| span.status.as_str());
    let tokens = root.and_then(|(row, root)| {
        let mut matches = evidence.usage.iter().filter(|(usage_row, usage)| {
            usage.span.span_id == root.span_id
                && usage.target_source_id == root.target_source_id
                && usage_row.identity.turn_id == row.identity.turn_id
                && usage_row.identity.worker_id == row.identity.worker_id
        });
        match (matches.next(), matches.next()) {
            (Some((_, usage)), None) => Some((usage.input_tokens, usage.output_tokens)),
            _ => None,
        }
    });
    let tokens = tokens
        .filter(|(input, output)| input.is_some() || output.is_some())
        .map_or_else(
            || "Unavailable".into(),
            |(input, output)| format!("{} / {}", token_count(input), token_count(output)),
        );
    let count = evidence.spans.len().to_string();
    let mut details = vec![
        metric("Trace", trace),
        metric("On this page", format!("root {status} · {count} span rows")),
        metric("Root tokens in / out", &tokens),
    ];
    details.extend(evidence.progress.values().map(|(row, progress)| {
        let mut progress = progress_description(progress);
        if let Some(turn) = row.identity.turn_id.as_deref() {
            progress.push_str(&format!(" · turn {turn}"));
        }
        if let Some(worker) = row.identity.worker_id.as_deref() {
            progress.push_str(&format!(" · worker {worker}"));
        }
        metric("Latest progress on page", progress)
    }));
    if let Some((row, span)) = root {
        details.push(span_detail(row, span));
    }
    details.extend([metric("Source", source), metric("Session", session)]);
    details.extend(
        evidence
            .spans
            .iter()
            .filter(|(row, _)| !root.is_some_and(|(root_row, _)| std::ptr::eq(*row, root_row)))
            .map(|&(row, span)| span_detail(row, span)),
    );
    EvidenceRow {
        cells: vec![status.into(), trace.into(), count, tokens],
        details,
    }
}

fn token_count(value: Option<u64>) -> String {
    value.map_or_else(|| "—".into(), |value| value.to_string())
}

fn span_detail(row: &Observation, span: &TraceSpan) -> MetricRow {
    let status = span.status.as_str();
    let duration = duration_millis(
        row.observed_at_unix_ns
            .saturating_sub(span.started_at_unix_ns),
    );
    let mut value = format!("{} · {status} · {duration}", span.span_id);
    if let Some(parent) = span.parent_span_id.as_deref() {
        value.push_str(&format!(" · parent {parent}"));
    }
    if let Some(target) = span.target_source_id.as_deref() {
        value.push_str(&format!(" · target {target}"));
    }
    metric(
        if span.parent_span_id.is_some() {
            "↳ Span"
        } else {
            "Span"
        },
        value,
    )
}
