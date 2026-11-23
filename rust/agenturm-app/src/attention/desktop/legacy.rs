//! Desktop delivery against an explicitly selected Python 0.2.8 receiver.

use super::{Delivery, FRESHNESS_NS, notify};
use crate::remote_history::legacy::{
    AttentionReport, DesktopAttentionReader, DesktopAttentionSample, PendingRequest,
};
use crate::{RemoteHistoryOptions, broker_cli, cockpit_startup};
use agenturm_store::{LegacyDesktopAttentionRequest, MAX_WORKSPACE_PAGE_SIZE, WorkspaceLedger};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, Eq, PartialEq, Serialize)]
struct Report {
    #[serde(flatten)]
    attention: AttentionReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    desktop_delivery: Option<Delivery>,
}

pub(in crate::attention) fn watch_legacy(
    options: &RemoteHistoryOptions,
    interval: Duration,
    limit: usize,
    output: impl Write,
) -> Result<(), String> {
    let mut reader = DesktopAttentionReader::connect(options)?;
    let endpoint = options.endpoint();
    let identity = reader.identity().to_owned();
    let state = cockpit_startup::state_directory(
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?;
    let namespace = format!(
        "{:x}",
        Sha256::digest(format!(
            "agenturm-python-desktop-attention/1\n{endpoint}\n{identity}\n"
        ))
    );
    let marker = broker_cli::private_workspace_marker(
        &state.join("python-desktop-attention").join(&namespace),
        &format!("agenturm-python-desktop-attention/1\n{namespace}\n"),
    )?;
    let ledger = WorkspaceLedger::open(marker).map_err(|error| error.to_string())?;
    let mut ready = false;
    super::super::watch(interval, limit, output, |stop| {
        let sample = reader.read()?;
        ready = sample.report.ready;
        let delivery = deliver(&sample, &mut reader, &ledger, endpoint, &identity, stop)?;
        Ok(Report {
            attention: sample.report,
            desktop_delivery: delivery,
        })
    })?;
    if limit == 1 && !ready {
        return Err("Python receiver request inventory is incomplete".into());
    }
    Ok(())
}

fn deliver(
    sample: &DesktopAttentionSample,
    reader: &mut DesktopAttentionReader,
    ledger: &WorkspaceLedger,
    endpoint: SocketAddr,
    identity: &str,
    stop: &AtomicBool,
) -> Result<Option<Delivery>, String> {
    if stop.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let (clock, requests) = match complete_sample(sample) {
        Ok(proof) => proof,
        Err(deferred) => return Ok(Some(deferred)),
    };
    let cutoff = clock.saturating_sub(FRESHNESS_NS);
    let mut uncertain_clock = false;
    let mut candidates = Vec::new();
    for request in requests {
        match request.receiver_clock() {
            Some(time) if (cutoff..=clock).contains(&time) => {
                candidates.push(LegacyDesktopAttentionRequest {
                    event_key: request.desktop_key(),
                    observed_at_unix_ns: time,
                })
            }
            Some(time) if time <= clock => {}
            _ => uncertain_clock = true,
        }
    }
    if candidates.len() > MAX_WORKSPACE_PAGE_SIZE {
        return Ok(Some(Delivery::DeferredIncomplete));
    }
    let verified = reader.read()?;
    let clock = match same_current_requests(&verified, requests, clock, &candidates) {
        Ok(clock) => clock,
        Err(deferred) => return Ok(Some(deferred)),
    };
    if stop.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let claimed = ledger
        .claim_legacy_desktop_attention(endpoint, identity, clock, &candidates)
        .map_err(|error| format!("Desktop attention claim failed: {error}"))?;
    if claimed == 0 {
        return Ok(uncertain_clock.then_some(Delivery::DeferredSourceClock));
    }
    let verified = reader.read()?;
    if let Err(deferred) = same_current_requests(&verified, requests, clock, &candidates) {
        return Ok(Some(deferred));
    }
    Ok((!stop.load(Ordering::Relaxed)).then(|| notify(claimed, stop)))
}

fn complete_sample(sample: &DesktopAttentionSample) -> Result<(u64, &[PendingRequest]), Delivery> {
    let requests = sample
        .report
        .desktop_requests()
        .ok_or(Delivery::DeferredIncomplete)?;
    let clock = sample.clock.ok_or(Delivery::DeferredSourceClock)?;
    Ok((clock, requests))
}

fn same_current_requests(
    sample: &DesktopAttentionSample,
    requests: &[PendingRequest],
    minimum_clock: u64,
    candidates: &[LegacyDesktopAttentionRequest],
) -> Result<u64, Delivery> {
    let (clock, current) = complete_sample(sample)?;
    if clock < minimum_clock {
        return Err(Delivery::DeferredSourceClock);
    }
    if current != requests {
        return Err(Delivery::DeferredHistoryChanged);
    }
    let cutoff = clock.saturating_sub(FRESHNESS_NS);
    if candidates
        .iter()
        .any(|row| row.observed_at_unix_ns < cutoff)
    {
        return Err(Delivery::DeferredSourceClock);
    }
    Ok(clock)
}
