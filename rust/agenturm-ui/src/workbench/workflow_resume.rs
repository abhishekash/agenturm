//! A reviewed native resume command for an exact recorded workflow request.

use super::RequestEntry;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResumeRuntime {
    Codex,
    Claude,
}

pub struct WorkflowResume {
    pub request: RequestEntry,
    pub runtime: ResumeRuntime,
}

impl WorkflowResume {
    pub(super) fn command(&self) -> Option<String> {
        let session = &self.request.owner.session;
        if self.request.owner.worker.is_some()
            || session.len() > 200
            || !session
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !session
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            return None;
        }
        let (source, command) = match self.runtime {
            ResumeRuntime::Codex => ("harness.codex", "codex resume"),
            ResumeRuntime::Claude => ("harness.claude", "claude --resume"),
        };
        let owner = &self.request.owner.source;
        if owner != source
            && !owner
                .strip_prefix(source)
                .is_some_and(|rest| rest.starts_with('.'))
        {
            return None;
        }
        Some(format!("{command} {session}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RequestProof, WorkAnchor};

    #[test]
    fn commands_are_fixed_to_the_verified_native_family_and_safe_session_identifier() {
        let resume = |source: &str, session: &str, worker, runtime| {
            WorkflowResume {
                request: RequestEntry {
                    owner: WorkAnchor {
                        source: source.into(),
                        session: session.into(),
                        worker,
                        turn: Some("turn".into()),
                    },
                    event_id: "request".into(),
                    revision: 0,
                    timestamp: 1,
                    reason: "approval".into(),
                    seen: false,
                    proof: RequestProof {
                        history_identity: Some("fixture".into()),
                        snapshot_id: 2.into(),
                    },
                },
                runtime,
            }
            .command()
        };
        assert_eq!(
            resume("harness.claude", "ab-09_cd", None, ResumeRuntime::Claude).as_deref(),
            Some("claude --resume ab-09_cd")
        );
        assert_eq!(
            resume(
                "harness.codex.worktree",
                "scope:id-09",
                None,
                ResumeRuntime::Codex
            )
            .as_deref(),
            Some("codex resume scope:id-09")
        );
        for session in ["", "--help", "1;pwd", "one two", "line\nnext"] {
            assert!(resume("harness.codex", session, None, ResumeRuntime::Codex).is_none());
        }
        assert!(resume("harness.codexcopy", "session", None, ResumeRuntime::Codex).is_none());
        assert!(resume("harness.codex", "session", None, ResumeRuntime::Claude).is_none());
        assert!(
            resume(
                "harness.codex",
                "session",
                Some("worker".into()),
                ResumeRuntime::Codex
            )
            .is_none()
        );
    }
}
