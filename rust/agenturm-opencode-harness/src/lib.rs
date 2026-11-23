//! Bounded OpenCode SQLite observation and private-reference readers.

#![forbid(unsafe_code)]

mod sqlite;

pub use sqlite::{PromptSourceStamp, UserPromptEvidence, poll, poll_legacy, prove_user_prompts};
