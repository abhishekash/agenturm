//! Pi opts into the shared bounded directory checkpoint engine.

use crate::PiFormat;

impl agenturm_directory_source::DirectoryFormat for PiFormat {
    const DISCOVERY_GAP_EVENT_PREFIX: &'static str = "pi-discovery-gap";
}

pub(crate) type DirectoryPoller = agenturm_directory_source::DirectoryPoller<PiFormat>;
