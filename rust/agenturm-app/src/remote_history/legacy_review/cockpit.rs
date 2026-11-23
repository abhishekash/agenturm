//! Explicit exact-execution private view of a polled legacy receiver.

use super::*;
use crate::remote_history::RemoteHistoryOptions;
use agenturm_ui::{LiveReviewMode, LiveReviewSelection, PromptDelivery};

const MODAL_BYTES: usize = 16_000;

fn exact_selection(selection: &LiveReviewSelection, scope: &Scope) -> bool {
    let target = &selection.target;
    scope.receiver == target.receiver_id
        && scope.producer == target.owner.service
        && scope.session_id == target.owner.session_id
        && target.owner.agent_id.as_deref() == Some(scope.agent_id.as_str())
        && scope.run_id == target.trace_id
        && match selection.mode {
            LiveReviewMode::Run => scope.kind == "run" && scope.turn_id.is_empty(),
            LiveReviewMode::Turn => {
                scope.kind == "turn" && target.turn_id.as_deref() == Some(scope.turn_id.as_str())
            }
        }
}

pub(crate) fn deliver(
    receiver: &LegacyReceiver,
    options: &RemoteHistoryOptions,
    selection: &LiveReviewSelection,
    mut delivery: PromptDelivery,
) -> Result<(), String> {
    let target = &selection.target;
    let actor = target
        .owner
        .agent_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(SCOPE_ERROR)?;
    if target.receiver_id != receiver.native_identity()
        || !valid_trace(&target.trace_id)
        || !valid_identity(actor)
        || !valid_identity(&target.owner.service)
        || !valid_identity(&target.owner.session_id)
        || target.owner.session_id.is_empty()
        || (selection.mode == LiveReviewMode::Turn
            && !target.turn_id.as_deref().is_some_and(valid_trace))
    {
        return Err(SCOPE_ERROR.into());
    }
    let options = LegacyReviewOptions {
        receiver: options.clone(),
        service: target.owner.service.clone(),
        agent_id: actor.into(),
        session_id: target.owner.session_id.clone(),
        trace_id: target.trace_id.clone(),
        whole_turn: selection.mode == LiveReviewMode::Turn,
        pages: 5,
        format: crate::review_receipt::ReviewFormat::Json,
        output: None,
    };
    let deadline = Instant::now() + REVIEW_TIME;
    let review = prepare_review(receiver, &options, deadline, |scope, policy| {
        if policy.denied {
            return Err("Capture policy denies private Review".into());
        }
        if !exact_selection(selection, scope) {
            return Err(SCOPE_ERROR.into());
        }
        Ok(())
    })?;
    let full = output::render(&review.projection);
    let mut end = full.len().min(MODAL_BYTES);
    let encoded = loop {
        while !full.is_char_boundary(end) {
            end -= 1;
        }
        let mut text = full[..end].to_owned();
        if end < full.len() {
            text.push_str(
                "\n\nDisplay clipped at 16 KiB. Use Review export for the full bounded receipt.\n",
            );
        }
        text.push_str("\nCapture policy is checked by polling; this receiver has no server-held display lock.\n");
        let bytes = serde_json::to_vec(&text).map_err(|_| "Cannot format private Review")?;
        if bytes.len() <= 16_384 {
            break bytes;
        }
        end = end.saturating_sub(256);
    };
    delivery
        .write_all(&encoded)
        .map_err(|_| "Private Review display closed")?;
    delivery.flush_guarded(|deadline| review.authority.verify(receiver, deadline))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_ui::{LiveOwner, LiveReviewTarget};

    #[test]
    fn frozen_modal_scope_requires_exact_receiver_owner_trace_and_explicit_turn() {
        let mut selection = LiveReviewSelection {
            target: LiveReviewTarget {
                receiver_id: "receiver".into(),
                owner: LiveOwner {
                    service: "fixture".into(),
                    session_id: "session".into(),
                    agent_id: Some("actor".into()),
                },
                trace_id: "a".repeat(32),
                turn_id: Some("b".repeat(32)),
            },
            mode: LiveReviewMode::Run,
        };
        let mut scope = Scope {
            receiver: "receiver".into(),
            producer: "fixture".into(),
            agent_id: "actor".into(),
            session_id: "session".into(),
            run_id: "a".repeat(32),
            run_start_ns: None,
            kind: "run",
            endpoint_fallback: false,
            run_end_ns: None,
            turn_id: String::new(),
            turn_steps: BTreeSet::new(),
        };
        assert!(exact_selection(&selection, &scope));
        scope.agent_id = "sibling".into();
        assert!(!exact_selection(&selection, &scope));
        scope.agent_id = "actor".into();
        scope.receiver = "replaced".into();
        assert!(!exact_selection(&selection, &scope));
        scope.receiver = "receiver".into();
        scope.run_id = "c".repeat(32);
        assert!(!exact_selection(&selection, &scope));
        scope.run_id = "a".repeat(32);
        selection.mode = LiveReviewMode::Turn;
        assert!(!exact_selection(&selection, &scope));
        scope.kind = "turn";
        scope.turn_id = "b".repeat(32);
        assert!(exact_selection(&selection, &scope));
        selection.target.turn_id = None;
        assert!(!exact_selection(&selection, &scope));
        selection.target.owner.agent_id = None;
        assert!(!exact_selection(&selection, &scope));
    }
}
