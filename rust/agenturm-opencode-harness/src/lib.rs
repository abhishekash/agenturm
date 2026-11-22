//! Bounded OpenCode SQLite observation and private-reference readers.

#![forbid(unsafe_code)]

mod sqlite;

pub use sqlite::{UserPromptEvidence, poll, prove_user_prompts, verify_prompt_checkpoint};
