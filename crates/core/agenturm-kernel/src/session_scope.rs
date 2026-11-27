//! Neutral session time scope shared by every surface that lists sessions.
//!
//! The reference cockpit separates *Today* (current work and today's reported
//! outcomes) from *History* (everything retained). That policy is presentation
//! policy over recorded facts, not a storage rule: the store keeps every row in
//! both scopes. Keeping the boundary here, free of provider or UI vocabulary,
//! lets the roster, the session chooser and any future evidence query consume
//! one deterministic rule.
//!
//! Two things stay explicit:
//!
//! * The calendar boundary is the local day. Callers resolve the local UTC
//!   offset for the exact instant (including a daylight-saving transition) and
//!   pass the resulting day start. [`local_day_start_unix_ns`] is the
//!   arithmetic helper for a caller with a fixed offset.
//! * A recorded lifecycle state never proves that an operating-system process
//!   is alive now. `Today` therefore describes *recorded current work*; it is
//!   not a liveness claim.

/// Where a session belongs in the roster's time dimension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionTimeScope {
    /// Recorded current work, pending input of any age and the current local
    /// day's reported outcomes.
    Today,
    /// Every retained session, including reviewed and older outcomes.
    History,
}

/// Recorded facts the time scope reads. No field implies current liveness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionScopeFacts<'a> {
    /// Latest recorded lifecycle state, exactly as stored.
    pub state: Option<&'a str>,
    /// Last recorded observation time for the session.
    pub updated_at_unix_ns: u64,
    /// A recorded request is still open (`seen == false`).
    pub pending_input: bool,
    /// The exact recorded review version is acknowledged.
    pub reviewed: bool,
}

/// States the reference treats as current work of any age.
pub const ACTIVE_STATES: &[&str] = &["running", "started", "restored"];

/// States the reference reports as an attention outcome.
pub const ATTENTION_STATES: &[&str] = &["waiting", "blocked", "failed", "interrupted"];

/// Outcome states that only appear in Today for the current local day.
pub const OUTCOME_STATES: &[&str] = &["completed", "ended"];

/// Returns whether `facts` belongs to `scope`.
///
/// `day_start_unix_ns` is local midnight in Unix nanoseconds. The rule mirrors
/// the tagged Python cockpit:
///
/// * current work of any age is in Today;
/// * pending input of any age is in Today;
/// * a pending input that was already acknowledged drops out of Today;
/// * a completed or failed outcome is in Today only when it was recorded since
///   local midnight and its review version is not acknowledged;
/// * an unknown state is in Today only when it was recorded since midnight and
///   has not been acknowledged;
/// * History accepts every retained row.
///
/// A waiting or blocked session without an open request follows the same
/// timestamp rule rather than claiming that an old wait is current.
pub fn in_session_time_scope(
    scope: SessionTimeScope,
    facts: SessionScopeFacts<'_>,
    day_start_unix_ns: u64,
) -> bool {
    match scope {
        SessionTimeScope::History => true,
        SessionTimeScope::Today => {
            let state = facts.state;
            if state.is_some_and(|state| ACTIVE_STATES.contains(&state)) {
                return true;
            }
            if facts.pending_input {
                return !facts.reviewed;
            }
            if facts.reviewed {
                return false;
            }
            facts.updated_at_unix_ns >= day_start_unix_ns
        }
    }
}

/// Local calendar-day start for a caller that already resolved the UTC offset
/// at `now_ns`. Rounded to whole seconds like the reference's `datetime`
/// replacement, and saturating rather than wrapping for pre-epoch clocks.
pub fn local_day_start_unix_ns(now_ns: u64, utc_offset_seconds: i32) -> u64 {
    let now_seconds = i64::try_from(now_ns / 1_000_000_000).unwrap_or(i64::MAX);
    let offset = i64::from(utc_offset_seconds);
    let local = now_seconds.saturating_add(offset);
    let day = local.div_euclid(86_400);
    let start_utc = day.saturating_mul(86_400).saturating_sub(offset).max(0);
    u64::try_from(start_utc)
        .unwrap_or(0)
        .saturating_mul(1_000_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOON_UTC: u64 = 1_766_491_200 * 1_000_000_000; // 2025-12-23 12:00:00Z
    const UTC_MIDNIGHT: u64 = 1_766_448_000 * 1_000_000_000; // 2025-12-23 00:00:00Z
    const HOUR: u64 = 3_600 * 1_000_000_000;

    fn facts<'a>(
        state: &'a str,
        updated: u64,
        pending: bool,
        reviewed: bool,
    ) -> SessionScopeFacts<'a> {
        SessionScopeFacts {
            state: Some(state),
            updated_at_unix_ns: updated,
            pending_input: pending,
            reviewed,
        }
    }

    #[test]
    fn local_day_start_follows_the_supplied_offset_across_midnight() {
        assert_eq!(local_day_start_unix_ns(NOON_UTC, 0), UTC_MIDNIGHT);
        // Local midnight is earlier in UTC for an eastern offset and later for
        // a western one.
        assert_eq!(
            local_day_start_unix_ns(NOON_UTC, 3_600),
            UTC_MIDNIGHT - HOUR
        );
        assert_eq!(
            local_day_start_unix_ns(NOON_UTC, -5 * 3_600),
            UTC_MIDNIGHT + 5 * HOUR
        );

        // 00:30 UTC is already 24 December in Berlin (+01:00) but still
        // 23 December in New York (-05:00). A fixed UTC-day rule would put
        // both rows in the same bucket; the local rule does not.
        let after_utc_midnight = UTC_MIDNIGHT + 24 * HOUR + HOUR / 2;
        let berlin_start = local_day_start_unix_ns(after_utc_midnight, 3_600);
        let new_york_start = local_day_start_unix_ns(after_utc_midnight, -5 * 3_600);
        assert_eq!(berlin_start, UTC_MIDNIGHT + 24 * HOUR - HOUR);
        assert_eq!(new_york_start, UTC_MIDNIGHT + 5 * HOUR);
        // 22:30 UTC on 23 December is already 23 December locally in New York
        // (17:30) but crosses the Berlin midnight into 24 December (23:30 is
        // still the 23rd; midnight is 23:00 UTC), so the same recorded outcome
        // is History in Berlin and Today in New York.
        let late_utc = UTC_MIDNIGHT + 22 * HOUR + HOUR / 2;
        assert!(!in_session_time_scope(
            SessionTimeScope::Today,
            facts("completed", late_utc, false, false),
            berlin_start
        ));
        assert!(in_session_time_scope(
            SessionTimeScope::Today,
            facts("completed", late_utc, false, false),
            new_york_start
        ));
    }

    #[test]
    fn today_keeps_active_work_and_pending_input_of_any_age() {
        let start = local_day_start_unix_ns(NOON_UTC, 0);
        let yesterday = start.saturating_sub(86_400 * 1_000_000_000);
        for state in ACTIVE_STATES {
            assert!(
                in_session_time_scope(
                    SessionTimeScope::Today,
                    facts(state, yesterday, false, false),
                    start
                ),
                "{state} should stay in Today"
            );
        }
        assert!(in_session_time_scope(
            SessionTimeScope::Today,
            facts("waiting", yesterday, true, false),
            start
        ));
        // An acknowledged pending request is no longer current input.
        assert!(!in_session_time_scope(
            SessionTimeScope::Today,
            facts("waiting", yesterday, true, true),
            start
        ));
    }

    #[test]
    fn outcomes_use_the_local_day_boundary_and_review_state() {
        let start = local_day_start_unix_ns(NOON_UTC, 0);
        let today = start + 1_000;
        let yesterday = start.saturating_sub(1);
        assert!(in_session_time_scope(
            SessionTimeScope::Today,
            facts("completed", today, false, false),
            start
        ));
        assert!(!in_session_time_scope(
            SessionTimeScope::Today,
            facts("completed", yesterday, false, false),
            start
        ));
        assert!(!in_session_time_scope(
            SessionTimeScope::Today,
            facts("failed", today, false, true),
            start
        ));
        assert!(!in_session_time_scope(
            SessionTimeScope::Today,
            facts("interrupted", yesterday, false, false),
            start
        ));
        // An unreviewed older failure is exactly the History case the plan
        // calls out; History accepts it.
        assert!(in_session_time_scope(
            SessionTimeScope::History,
            facts("interrupted", yesterday, false, false),
            start
        ));
        // Unknown work recorded today is visible; unknown work from before
        // midnight is not presented as current.
        assert!(in_session_time_scope(
            SessionTimeScope::Today,
            facts("unrecognised", today, false, false),
            start
        ));
        assert!(!in_session_time_scope(
            SessionTimeScope::Today,
            facts("unrecognised", yesterday, false, false),
            start
        ));
    }
}
