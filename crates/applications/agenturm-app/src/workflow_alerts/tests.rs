use super::*;
use agenturm_kernel::{ExtensionId, ObservationIdentity, ObservationStore, ReportedCost};
use agenturm_store::{
    Store,
    native_attention::{NativeAttentionCapture, NativeAttentionKind, NativeAttentionScope},
};

fn row(
    source: &str,
    event: &str,
    turn: Option<&str>,
    worker: Option<&str>,
    kind: ObservationKind,
) -> Observation {
    let mut identity = ObservationIdentity::session("same-session");
    identity.turn_id = turn.map(str::to_owned);
    identity.worker_id = worker.map(str::to_owned);
    Observation {
        harness_id: ExtensionId::parse(source).unwrap(),
        event_id: event.into(),
        revision: 0,
        identity,
        observed_at_unix_ns: 25,
        kind,
    }
}

fn state(label: &str) -> ObservationKind {
    ObservationKind::Lifecycle {
        state: label.into(),
        reason_code: Some("private-fixture-reason".into()),
    }
}

fn usage(input_tokens: Option<u64>, output_tokens: Option<u64>) -> ObservationKind {
    ObservationKind::Usage {
        input_tokens,
        output_tokens,
        cache_read_tokens: Some(800),
        cache_write_tokens: Some(800),
        total_tokens: Some(1_600),
    }
}

fn cost(
    event: &str,
    operation: &str,
    currency: &str,
    nano: u64,
    (basis, rounded): (NativeCostBasis, bool),
) -> Observation {
    let mut row = row(
        "harness.one",
        event,
        Some("shared-turn"),
        None,
        ObservationKind::ReportedCost(
            serde_json::from_value::<ReportedCost>(serde_json::json!({
                "currency": currency, "nanounits": nano,
                "native_origin": {"basis": basis, "rounded": rounded},
            }))
            .unwrap(),
        ),
    );
    row.identity.operation_id = Some(operation.into());
    row
}

fn observed(rows: Vec<Observation>) -> ObservationPage {
    ObservationPage {
        observations: rows,
        snapshot_id: 4.into(),
        has_more: false,
    }
}

#[test]
fn body_free_terminal_causes_keep_exact_source_worker_and_quiet_completion() {
    let evidence = observed(vec![
        row(
            "harness.one",
            "failure",
            Some("shared-turn"),
            None,
            state("failed"),
        ),
        row(
            "harness.one",
            "interruption",
            Some("shared-turn"),
            Some("side"),
            state("interrupted"),
        ),
        row(
            "harness.two",
            "completion",
            Some("shared-turn"),
            None,
            state("completed"),
        ),
        row("harness.two", "unanchored", None, None, state("completed")),
        row(
            "harness.two",
            "tool-exit",
            Some("shared-turn"),
            None,
            ObservationKind::Tool {
                category: "shell".into(),
                result: "failed".into(),
            },
        ),
    ]);
    let (page, _) = project(
        &evidence,
        "history",
        inventory(&evidence),
        &WorkflowThresholds::default(),
        None,
    );
    let [failure, interruption, completion] = page.alerts.as_slice() else {
        panic!("three exact outcomes");
    };
    assert_eq!(failure.kind, WorkflowAlertKind::Failure);
    assert_eq!(failure.scope.source_id.as_str(), "harness.one");
    assert_eq!(failure.scope.worker_id, None);
    assert_eq!(interruption.kind, WorkflowAlertKind::Interruption);
    assert_eq!(interruption.scope.worker_id.as_deref(), Some("side"));
    assert_eq!(completion.kind, WorkflowAlertKind::Completion);
    assert_eq!(completion.scope.source_id.as_str(), "harness.two");
    assert!(completion.quiet);
    assert!(!failure.quiet && !interruption.quiet);
    assert_eq!(page.gaps.no_turn, 1);
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains("private-fixture-reason")
    );
}

#[test]
fn threshold_id_is_stable_per_policy_and_scope_and_known_totals_never_mix() {
    let exact_unknown_basis = (NativeCostBasis::Unknown, false);
    let estimate = (NativeCostBasis::Estimate, false);
    let mut evidence = observed(vec![
        row(
            "harness.one",
            "native-a",
            Some("shared-turn"),
            None,
            usage(Some(2), Some(8)),
        ),
        row(
            "harness.one",
            "native-zero",
            Some("shared-turn"),
            Some("side"),
            usage(Some(0), None),
        ),
        row(
            "harness.two",
            "native-b",
            Some("shared-turn"),
            None,
            usage(Some(2), Some(2)),
        ),
        row(
            "otlp.fixture",
            "telemetry",
            Some("shared-turn"),
            None,
            usage(Some(900), Some(900)),
        ),
        cost("cost-a", "msg-a", "USD", 4, exact_unknown_basis),
        cost(
            "cost-b",
            "msg-b",
            "USD",
            7,
            (NativeCostBasis::Unknown, true),
        ),
        cost(
            "duplicate-a",
            "msg-duplicate",
            "USD",
            100,
            exact_unknown_basis,
        ),
        cost(
            "duplicate-b",
            "msg-duplicate",
            "USD",
            100,
            exact_unknown_basis,
        ),
        cost("est", "msg-c", "USD", 1000, estimate),
        cost("eur", "msg-d", "EUR", 1000, exact_unknown_basis),
    ]);
    let policy = WorkflowThresholds {
        tokens: 9,
        usd_nanos: 10,
        duration_ns: 0,
    };
    let (prior, _) = project(&evidence, "history", inventory(&evidence), &policy, None);
    let [tokens, usd] = prior.alerts.as_slice() else {
        panic!("two exact threshold alerts");
    };
    assert_eq!(
        (tokens.kind, tokens.measured),
        (WorkflowAlertKind::Tokens, Some(10))
    );
    assert_eq!(
        (usd.kind, usd.measured),
        (WorkflowAlertKind::UsdCost, Some(10))
    );
    assert_eq!(tokens.coverage, WorkflowAlertCoverage::ReportedLowerBound);
    assert_eq!(prior.gaps.tokens_unreported, 0);
    assert_eq!(prior.gaps.usd_unreported, 2);
    evidence.snapshot_id = 5.into();
    evidence.has_more = true;
    evidence.observations.push(row(
        "harness.one",
        "later-usage",
        Some("shared-turn"),
        None,
        usage(Some(1), None),
    ));
    let (more, _) = project(&evidence, "history", inventory(&evidence), &policy, None);
    assert!(more.partial);
    assert_eq!(more.alerts[0].id, tokens.id);
    assert_eq!(more.alerts[0].measured, Some(11));
    let changed_policy = WorkflowThresholds {
        tokens: 10,
        ..policy
    };
    let (changed, _) = project(
        &evidence,
        "history",
        inventory(&evidence),
        &changed_policy,
        None,
    );
    assert_ne!(changed.alerts[0].id, tokens.id);
    assert_eq!(changed.alerts[1].id, usd.id);
}

#[test]
fn elapsed_requires_current_exact_start_and_keeps_known_zero_distinct() {
    let evidence = observed(vec![
        row(
            "harness.one",
            "running",
            Some("shared-turn"),
            None,
            state("running"),
        ),
        row(
            "harness.one",
            "zero",
            Some("shared-turn"),
            None,
            usage(Some(0), None),
        ),
        row(
            "harness.one",
            "side",
            Some("shared-turn"),
            Some("side"),
            state("running"),
        ),
    ]);
    let scopes = inventory(&evidence);
    let scope = scopes
        .turns
        .keys()
        .find(|scope| scope.worker_id.is_none())
        .unwrap()
        .clone();
    let policy = WorkflowThresholds {
        duration_ns: 25,
        tokens: 1,
        usd_nanos: 1,
    };
    let mut clock = ExecutionClockPage {
        snapshot_id: 3.into(),
        receiver_time_unix_ns: Some(130),
        active: vec![ExecutionClockStart {
            scope: scope.clone(),
            event_id: "start-receipt".into(),
            receipt_time_unix_ns: 100,
        }],
    };
    let (stale, samples) = project(
        &evidence,
        "history",
        inventory(&evidence),
        &policy,
        Some(&clock),
    );
    assert!(stale.alerts.is_empty() && samples.is_empty());
    assert_eq!(stale.gaps.elapsed_unproven, 2);
    clock.snapshot_id = evidence.snapshot_id;
    let (current, samples) = project(
        &evidence,
        "history",
        inventory(&evidence),
        &policy,
        Some(&clock),
    );
    let [elapsed] = current.alerts.as_slice() else {
        panic!("one exact elapsed alert");
    };
    assert_eq!(elapsed.scope, scope);
    assert_eq!(
        (elapsed.kind, elapsed.measured, elapsed.threshold),
        (WorkflowAlertKind::Elapsed, Some(30), Some(25))
    );
    assert_eq!(elapsed.evidence.clock, WorkflowAlertClock::ReceiverReceipt);
    assert_eq!(elapsed.evidence.observed_at_unix_ns, 100);
    assert_eq!(current.gaps.elapsed_unproven, 1);
    let [sample] = samples.as_slice() else {
        panic!("one exact sample");
    };
    assert_eq!((sample.tokens, sample.usd_nanos), (Some(0), None));
    assert_eq!(sample.start_event_id, "start-receipt");
}

#[test]
fn old_unrelated_history_cannot_hide_a_complete_threshold_transition() {
    const MAIN: &str = "harness.current";
    const ARCHIVE: &str = "harness.archive";
    const TURN: Option<&str> = Some("shared-turn");
    let mut store = Store::open(":memory:").unwrap();
    let mut running = row(MAIN, "running", TURN, None, state("running"));
    running.observed_at_unix_ns = 100;
    let mut baseline = row(MAIN, "baseline", TURN, None, usage(Some(2), None));
    baseline.observed_at_unix_ns = 1;
    let sibling = row(
        MAIN,
        "sibling",
        TURN,
        Some("sibling"),
        usage(Some(900), None),
    );
    ObservationStore::commit(
        &mut store,
        MAIN,
        None,
        "main-1",
        &[running, baseline, sibling],
    )
    .unwrap();
    let mut preceding = None;
    for chunk in 0..5 {
        let rows: Vec<_> = (0..1_001)
            .map(|index| {
                row(
                    ARCHIVE,
                    &format!("old-{chunk}-{index}"),
                    TURN,
                    None,
                    usage(Some(10), None),
                )
            })
            .collect();
        let next = format!("archive-{chunk}");
        ObservationStore::commit(&mut store, ARCHIVE, preceding.as_deref(), &next, &rows).unwrap();
        preceding = Some(next);
    }
    let collector = ExtensionId::parse("otlp.native-start").unwrap();
    let capture = NativeAttentionCapture {
        history_binding: store.history_identity().unwrap(),
        scope: NativeAttentionScope {
            source_id: MAIN.into(),
            session_id: "same-session".into(),
            turn_id: TURN.unwrap().into(),
            worker_id: None,
        },
        sender_id: "1".repeat(32),
        event_id: "start".into(),
        sequence: 1,
        kind: NativeAttentionKind::TurnStarted,
        request_id: None,
        source_time_unix_ns: None,
    };
    store
        .admit_native_attention(&collector, &capture, None)
        .unwrap();
    let policy = WorkflowThresholds {
        tokens: 5,
        ..WorkflowThresholds::default()
    };
    let (display, proof) = load_with_samples(&store, &policy).unwrap();
    assert!(display.partial);
    let first = proof.unwrap();
    let [before] = first.samples.as_slice() else {
        panic!("complete current scope sample");
    };
    assert_eq!(before.tokens, Some(2));
    assert_eq!(before.scope.source_id.as_str(), MAIN);
    assert_eq!(before.scope.worker_id, None);

    let mut newer = row(MAIN, "new-report", TURN, None, usage(Some(4), None));
    newer.observed_at_unix_ns = 101;
    ObservationStore::commit(&mut store, MAIN, Some("main-1"), "main-2", &[newer]).unwrap();
    let (_, proof) = load_with_samples(&store, &policy).unwrap();
    let second = proof.unwrap();
    let [after] = second.samples.as_slice() else {
        panic!("scoped current transition");
    };
    assert_eq!(after.tokens, Some(6));
    assert_eq!(after.scope, before.scope);
    assert_eq!(after.start_event_id, before.start_event_id);
    assert!(second.snapshot_id.watermark() > first.snapshot_id.watermark());

    let mut preceding = "main-2".to_owned();
    for chunk in 0..5 {
        let rows: Vec<_> = (0..1_001)
            .map(|index| {
                row(
                    MAIN,
                    &format!("scope-overflow-{chunk}-{index}"),
                    TURN,
                    None,
                    usage(Some(10), None),
                )
            })
            .collect();
        let next = format!("scope-overflow-{chunk}");
        ObservationStore::commit(&mut store, MAIN, Some(&preceding), &next, &rows).unwrap();
        preceding = next;
    }
    let (partial, proof) = load_with_samples(&store, &policy).unwrap();
    assert!(partial.partial);
    assert!(proof.unwrap().samples.is_empty());
}
