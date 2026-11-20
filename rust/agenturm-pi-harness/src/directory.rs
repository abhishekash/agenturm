//! Pi opts into the shared bounded directory checkpoint engine.

use crate::{PiFormat, content};

impl<const CONTENT: bool> agenturm_directory_source::DirectoryFormat for PiFormat<CONTENT> {
    const DISCOVERY_GAP_EVENT_PREFIX: &'static str = "pi-discovery-gap";
    const CONTENT_REF_FEATURE: Option<&'static str> = if CONTENT {
        Some("pi-content-ref/1")
    } else {
        None
    };
    const EXTRA_FEATURES: &'static [&'static str] = if CONTENT { &["harness.pi/1"] } else { &[] };

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
        }
    }
}

pub(crate) type DirectoryPoller = agenturm_directory_source::DirectoryPoller<PiFormat<false>>;
pub(crate) type ContentDirectoryPoller = agenturm_directory_source::DirectoryPoller<PiFormat<true>>;
