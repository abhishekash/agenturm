//! Codex opts into the shared bounded directory checkpoint engine.

use crate::CodexFormat;

impl<const LEGACY: bool> agenturm_directory_source::DirectoryFormat for CodexFormat<LEGACY> {
    const DISCOVERY_GAP_EVENT_PREFIX: &'static str = "codex-discovery-gap";
}

pub(crate) type DirectoryPoller<const LEGACY: bool> =
    agenturm_directory_source::DirectoryPoller<CodexFormat<LEGACY>>;
