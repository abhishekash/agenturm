//! Resolve local session files into the same explicit config used by collect.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) const MAX_SESSIONS_PER_HARNESS: usize = super::MAX_CONFIGURED_SOURCES / 2;
pub(crate) const DEFAULT_SESSIONS_PER_HARNESS: usize = 4;
pub(crate) const PI_PLUGIN_ID: &str = "example.rust-pi-harness";
const MAX_DISCOVERY_ENTRIES: usize = 100_000;
const MAX_DISCOVERY_DEPTH: usize = 8;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FollowDiscovery {
    pub(crate) codex_root: PathBuf,
    pub(crate) pi_root: PathBuf,
    pub(crate) sessions_per_harness: usize,
}

impl FollowDiscovery {
    pub(crate) fn roots(&self) -> [&Path; 2] {
        [&self.codex_root, &self.pi_root]
    }

    pub(crate) fn sources(&self) -> Result<Vec<super::CollectionSourceInput>, String> {
        let mut sources = Vec::new();
        for (kind, plugin, root) in [
            ("codex", agenturm_runtime::CODEX_PLUGIN_ID, &self.codex_root),
            ("pi", PI_PLUGIN_ID, &self.pi_root),
        ] {
            let files = recent_session_files(root, self.sessions_per_harness)
                .map_err(|error| format!("scan {kind} sessions failed: {error}"))?;
            for file in files {
                sources.push(source_entry(kind, plugin, &file)?);
            }
        }
        Ok(sources)
    }
}

pub fn execute_discover(sessions_per_harness: usize) -> Result<String, String> {
    discover(sessions_per_harness, false)
}

pub fn discover(sessions_per_harness: usize, follow: bool) -> Result<String, String> {
    if !(1..=MAX_SESSIONS_PER_HARNESS).contains(&sessions_per_harness) {
        return Err(format!(
            "discovery requires 1 to {MAX_SESSIONS_PER_HARNESS} sessions per harness"
        ));
    }

    let executable = env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|error| format!("resolve Agenturm executable failed: {error}"))?;
    let bin = executable
        .parent()
        .ok_or("Agenturm executable has no directory")?;
    let plugin_host = bin.join("agenturm-plugin-host");
    let catalog = bin
        .parent()
        .ok_or("Agenturm executable has no bundle directory")?
        .join("agenturm-plugins.json");
    if !plugin_host.is_file() || !catalog.is_file() {
        return Err("discovery requires the bundled plugin host and agenturm-plugins.json".into());
    }

    let selection = FollowDiscovery {
        codex_root: session_root("CODEX_HOME", ".codex")?,
        pi_root: session_root("PI_CODING_AGENT_DIR", ".pi/agent")?,
        sessions_per_harness,
    };
    let mut config = json!({
        "version": 2,
        "plugin_host": config_path(&plugin_host)?,
        "plugin_catalog": config_path(&catalog)?,
    });
    if follow {
        config_path(&selection.codex_root)?;
        config_path(&selection.pi_root)?;
        config["follow"] = json!(selection);
    } else {
        let sources = selection.sources()?;
        if sources.is_empty() {
            return Err("no Codex or Pi session files found".into());
        }
        config["sources"] = json!(sources);
    }
    serde_json::to_string_pretty(&config)
        .map_err(|error| format!("write collection config failed: {error}"))
}

fn session_root(variable: &str, default_home: &str) -> Result<PathBuf, String> {
    let base = if let Some(value) = env::var_os(variable).filter(|value| !value.is_empty()) {
        PathBuf::from(value)
    } else {
        let home = env::var_os("HOME").ok_or_else(|| format!("set HOME or {variable}"))?;
        PathBuf::from(home).join(default_home)
    };
    if !base.is_absolute() {
        return Err(format!("{variable} or HOME must be an absolute path"));
    }
    Ok(base.join("sessions"))
}

fn recent_session_files(root: &Path, limit: usize) -> Result<Vec<PathBuf>, String> {
    let root = match fs::canonicalize(root) {
        Ok(root) => root,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut pending = vec![(root, 0)];
    let mut recent = BTreeSet::<(SystemTime, PathBuf)>::new();
    let mut entries = 0;
    while let Some((directory, depth)) = pending.pop() {
        for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
            entries += 1;
            if entries > MAX_DISCOVERY_ENTRIES {
                return Err("session discovery entry limit exceeded".into());
            }
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            let path = entry.path();
            if kind.is_dir() && depth < MAX_DISCOVERY_DEPTH {
                pending.push((path, depth + 1));
            } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
                let modified = match entry.metadata().and_then(|meta| meta.modified()) {
                    Ok(modified) => modified,
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.to_string()),
                };
                recent.insert((modified, path));
                if recent.len() > limit {
                    recent.pop_first();
                }
            }
        }
    }
    Ok(recent.into_iter().rev().map(|(_, path)| path).collect())
}

fn source_entry(
    name: &str,
    plugin_id: &str,
    file: &Path,
) -> Result<super::CollectionSourceInput, String> {
    let path = config_path(file)?;
    let digest = Sha256::digest(path.as_bytes());
    let suffix = u128::from_be_bytes(digest[..16].try_into().expect("16-byte digest prefix"));
    Ok(super::CollectionSourceInput {
        plugin_id: plugin_id.into(),
        adapter: None,
        source_root: file.to_path_buf(),
        source_id: format!("harness.{name}.{suffix:032x}"),
    })
}

fn config_path(path: &Path) -> Result<&str, String> {
    path.to_str()
        .filter(|path| path.len() <= 4_096)
        .ok_or_else(|| "bundle and session paths must be UTF-8 and at most 4096 bytes".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, FileTimes};
    use std::time::Duration;

    #[test]
    fn discovery_selects_the_latest_nested_jsonl_files_with_stable_source_ids() {
        let directory = env::temp_dir().join(format!("agenturm-discover-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let root = directory.join("sessions");
        let nested = root.join("2026/11");
        fs::create_dir_all(&nested).unwrap();
        for (file, seconds) in [
            (root.join("older.jsonl"), 1),
            (nested.join("recent.jsonl"), 2),
            (root.join("newer.jsonl"), 3),
            (root.join("ignored.txt"), 4),
        ] {
            File::create(file)
                .unwrap()
                .set_times(
                    FileTimes::new()
                        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
                )
                .unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("newer.jsonl"), root.join("link.jsonl")).unwrap();

        let result = recent_session_files(&root, 2);
        let missing = recent_session_files(&root.join("missing"), 2);
        fs::remove_dir_all(&directory).unwrap();

        let result = result.unwrap();
        assert!(missing.unwrap().is_empty());
        assert_eq!(
            result
                .iter()
                .map(|path| path.file_name().unwrap())
                .collect::<Vec<_>>(),
            ["newer.jsonl", "recent.jsonl"]
        );
        let first = source_entry("codex", "example.rust-codex-harness", &result[0]).unwrap();
        let second = source_entry("codex", "example.rust-codex-harness", &result[1]).unwrap();
        assert_ne!(first.source_id, second.source_id);
        assert_eq!(
            source_entry("codex", "example.rust-codex-harness", &result[0])
                .unwrap()
                .source_id,
            first.source_id
        );
        assert!(agenturm_kernel::ExtensionId::parse(&first.source_id).is_ok());
    }
}
