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
pub(crate) const CLAUDE_PLUGIN_ID: &str = "example.rust-claude-harness";
pub(crate) const KIRO_PLUGIN_ID: &str = "example.rust-kiro-harness";
pub(crate) const OPENCODE_PLUGIN_ID: &str = "example.rust-opencode-harness";
const MAX_DISCOVERY_ENTRIES: usize = 100_000;
const MAX_DISCOVERY_DEPTH: usize = 8;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FollowDiscovery {
    pub(crate) codex_root: PathBuf,
    pub(crate) pi_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claude_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kiro_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opencode_db: Option<PathBuf>,
    pub(crate) sessions_per_harness: usize,
}

impl FollowDiscovery {
    pub(crate) fn roots(&self) -> impl Iterator<Item = &Path> {
        self.targets().map(|(_, _, path, _)| path)
    }

    pub(crate) fn resolve_paths(&mut self, resolve: impl Fn(PathBuf) -> PathBuf) {
        for path in [
            Some(&mut self.codex_root),
            Some(&mut self.pi_root),
            self.claude_root.as_mut(),
            self.kiro_root.as_mut(),
            self.opencode_db.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            *path = resolve(std::mem::take(path));
        }
    }

    pub(crate) fn plugins(&self) -> impl Iterator<Item = &'static str> {
        self.targets().map(|(_, plugin, _, _)| plugin)
    }

    fn targets(&self) -> impl Iterator<Item = (&'static str, &'static str, &Path, SourceShape)> {
        [
            (
                "codex",
                agenturm_runtime::CODEX_PLUGIN_ID,
                Some(self.codex_root.as_path()),
                SourceShape::Transcript,
            ),
            (
                "pi",
                PI_PLUGIN_ID,
                Some(self.pi_root.as_path()),
                SourceShape::Transcript,
            ),
            (
                "claude",
                CLAUDE_PLUGIN_ID,
                self.claude_root.as_deref(),
                SourceShape::Transcript,
            ),
            (
                "kiro",
                KIRO_PLUGIN_ID,
                self.kiro_root.as_deref(),
                SourceShape::Kiro,
            ),
            (
                "opencode",
                OPENCODE_PLUGIN_ID,
                self.opencode_db.as_deref(),
                SourceShape::Database,
            ),
        ]
        .into_iter()
        .filter_map(|(kind, plugin, path, shape)| Some((kind, plugin, path?, shape)))
    }

    pub(crate) fn sources(&self) -> Result<Vec<super::CollectionSourceInput>, String> {
        let mut by_harness = Vec::new();
        for (kind, plugin, root, shape) in self.targets() {
            let paths = match shape {
                SourceShape::Database => regular_database(root),
                SourceShape::Transcript | SourceShape::Kiro => {
                    recent_session_files(root, self.sessions_per_harness, shape)
                }
            }
            .map_err(|error| format!("scan {kind} sessions failed: {error}"))?;
            by_harness.push((kind, plugin, paths.into_iter()));
        }
        // Give each harness a slot before drawing more sessions from a busy
        // harness. The existing per-harness limit still applies.
        let mut sources = Vec::new();
        while sources.len() < super::MAX_CONFIGURED_SOURCES {
            let before = sources.len();
            for (kind, plugin, paths) in &mut by_harness {
                if let Some(path) = paths.next() {
                    sources.push(source_entry(kind, plugin, &path)?);
                    if sources.len() == super::MAX_CONFIGURED_SOURCES {
                        break;
                    }
                }
            }
            if sources.len() == before {
                break;
            }
        }
        Ok(sources)
    }
}

#[derive(Clone, Copy)]
enum SourceShape {
    Transcript,
    Kiro,
    Database,
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

    let installed = super::load_adapter_catalog(&catalog)?;
    let selection = FollowDiscovery {
        codex_root: data_root("CODEX_HOME", ".codex")?.join("sessions"),
        pi_root: data_root("PI_CODING_AGENT_DIR", ".pi/agent")?.join("sessions"),
        claude_root: installed
            .contains_key(CLAUDE_PLUGIN_ID)
            .then(|| data_root("CLAUDE_CONFIG_DIR", ".claude").map(|path| path.join("projects")))
            .transpose()?,
        kiro_root: installed
            .contains_key(KIRO_PLUGIN_ID)
            .then(|| data_root("KIRO_HOME", ".kiro").map(|path| path.join("sessions")))
            .transpose()?,
        opencode_db: installed
            .contains_key(OPENCODE_PLUGIN_ID)
            .then(opencode_db)
            .transpose()?,
        sessions_per_harness,
    };
    let mut config = json!({
        "version": 2,
        "plugin_host": config_path(&plugin_host)?,
        "plugin_catalog": config_path(&catalog)?,
    });
    if follow {
        for root in selection.roots() {
            config_path(root)?;
        }
        config["follow"] = json!(selection);
    } else {
        let sources = selection.sources()?;
        if sources.is_empty() {
            return Err("no supported local harness sessions found".into());
        }
        config["sources"] = json!(sources);
    }
    serde_json::to_string_pretty(&config)
        .map_err(|error| format!("write collection config failed: {error}"))
}

fn data_root(variable: &str, default_home: &str) -> Result<PathBuf, String> {
    let base = if let Some(value) = env::var_os(variable).filter(|value| !value.is_empty()) {
        PathBuf::from(value)
    } else {
        let home = env::var_os("HOME").ok_or_else(|| format!("set HOME or {variable}"))?;
        PathBuf::from(home).join(default_home)
    };
    if !base.is_absolute() {
        return Err(format!("{variable} or HOME must be an absolute path"));
    }
    Ok(base)
}

fn opencode_db() -> Result<PathBuf, String> {
    let path = env::var_os("OPENCODE_DB")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if let Some(absolute) = path.as_ref().filter(|path| path.is_absolute()) {
        return Ok(absolute.clone());
    }
    Ok(data_root("XDG_DATA_HOME", ".local/share")?
        .join("opencode")
        .join(path.unwrap_or_else(|| PathBuf::from("opencode.db"))))
}

fn regular_database(path: &Path) -> Result<Vec<PathBuf>, String> {
    match fs::canonicalize(path) {
        Ok(path) if path.is_file() => Ok(vec![path]),
        Ok(_) => Err("OpenCode database path is not a regular file".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.to_string()),
    }
}

fn recent_session_files(
    root: &Path,
    limit: usize,
    shape: SourceShape,
) -> Result<Vec<PathBuf>, String> {
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
                if !matches!(shape, SourceShape::Kiro)
                    || !matches!(
                        entry.file_name().to_str(),
                        Some("cli" | "snapshots" | ".index")
                    )
                {
                    pending.push((path, depth + 1));
                }
            } else if kind.is_file() && is_transcript(&path, shape) {
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

fn is_transcript(path: &Path, shape: SourceShape) -> bool {
    if path.extension().is_none_or(|ext| ext != "jsonl") {
        return false;
    }
    if !matches!(shape, SourceShape::Kiro) {
        return true;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let session = if path
        .file_name()
        .is_some_and(|name| name == "messages.jsonl")
    {
        Some(parent)
    } else if parent
        .file_name()
        .is_some_and(|name| name == "sub-executions")
    {
        parent.parent()
    } else {
        None
    };
    session
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("sess_"))
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
    fn follow_discovery_budgets_all_harnesses_and_filters_kiro_paths() {
        let directory =
            env::temp_dir().join(format!("agenturm-discover-all-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let make_files = |paths: Vec<PathBuf>| {
            for path in paths {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                File::create(path).unwrap();
            }
        };
        let selection = FollowDiscovery {
            codex_root: directory.join("codex"),
            pi_root: directory.join("pi"),
            claude_root: Some(directory.join("claude")),
            kiro_root: Some(directory.join("kiro")),
            opencode_db: Some(directory.join("opencode/opencode.db")),
            sessions_per_harness: MAX_SESSIONS_PER_HARNESS,
        };
        for root in [
            &selection.codex_root,
            &selection.pi_root,
            selection.claude_root.as_ref().unwrap(),
        ] {
            make_files(
                (0..8)
                    .map(|index| root.join(format!("{index}.jsonl")))
                    .collect(),
            );
        }
        let kiro = selection.kiro_root.as_ref().unwrap();
        make_files(vec![
            kiro.join("workspace/sess_1/messages.jsonl"),
            kiro.join("workspace/sess_1/sub-executions/worker.jsonl"),
            kiro.join("workspace/sess_1/unrelated.jsonl"),
            kiro.join("cli/sess_2/messages.jsonl"),
            kiro.join("workspace/unknown/messages.jsonl"),
            selection.opencode_db.clone().unwrap(),
        ]);
        let found = selection.sources().unwrap();
        fs::remove_dir_all(directory).unwrap();
        assert_eq!(found.len(), super::super::MAX_CONFIGURED_SOURCES);
        assert_eq!(
            found
                .iter()
                .take(5)
                .map(|source| source.plugin_id.as_str())
                .collect::<Vec<_>>(),
            [
                agenturm_runtime::CODEX_PLUGIN_ID,
                PI_PLUGIN_ID,
                CLAUDE_PLUGIN_ID,
                KIRO_PLUGIN_ID,
                OPENCODE_PLUGIN_ID
            ]
        );
        let kiro: BTreeSet<_> = found
            .iter()
            .filter(|source| source.plugin_id == KIRO_PLUGIN_ID)
            .map(|source| source.source_root.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(kiro, BTreeSet::from(["messages.jsonl", "worker.jsonl"]));
    }

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

        let result = recent_session_files(&root, 2, SourceShape::Transcript);
        let missing = recent_session_files(&root.join("missing"), 2, SourceShape::Transcript);
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
