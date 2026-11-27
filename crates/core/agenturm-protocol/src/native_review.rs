//! Metadata for explicit native requests and internal Codex review rollouts.

use serde::{Deserialize, Serialize};

/// Internal review rollouts live in their own source namespace.
pub fn is_codex_review_source(source: &str) -> bool {
    source.strip_prefix("review.codex.").is_some_and(sha256)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InternalReview {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    pub state: InternalReviewState,
}

impl InternalReview {
    pub fn is_valid(&self) -> bool {
        native_id(&self.session_id)
            && self
                .parent_session_id
                .as_ref()
                .is_none_or(|parent| native_id(parent) && *parent != self.session_id)
            && self.worker_id.as_deref().is_none_or(native_id)
            && match self.state {
                InternalReviewState::Classified => self.turn_id.is_none(),
                _ => self.turn_id.as_deref().is_some_and(native_id),
            }
    }
}

/// Reports rollout lifecycle. Completion does not assert an approval decision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InternalReviewState {
    Classified,
    Started,
    Completed,
    Interrupted,
}

impl InternalReviewState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Classified => "classified",
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRequest {
    pub request_id: String,
    pub request_kind: NativeRequestKind,
    pub state: NativeRequestState,
}

impl NativeRequest {
    pub fn is_valid(&self) -> bool {
        sha256(&self.request_id)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRequestKind {
    Approval,
    UserInput,
}

impl NativeRequestKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::UserInput => "user_input",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRequestState {
    Requested,
}

impl NativeRequestState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
        }
    }
}

fn native_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_fields_and_state_are_exact() {
        let mut review: InternalReview = serde_json::from_str(
            r#"{"session_id":"reviewer","parent_session_id":"parent","state":"classified"}"#,
        )
        .unwrap();
        assert!(review.is_valid());
        review.state = InternalReviewState::Completed;
        assert!(!review.is_valid());
        review.turn_id = Some("actual-turn".into());
        assert!(review.is_valid());
        review.parent_session_id = Some(review.session_id.clone());
        assert!(!review.is_valid());
        assert!(
            serde_json::from_str::<InternalReview>(
                r#"{"session_id":"reviewer","state":"completed","decision":"approved"}"#,
            )
            .is_err()
        );
        assert!(is_codex_review_source(&format!(
            "review.codex.{}",
            "a".repeat(64)
        )));
        assert!(!is_codex_review_source("review.codex.any"));
    }

    #[test]
    fn requests_carry_only_an_opaque_id_kind_and_observed_state() {
        let value = format!(
            r#"{{"request_id":"{}","request_kind":"approval","state":"requested"}}"#,
            "a".repeat(64),
        );
        assert!(
            serde_json::from_str::<NativeRequest>(&value)
                .unwrap()
                .is_valid()
        );
        assert!(
            serde_json::from_str::<NativeRequest>(&value.replace("requested", "approved")).is_err()
        );
        assert!(
            serde_json::from_str::<NativeRequest>(&value.replace("approval", "arbitrary")).is_err()
        );
        assert!(
            !serde_json::from_str::<NativeRequest>(&value.replace(&"a".repeat(64), "raw-request"))
                .unwrap()
                .is_valid()
        );
    }
}
