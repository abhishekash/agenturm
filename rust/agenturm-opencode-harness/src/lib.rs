//! Bounded OpenCode SQLite observation and private-reference readers.

#![forbid(unsafe_code)]

mod sqlite;

pub use sqlite::{PromptSourceStamp, UserPromptEvidence, poll, prove_user_prompts};
