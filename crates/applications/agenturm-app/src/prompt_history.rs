//! Bounded prompt-history pages shared by the inspector navigator.
//!
//! The content adapter returns one newest-first, snapshot-pinned window of at
//! most [`agenturm_store::MAX_PROMPT_REFERENCES`] references. This module turns
//! that window into deterministic older pages without re-querying or widening
//! scope, and reports the retained cap so the UI never claims a complete
//! transcript. Grant proof remains in `agenturm-content`; no reference here is
//! readable until `content_preview::reveal_user_prompt` rechecks the grant.

use crate::content_preview::PromptReference;
use agenturm_kernel::ExtensionId;
use agenturm_store::Store;
use std::fmt;
use std::path::Path;

pub(crate) const DEFAULT_PROMPT_PAGE: usize = 100;

/// Keyset cursor for one captured request. `event_id` breaks equal timestamps,
/// matching the adapter's newest-first ordering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PromptCursor {
    pub(crate) observed_at_unix_ns: u64,
    pub(crate) event_id: String,
}

impl PromptCursor {
    pub(crate) fn of(reference: &PromptReference) -> Self {
        Self {
            observed_at_unix_ns: reference.observed_at_unix_ns,
            event_id: reference.event_id.clone(),
        }
    }

    fn older_than(&self, reference: &PromptReference) -> bool {
        reference.observed_at_unix_ns < self.observed_at_unix_ns
            || (reference.observed_at_unix_ns == self.observed_at_unix_ns
                && reference.event_id < self.event_id)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PromptPage {
    /// One page in the adapter's newest-first order.
    pub(crate) requests: Vec<PromptReference>,
    /// References retained in the adapter window, not in the whole session.
    pub(crate) retained: usize,
    pub(crate) retained_cap: usize,
    /// The continued window reached the adapter cap; older history is Activity's.
    pub(crate) limited: bool,
    /// Cursor for the next older page when one exists inside the window.
    pub(crate) older: Option<PromptCursor>,
}

#[cfg(test)]
impl PromptPage {
    pub(crate) fn earlier_available(&self) -> bool {
        self.older.is_some()
    }

    pub(crate) fn page_meta(&self) -> (usize, usize, bool, bool) {
        (
            self.requests.len(),
            self.retained_cap,
            self.earlier_available(),
            self.limited,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PromptHistoryError {
    MissingIdentity,
    /// The source changed or its capture could not be proven. The reason is
    /// shown verbatim; a missing approval has its own recoverable state.
    Denied(String),
    ApprovalRequired(String),
    /// The capability is not present in this build or mode.
    Unavailable(String),
}

impl fmt::Display for PromptHistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingIdentity => {
                formatter.write_str("Prompt history needs a session or agent ID")
            }
            Self::Denied(reason) | Self::ApprovalRequired(reason) => formatter.write_str(reason),
            Self::Unavailable(reason) => formatter.write_str(reason),
        }
    }
}

/// Windows the adapter's newest-first bounded references into one older page.
///
/// This is presentation paging over an already retained and proof-checked
/// window: it never broadens scope, never invents a reference and never claims
/// rows beyond the adapter cap.
pub(crate) fn window(
    references: Vec<PromptReference>,
    before: Option<&PromptCursor>,
    page: usize,
    retained_cap: usize,
) -> PromptPage {
    let page = page.max(1);
    let mut references = references;
    references.sort_by(|left, right| {
        right
            .observed_at_unix_ns
            .cmp(&left.observed_at_unix_ns)
            .then_with(|| right.event_id.cmp(&left.event_id))
            .then_with(|| right.revision.cmp(&left.revision))
    });
    references
        .dedup_by(|left, right| left.event_id == right.event_id && left.revision == right.revision);
    let retained = references.len();
    let limited = retained >= retained_cap;
    let start = before.map_or(0, |cursor| {
        references
            .iter()
            .position(|reference| cursor.older_than(reference))
            .unwrap_or(retained)
    });
    let end = (start + page).min(retained);
    let requests: Vec<_> = references[start..end].to_vec();
    let older = requests
        .last()
        .filter(|_| end < retained)
        .map(PromptCursor::of);
    PromptPage {
        requests,
        retained,
        retained_cap,
        limited,
        older,
    }
}

/// Loads one prompt-history page under the current grant.
pub(crate) fn scoped_window(
    references: Vec<PromptReference>,
    turn: Option<&str>,
    before: Option<&PromptCursor>,
    page: usize,
) -> PromptPage {
    let limited = references.len() >= agenturm_store::MAX_PROMPT_REFERENCES;
    let references = references
        .into_iter()
        .filter(|reference| turn.is_none_or(|turn| reference.turn_id == turn))
        .collect();
    let mut result = window(
        references,
        before,
        page,
        agenturm_store::MAX_PROMPT_REFERENCES,
    );
    result.limited = limited;
    result
}

/// Loads one prompt-history page under the current grant.
pub(crate) struct PromptReadScope<'a> {
    pub(crate) source: &'a ExtensionId,
    pub(crate) session: &'a str,
    pub(crate) worker: Option<&'a str>,
    pub(crate) turn: Option<&'a str>,
}

/// Loads one prompt-history page under the current grant.
pub(crate) fn load_page(
    db: &Path,
    config: &(impl crate::content_preview::ContentSourceConfig + ?Sized),
    scope: PromptReadScope<'_>,
    before: Option<&PromptCursor>,
    page: usize,
    writer: Option<&Store>,
) -> Result<PromptPage, PromptHistoryError> {
    let PromptReadScope {
        source,
        session,
        worker,
        turn,
    } = scope;
    if session.trim().is_empty() {
        return Err(PromptHistoryError::MissingIdentity);
    }
    #[cfg(target_os = "linux")]
    {
        let references = match crate::content_preview::list_user_prompts_with_access(
            db, config, source, session, worker, writer,
        )
        .map_err(PromptHistoryError::Denied)?
        {
            agenturm_content::PromptListing::Granted(references) => references,
            agenturm_content::PromptListing::ApprovalRequired(reason) => {
                return Err(PromptHistoryError::ApprovalRequired(reason));
            }
        };
        Ok(scoped_window(references, turn, before, page))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (
            db, config, source, session, worker, turn, before, page, writer,
        );
        Err(PromptHistoryError::Unavailable(
            "Prompt history is available on Linux".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(index: usize, timestamp: u64) -> PromptReference {
        PromptReference {
            event_id: format!("event-{index:03}"),
            revision: 0,
            turn_id: format!("turn-{index}"),
            observed_at_unix_ns: timestamp,
            snapshot_id: Some(9.into()),
            history_identity: Some("history".into()),
        }
    }

    #[test]
    fn pages_are_deterministic_newest_first_and_keep_exact_identity() {
        // Adapter order is newest first; timestamps include a tie.
        let references = vec![
            reference(6, 600),
            reference(5, 500),
            reference(4, 400),
            reference(3, 400),
            reference(2, 200),
            reference(1, 100),
        ];
        let first = window(references.clone(), None, 2, 256);
        assert_eq!(
            first
                .requests
                .iter()
                .map(|row| row.event_id.as_str())
                .collect::<Vec<_>>(),
            ["event-006", "event-005"]
        );
        assert!(!first.limited);
        assert!(first.earlier_available());
        let cursor = first.older.clone().unwrap();

        let second = window(references.clone(), Some(&cursor), 2, 256);
        assert_eq!(
            second
                .requests
                .iter()
                .map(|row| row.event_id.as_str())
                .collect::<Vec<_>>(),
            ["event-004", "event-003"]
        );
        let cursor = second.older.clone().unwrap();
        let third = window(references.clone(), Some(&cursor), 2, 256);
        assert_eq!(
            third
                .requests
                .iter()
                .map(|row| row.event_id.as_str())
                .collect::<Vec<_>>(),
            ["event-002", "event-001"]
        );
        assert!(!third.earlier_available());
    }

    #[test]
    fn the_adapter_cap_is_disclosed_and_never_presented_as_a_complete_transcript() {
        let references = (0..256)
            .map(|index| reference(index, u64::try_from(index).unwrap() + 1))
            .collect();
        let page = window(references, None, 100, 256);
        assert!(page.limited);
        assert_eq!(page.retained, 256);
        assert_eq!(page.retained_cap, 256);
        assert_eq!(page.requests.len(), 100);
        assert!(page.earlier_available());
        assert_eq!(page.page_meta(), (100, 256, true, true));
    }

    #[test]
    fn an_empty_exact_scope_is_distinct_from_a_capped_window() {
        let page = window(Vec::new(), None, 100, 256);
        assert!(page.requests.is_empty());
        assert_eq!(page.retained, 0);
        assert!(!page.limited);
        assert!(!page.earlier_available());
    }

    #[test]
    fn duplicates_and_a_missing_cursor_do_not_shift_identity() {
        let references = vec![
            reference(3, 300),
            reference(3, 300),
            reference(2, 200),
            reference(1, 100),
        ];
        let page = window(references, None, 10, 256);
        assert_eq!(page.requests.len(), 3);
        assert!(!page.earlier_available());
        let unknown = PromptCursor {
            observed_at_unix_ns: 250,
            event_id: "event-999".into(),
        };
        let page = window(
            vec![reference(2, 200), reference(1, 100)],
            Some(&unknown),
            10,
            256,
        );
        assert_eq!(page.requests.len(), 2);
        assert!(!page.earlier_available());
    }

    #[test]
    fn missing_identity_is_reported_before_any_adapter_read() {
        let error = load_page(
            Path::new("/nonexistent"),
            Path::new("/nonexistent"),
            PromptReadScope {
                source: &ExtensionId::parse("harness.fixture").unwrap(),
                session: " ",
                worker: None,
                turn: None,
            },
            None,
            DEFAULT_PROMPT_PAGE,
            None,
        )
        .unwrap_err();
        assert_eq!(error, PromptHistoryError::MissingIdentity);
        assert_eq!(
            error.to_string(),
            "Prompt history needs a session or agent ID"
        );
    }
}
