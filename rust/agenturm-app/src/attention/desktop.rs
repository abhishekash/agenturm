//! Desktop delivery for recent native requests with comparable source clocks.

use super::{Snapshot, snapshot_report};
use crate::{
    HistoryReader,
    cockpit_workflows::{project_requests, request_key},
};
use agenturm_kernel::native_review::{NativeRequestKind, NativeRequestState};
use agenturm_kernel::{Observation, ObservationKind, ObservationQuery, ReadSnapshot};
use agenturm_store::{DesktopAttentionRequest, MAX_WORKSPACE_PAGE_SIZE, WorkspaceLedger};
use agenturm_ui::{RequestEntry, RequestProof};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const HISTORY_PAGE_SIZE: usize = 5_000;
const FRESHNESS_NS: u64 = 15_000_000_000;

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Delivery {
    DeferredIncomplete,
    DeferredSourceClock,
    DeferredHistoryChanged,
    Delivered,
    Unavailable,
    Failed,
}

pub(super) fn current_position(
    store: &impl HistoryReader,
) -> Result<(String, ReadSnapshot), String> {
    let identity = store
        .workspace_history_identity()
        .ok_or("Desktop attention requires an established history identity")?;
    let page = store.query_observations(&ObservationQuery::latest(1))?;
    Ok((identity, page.snapshot_id))
}

pub(super) fn snapshot(
    store: &impl HistoryReader,
    ledger: &WorkspaceLedger,
    stop: &AtomicBool,
    current: impl Fn() -> Result<(String, ReadSnapshot), String>,
) -> Result<Snapshot, String> {
    let identity = store
        .workspace_history_identity()
        .ok_or("Desktop attention requires an established history identity")?;
    let page = store.query_observations(&ObservationQuery::latest(HISTORY_PAGE_SIZE))?;
    let proof = RequestProof {
        snapshot_id: page.snapshot_id,
        history_identity: Some(identity.clone()),
    };
    let mut requests = project_requests(&page.observations, &proof);
    let now: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|time| time.as_nanos().try_into().ok())
        .ok_or("Desktop attention requires a valid local clock")?;
    let cutoff = now.saturating_sub(FRESHNESS_NS);
    let fresh_overflow = requests
        .get(MAX_WORKSPACE_PAGE_SIZE)
        .is_some_and(|row| row.timestamp >= cutoff);
    let partial = requests.len() > MAX_WORKSPACE_PAGE_SIZE || page.has_more;
    requests.truncate(MAX_WORKSPACE_PAGE_SIZE);
    let keys = requests
        .iter()
        .map(request_key)
        .collect::<Result<Vec<_>, _>>()?;
    let seen = ledger
        .seen_requests(&keys)
        .map_err(|error| error.to_string())?;
    for (request, key) in requests.iter_mut().zip(keys) {
        request.seen = seen.contains(&key);
    }
    // The Store selects the newest page and returns its rows chronologically.
    let complete = !fresh_overflow
        && covers_clock_window(
            page.has_more,
            page.observations.first().map(|row| row.observed_at_unix_ns),
            cutoff,
        );
    if !complete {
        return Ok(snapshot_report(
            requests,
            partial,
            Some(Delivery::DeferredIncomplete),
        ));
    }

    let native_requests = page
        .observations
        .iter()
        .filter_map(|row| match &row.kind {
            ObservationKind::NativeRequest(_) => Some((
                (row.harness_id.as_str(), row.event_id.as_str(), row.revision),
                row,
            )),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let mut candidates = Vec::new();
    for row in requests
        .iter()
        .filter(|row| !row.seen && row.timestamp >= cutoff)
    {
        if row.timestamp > now || !has_comparable_clock(row, &native_requests) {
            return Ok(snapshot_report(
                requests,
                partial,
                Some(Delivery::DeferredSourceClock),
            ));
        }
        let (anchor, event_id, revision) = request_key(row)?;
        candidates.push(DesktopAttentionRequest {
            anchor,
            event_id,
            revision,
            observed_at_unix_ns: row.timestamp,
        });
    }

    let expected = (identity, page.snapshot_id);
    if stop.load(Ordering::Relaxed) || current()? != expected {
        return Ok(snapshot_report(
            requests,
            partial,
            Some(Delivery::DeferredHistoryChanged),
        ));
    }
    let claimed = ledger
        .claim_desktop_attention(&expected.0, expected.1, now, &candidates)
        .map_err(|error| format!("Desktop attention claim failed: {error}"))?;
    let delivered = if claimed == 0 || stop.load(Ordering::Relaxed) {
        None
    } else if current()? != expected {
        Some(Delivery::DeferredHistoryChanged)
    } else {
        Some(notify(claimed, stop))
    };
    Ok(snapshot_report(requests, partial, delivered))
}

fn covers_clock_window(has_older_rows: bool, last_timestamp: Option<u64>, cutoff: u64) -> bool {
    !has_older_rows || last_timestamp.is_some_and(|timestamp| timestamp < cutoff)
}

fn has_comparable_clock(
    row: &RequestEntry,
    native_requests: &HashMap<(&str, &str, u32), &Observation>,
) -> bool {
    let expected = match row.reason.as_str() {
        "codex_approval_request" => NativeRequestKind::Approval,
        "codex_user_input_request" => NativeRequestKind::UserInput,
        _ => return false,
    };
    let Some(base) = row
        .event_id
        .strip_suffix(":lifecycle")
        .filter(|base| base.starts_with("codex:"))
    else {
        return false;
    };
    let companion = format!("{base}:native-request");
    native_requests.get(&(row.owner.source.as_str(), companion.as_str(), row.revision))
        .is_some_and(|fact| {
            matches!(&fact.kind, ObservationKind::NativeRequest(request)
                if request.request_kind == expected && request.state == NativeRequestState::Requested)
                && fact.identity.session_id.as_deref() == Some(&row.owner.session)
                && fact.identity.worker_id == row.owner.worker
                && fact.identity.turn_id == row.owner.turn
                && fact.observed_at_unix_ns == row.timestamp
        })
}

#[cfg(target_os = "linux")]
fn notify(claimed: usize, stop: &AtomicBool) -> Delivery {
    use std::{
        io::ErrorKind,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    let body = format!(
        "{claimed} recorded attention request(s). Open Agenturm to inspect and respond in your agent terminal."
    );
    let mut child = match Command::new("notify-send")
        .args(["--app-name=Agenturm", "--", "Agenturm attention", &body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == ErrorKind::NotFound => return Delivery::Unavailable,
        Err(_) => return Delivery::Failed,
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Delivery::Delivered,
            Ok(None) if Instant::now() < deadline && !stop.load(Ordering::Relaxed) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Delivery::Failed;
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn notify(_claimed: usize, _stop: &AtomicBool) -> Delivery {
    Delivery::Unavailable
}

#[cfg(test)]
mod tests {
    use super::covers_clock_window;

    #[test]
    fn a_partial_history_covers_the_fresh_window_only_past_its_boundary() {
        assert!(covers_clock_window(false, None, 100));
        assert!(!covers_clock_window(true, None, 100));
        assert!(covers_clock_window(true, Some(99), 100));
        assert!(!covers_clock_window(true, Some(100), 100));
        assert!(!covers_clock_window(true, Some(101), 100));
    }
}
