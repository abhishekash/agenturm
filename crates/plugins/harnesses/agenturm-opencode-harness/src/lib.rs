//! Bounded OpenCode SQLite observation and private-reference readers.

#![forbid(unsafe_code)]

mod sqlite;

pub use sqlite::{
    PatchPartEvidence, PromptSourceStamp, ProvenPatchPart, UserPromptEvidence, poll, poll_legacy,
    prove_patch_part, prove_user_prompts,
};
