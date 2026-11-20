//! Codex opts into the shared bounded directory checkpoint engine.

use crate::CodexFormat;

impl agenturm_directory_source::DirectoryFormat for CodexFormat {
    const DISCOVERY_GAP_EVENT_PREFIX: &'static str = "codex-discovery-gap";
}

pub(crate) type DirectoryPoller = agenturm_directory_source::DirectoryPoller<CodexFormat>;
