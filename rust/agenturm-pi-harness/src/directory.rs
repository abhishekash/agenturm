//! Pi opts into the shared bounded directory checkpoint engine.

use crate::{PiFormat, content};

impl<const CONTENT: bool, const COMMAND: bool> agenturm_directory_source::DirectoryFormat
    for PiFormat<CONTENT, COMMAND>
{
    const DISCOVERY_GAP_EVENT_PREFIX: &'static str = "pi-discovery-gap";
    const REFERENCE_FEATURE: Option<&'static str> = if CONTENT {
        Some("pi-content-ref/1")
    } else if COMMAND {
        Some("pi-command-ref/1")
    } else {
        None
    };
    const REFERENCE_ENVELOPE_KEY: &'static str = if COMMAND {
        "command_refs"
    } else {
        "content_refs"
    };
    const EXTRA_FEATURES: &'static [&'static str] = if CONTENT || COMMAND {
        &["harness.pi/1"]
    } else {
        &[]
    };

    fn decorate_member(
        root: &std::path::Path,
        path: &std::path::Path,
        member: &str,
        prior_cursor: &str,
        local: &serde_json::Value,
        batch: &mut serde_json::Value,
        max_bytes: usize,
    ) {
        if CONTENT {
            content::decorate(root, path, member, prior_cursor, local, batch, max_bytes);
        } else if COMMAND {
            content::decorate_command(root, path, member, prior_cursor, local, batch, max_bytes);
        }
    }
}

pub(crate) type DirectoryPoller = agenturm_directory_source::DirectoryPoller<PiFormat<false>>;
pub(crate) type ContentDirectoryPoller = agenturm_directory_source::DirectoryPoller<PiFormat<true>>;
pub(crate) type CommandDirectoryPoller =
    agenturm_directory_source::DirectoryPoller<PiFormat<false, true>>;
