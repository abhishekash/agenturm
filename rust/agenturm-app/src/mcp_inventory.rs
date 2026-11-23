//! Display-only names from explicitly selected local MCP configuration files.

#[cfg(target_os = "linux")]
mod local;
#[cfg(target_os = "linux")]
mod toml;

use agenturm_ui::McpInventory;

use crate::discovery::SourceAuthority;

pub(super) fn configured(authority: Option<&SourceAuthority>) -> Option<McpInventory> {
    #[cfg(target_os = "linux")]
    return local::configured(authority);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = authority;
        None
    }
}

#[cfg(target_os = "linux")]
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}
