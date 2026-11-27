//! Body-free Workflow incidents from a pinned current history.

use crate::HistoryReader;
use agenturm_kernel::{
    NativeCostBasis, Observation, ObservationKind, ObservationPage, ObservationQuery, ReadSnapshot,
    retained_history::HistoryProjection,
    workflow_alerts::{
        MAX_WORKFLOW_ALERTS, WorkflowAlert, WorkflowAlertClock, WorkflowAlertCoverage,
        WorkflowAlertEvidence, WorkflowAlertGaps, WorkflowAlertKind, WorkflowAlertPage,
        WorkflowAlertScope,
    },
    workflow_notifications::WorkflowThresholds,
};
use agenturm_store::native_attention::{
    ExecutionClockPage, ExecutionClockQuery, ExecutionClockStart, MAX_EXECUTION_CLOCK_SCOPES,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const EVIDENCE_LIMIT: usize = 5_000;

#[cfg(test)]
mod tests;

pub(super) fn load(
    reader: &impl HistoryReader,
    thresholds: &WorkflowThresholds,
) -> Result<WorkflowAlertPage, String> {
    read(reader, thresholds, false).map(|(page, _)| page)
}

pub(crate) struct WorkflowAlertSamplePage {
    pub history_identity: String,
    pub snapshot_id: ReadSnapshot,
    pub receiver_time_unix_ns: u64,
    pub samples: Vec<WorkflowAlertSample>,
}

pub(crate) struct WorkflowAlertSample {
    pub scope: WorkflowAlertScope,
    pub start_event_id: String,
    pub started_at_unix_ns: u64,
    pub tokens: Option<u64>,
    pub usd_nanos: Option<u64>,
}

pub(crate) fn load_with_samples(
    reader: &impl HistoryReader,
    thresholds: &WorkflowThresholds,
) -> Result<(WorkflowAlertPage, Option<WorkflowAlertSamplePage>), String> {
    read(reader, thresholds, true)
}

fn read(
    reader: &impl HistoryReader,
    thresholds: &WorkflowThresholds,
    want_samples: bool,
) -> Result<(WorkflowAlertPage, Option<WorkflowAlertSamplePage>), String> {
    if !thresholds.is_valid() {
        return Err("Workflow threshold is outside its supported range".into());
    }
    let history = reader
        .workspace_history_identity()
        .ok_or("Workflow alerts require an established history identity")?;
    let page = reader.query_projected_history(
        &ObservationQuery::latest(EVIDENCE_LIMIT),
        &HistoryProjection::WorkflowAlerts,
        None,
        None,
    )?;
    let inventory = inventory(&page);
    let need_clocks = thresholds.duration_ns > 0
        || (want_samples && (thresholds.tokens > 0 || thresholds.usd_nanos > 0));
    let scopes = if need_clocks {
        inventory
            .turns
            .iter()
            .filter(|(_, turn)| turn.running())
            .take(MAX_EXECUTION_CLOCK_SCOPES)
            .map(|(scope, _)| scope.clone())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let clocks = if scopes.is_empty() {
        None
    } else {
        reader
            .query_execution_clocks(&ExecutionClockQuery {
                snapshot_id: page.snapshot_id,
                scopes,
            })
            .ok()
            .filter(|clocks| clocks.snapshot_id == page.snapshot_id)
    };
    let (result, samples) = project(&page, &history, inventory, thresholds, clocks.as_ref());
    let sampled = if want_samples {
        clocks.and_then(|clock| {
            let receiver_time_unix_ns = clock.receiver_time_unix_ns?;
            let samples = if page.has_more {
                read_exact_samples(reader, &clock, receiver_time_unix_ns)
            } else {
                samples
            };
            Some(WorkflowAlertSamplePage {
                history_identity: history.clone(),
                snapshot_id: clock.snapshot_id,
                receiver_time_unix_ns,
                samples,
            })
        })
    } else {
        None
    };
    let current = reader.query_observations(&ObservationQuery::latest(1))?;
    if current.snapshot_id != page.snapshot_id
        || reader.workspace_history_identity().as_deref() != Some(&history)
    {
        return Err("Workflow history changed during the read. Refresh Workflow.".into());
    }
    Ok((result, sampled))
}

/// A broad history page can omit older usage for a current turn. Only a
/// complete exact scope at the already verified clock snapshot can seed a
/// threshold baseline. Truncated scopes remain omitted, not unknown values.
fn read_exact_samples(
    reader: &impl HistoryReader,
    clocks: &ExecutionClockPage,
    now: u64,
) -> Vec<WorkflowAlertSample> {
    clocks
        .active
        .iter()
        .take(MAX_EXECUTION_CLOCK_SCOPES)
        .filter_map(|start| {
            let scope = &start.scope;
            let query = ObservationQuery {
                harness_id: Some(scope.source_id.as_str().to_owned()),
                session_id: Some(scope.session_id.clone()),
                snapshot_id: Some(clocks.snapshot_id),
                ..ObservationQuery::latest(EVIDENCE_LIMIT)
            };
            let page = reader
                .query_projected_history(
                    &query,
                    &HistoryProjection::WorkflowAlerts,
                    Some(&scope.turn_id),
                    Some(scope.worker_id.as_deref()),
                )
                .ok()?;
            if page.has_more || page.snapshot_id != clocks.snapshot_id {
                return None;
            }
            let mut inventory = inventory(&page);
            if inventory.no_turn != 0 || inventory.turns.len() != 1 {
                return None;
            }
            let turn = inventory.turns.remove(scope)?;
            if !turn.running() {
                return None;
            }
            execution_sample(start, now, &turn.tokens, &turn.usd())
        })
        .collect()
}

#[derive(Default)]
struct ExactTurn<'a> {
    lifecycle: Option<&'a Observation>,
    terminal: BTreeMap<WorkflowAlertKind, &'a Observation>,
    tokens: Reported<'a>,
    costs: BTreeMap<&'a str, Option<&'a Observation>>,
}

impl<'a> ExactTurn<'a> {
    fn running(&self) -> bool {
        self.terminal.is_empty() && self.lifecycle.is_some_and(|row| {
            matches!(&row.kind, ObservationKind::Lifecycle { state, .. } if state == "running")
        })
    }

    fn usd(&self) -> Reported<'a> {
        let mut usd = Reported::default();
        for row in self.costs.values().flatten().copied() {
            if let ObservationKind::ReportedCost(cost) = &row.kind
                && cost.currency == "USD"
                && let Some(origin) = cost.native_origin
                && origin.basis != NativeCostBasis::Estimate
            {
                // Quantization can round up. A one-nano decrement bounds it.
                usd.add(
                    row,
                    cost.nanounits.saturating_sub(u64::from(origin.rounded)),
                );
            }
        }
        usd
    }
}

struct Inventory<'a> {
    turns: BTreeMap<WorkflowAlertScope, ExactTurn<'a>>,
    no_turn: usize,
}

#[derive(Default)]
struct Reported<'a> {
    value: u64,
    newest: Option<&'a Observation>,
}

impl<'a> Reported<'a> {
    fn add(&mut self, row: &'a Observation, value: u64) {
        self.value = self.value.saturating_add(value);
        if self.newest.is_none_or(|prior| after(row, prior)) {
            self.newest = Some(row);
        }
    }
}

fn after(row: &Observation, prior: &Observation) -> bool {
    (row.observed_at_unix_ns, row.event_id.as_str())
        > (prior.observed_at_unix_ns, prior.event_id.as_str())
}

fn inventory(page: &ObservationPage) -> Inventory<'_> {
    let mut turns = BTreeMap::<WorkflowAlertScope, ExactTurn<'_>>::new();
    let mut no_turn = BTreeSet::new();
    // The typed history projection already discarded superseded revisions,
    // including corrections with older timestamps or changed ownership.
    for row in &page.observations {
        if row.harness_id.as_str().starts_with("otlp.")
            || !matches!(
                row.kind,
                ObservationKind::Lifecycle { .. }
                    | ObservationKind::Usage { .. }
                    | ObservationKind::ReportedCost(_)
            )
        {
            continue;
        }
        let Some(session) = &row.identity.session_id else {
            continue;
        };
        let Some(turn) = &row.identity.turn_id else {
            no_turn.insert((&row.harness_id, session, &row.identity.worker_id));
            continue;
        };
        let scope = WorkflowAlertScope {
            source_id: row.harness_id.clone(),
            session_id: session.clone(),
            turn_id: turn.clone(),
            worker_id: row.identity.worker_id.clone(),
        };
        ingest(turns.entry(scope).or_default(), row);
    }
    Inventory {
        turns,
        no_turn: no_turn.len(),
    }
}

fn project(
    page: &ObservationPage,
    history: &str,
    inventory: Inventory<'_>,
    thresholds: &WorkflowThresholds,
    active: Option<&ExecutionClockPage>,
) -> (WorkflowAlertPage, Vec<WorkflowAlertSample>) {
    let active = active.filter(|clock| clock.snapshot_id == page.snapshot_id);
    let clocks = active
        .into_iter()
        .flat_map(|page| &page.active)
        .map(|start| (&start.scope, start))
        .collect::<BTreeMap<_, _>>();
    let now = active.and_then(|clock| clock.receiver_time_unix_ns);
    let mut result = WorkflowAlertPage {
        history_identity: history.to_owned(),
        snapshot_id: page.snapshot_id,
        alerts: Vec::new(),
        partial: page.has_more,
        gaps: WorkflowAlertGaps {
            no_turn: inventory.no_turn,
            ..WorkflowAlertGaps::default()
        },
    };
    let mut samples = Vec::new();
    for (scope, turn) in inventory.turns {
        let clock = clocks.get(&scope).copied().zip(now);
        if let Some(sample) = add_turn(&mut result, scope, turn, thresholds, clock) {
            samples.push(sample);
        }
    }
    result.alerts.sort_unstable_by(|left, right| {
        (
            left.kind,
            std::cmp::Reverse(left.evidence.observed_at_unix_ns),
            &left.id,
        )
            .cmp(&(
                right.kind,
                std::cmp::Reverse(right.evidence.observed_at_unix_ns),
                &right.id,
            ))
    });
    result.partial |= result.alerts.len() > MAX_WORKFLOW_ALERTS;
    result.alerts.truncate(MAX_WORKFLOW_ALERTS);
    (result, samples)
}

fn ingest<'a>(turn: &mut ExactTurn<'a>, row: &'a Observation) {
    match &row.kind {
        ObservationKind::Lifecycle { state, .. } => {
            if turn.lifecycle.is_none_or(|prior| after(row, prior)) {
                turn.lifecycle = Some(row);
            }
            if let Some(kind) = terminal_kind(state) {
                let prior = turn.terminal.entry(kind).or_insert(row);
                if after(row, prior) {
                    *prior = row;
                }
            }
        }
        ObservationKind::Usage {
            input_tokens,
            output_tokens,
            ..
        } => {
            if input_tokens.is_some() || output_tokens.is_some() {
                turn.tokens.add(
                    row,
                    input_tokens
                        .unwrap_or(0)
                        .saturating_add(output_tokens.unwrap_or(0)),
                );
            }
        }
        ObservationKind::ReportedCost(cost)
            if cost.native_origin.is_some() && cost.span.is_none() =>
        {
            if let Some(assistant) = row.identity.operation_id.as_deref() {
                // Multiple current cost facts for one assistant occurrence
                // are ambiguous even when they happen to quote one amount.
                turn.costs
                    .entry(assistant)
                    .and_modify(|prior| *prior = None)
                    .or_insert(Some(row));
            }
        }
        _ => {}
    }
}

fn terminal_kind(state: &str) -> Option<WorkflowAlertKind> {
    Some(match state {
        "failed" => WorkflowAlertKind::Failure,
        "interrupted" => WorkflowAlertKind::Interruption,
        "completed" => WorkflowAlertKind::Completion,
        _ => return None,
    })
}

fn add_turn(
    page: &mut WorkflowAlertPage,
    scope: WorkflowAlertScope,
    turn: ExactTurn<'_>,
    thresholds: &WorkflowThresholds,
    clock: Option<(&ExecutionClockStart, u64)>,
) -> Option<WorkflowAlertSample> {
    let running = turn.running();
    let usd = turn.usd();
    let sample = clock
        .filter(|_| running)
        .and_then(|(start, now)| execution_sample(start, now, &turn.tokens, &usd));
    for (kind, row) in turn.terminal {
        push(page, &scope, kind, source_evidence(row), None);
    }
    for (kind, limit, report) in [
        (WorkflowAlertKind::Tokens, thresholds.tokens, turn.tokens),
        (WorkflowAlertKind::UsdCost, thresholds.usd_nanos, usd),
    ] {
        if limit == 0 {
            continue;
        }
        if let Some(row) = report.newest {
            if report.value >= limit {
                push(
                    page,
                    &scope,
                    kind,
                    source_evidence(row),
                    Some((report.value, limit)),
                );
            }
        } else if kind == WorkflowAlertKind::Tokens {
            page.gaps.tokens_unreported += 1;
        } else {
            page.gaps.usd_unreported += 1;
        }
    }
    if !running {
        return None;
    }
    if let Some((proof, (_, now))) = sample.as_ref().zip(clock) {
        let duration = now - proof.started_at_unix_ns;
        if thresholds.duration_ns > 0 && duration >= thresholds.duration_ns {
            push(
                page,
                &scope,
                WorkflowAlertKind::Elapsed,
                WorkflowAlertEvidence {
                    event_id: proof.start_event_id.clone(),
                    revision: 0,
                    observed_at_unix_ns: proof.started_at_unix_ns,
                    clock: WorkflowAlertClock::ReceiverReceipt,
                },
                Some((duration, thresholds.duration_ns)),
            );
        }
        return sample;
    }
    if thresholds.duration_ns > 0 {
        page.gaps.elapsed_unproven += 1;
    }
    None
}

fn execution_sample(
    start: &ExecutionClockStart,
    now: u64,
    tokens: &Reported<'_>,
    usd: &Reported<'_>,
) -> Option<WorkflowAlertSample> {
    (start.receipt_time_unix_ns > 0 && start.receipt_time_unix_ns <= now).then(|| {
        WorkflowAlertSample {
            scope: start.scope.clone(),
            start_event_id: start.event_id.clone(),
            started_at_unix_ns: start.receipt_time_unix_ns,
            tokens: tokens.newest.map(|_| tokens.value),
            usd_nanos: usd.newest.map(|_| usd.value),
        }
    })
}

fn source_evidence(row: &Observation) -> WorkflowAlertEvidence {
    WorkflowAlertEvidence {
        event_id: row.event_id.clone(),
        revision: row.revision,
        observed_at_unix_ns: row.observed_at_unix_ns,
        clock: WorkflowAlertClock::SourceReported,
    }
}

fn push(
    page: &mut WorkflowAlertPage,
    scope: &WorkflowAlertScope,
    kind: WorkflowAlertKind,
    evidence: WorkflowAlertEvidence,
    measure: Option<(u64, u64)>,
) {
    let incarnation = page
        .snapshot_id
        .incarnation()
        .map(|value| value.to_string());
    let anchor = match measure {
        Some((_, threshold)) => serde_json::json!(["policy", threshold]),
        None => serde_json::json!(["event", evidence.event_id, evidence.revision]),
    };
    let identity = serde_json::json!([
        "agenturm-workflow-alert-v1",
        page.history_identity,
        incarnation,
        scope,
        kind,
        anchor,
    ]);
    page.alerts.push(WorkflowAlert {
        id: format!("{:x}", Sha256::digest(identity.to_string())),
        scope: scope.clone(),
        kind,
        evidence,
        measured: measure.map(|(value, _)| value),
        threshold: measure.map(|(_, limit)| limit),
        coverage: if measure.is_some() {
            WorkflowAlertCoverage::ReportedLowerBound
        } else {
            WorkflowAlertCoverage::ExactFact
        },
        quiet: kind == WorkflowAlertKind::Completion,
    });
}
