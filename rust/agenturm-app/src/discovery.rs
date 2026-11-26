//! Resolve local session files into the same explicit config used by collect.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub(crate) const MAX_SESSIONS_PER_HARNESS: usize = super::MAX_CONFIGURED_SOURCES / 2;
pub(crate) const DEFAULT_SESSIONS_PER_HARNESS: usize = 4;
pub(crate) const PI_PLUGIN_ID: &str = "example.rust-pi-harness";
pub(crate) const CLAUDE_PLUGIN_ID: &str = "example.rust-claude-harness";
pub(crate) const KIRO_PLUGIN_ID: &str = "example.rust-kiro-harness";
pub(crate) const OPENCODE_PLUGIN_ID: &str = "example.rust-opencode-harness";
const MAX_DISCOVERY_ENTRIES: usize = 100_000;
const MAX_DISCOVERY_DEPTH: usize = 8;
const MAX_TRACKED_TRANSCRIPTS: usize = 4_096;
const MAX_RETAINED_AUTHORITY_SOURCES: usize = 256;
const MAX_TRACKED_PATH_BYTES: usize = 2 * 1024 * 1024;
const MAX_SCAN_TIME: Duration = Duration::from_millis(500);
const MAX_CODEX_HEADER_BYTES: u64 = 64 * 1024;

mod open_transcripts;

#[derive(Clone, Debug)]
pub(crate) struct AuthorizedSource {
    pub(crate) source_id: String,
    pub(crate) plugin_id: String,
    pub(crate) source_root: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct SourceAuthority {
    pub(crate) follow: Option<FollowDiscovery>,
    pub(crate) sources: Vec<AuthorizedSource>,
    pub(crate) codex_home: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DiscoveryCoverage {
    pub(crate) selected: usize,
    pub(crate) eligible: usize,
    pub(crate) open_selected: usize,
    pub(crate) open_found: usize,
    pub(crate) excluded_reviews: usize,
    pub(crate) selected_reviews: usize,
    pub(crate) scan_partial: bool,
    pub(crate) handles_partial: bool,
}

impl DiscoveryCoverage {
    pub(crate) fn notice(self) -> String {
        let omitted = self.eligible.saturating_sub(self.selected);
        let state = if self.scan_partial || self.handles_partial {
            "Discovery partial"
        } else {
            "Discovery"
        };
        let mut text = format!("{state}: {}/{} selected", self.selected, self.eligible);
        if omitted > 0 {
            text.push_str(&format!(" · {omitted} queued"));
        }
        if self.open_found > 0 {
            text.push_str(&format!(
                " · {}/{} open handles",
                self.open_selected, self.open_found
            ));
        }
        if self.excluded_reviews > 0 {
            text.push_str(&format!(
                " · {}/{} internal reviews captured separately",
                self.selected_reviews, self.excluded_reviews
            ));
        }
        if self.scan_partial {
            text.push_str(" · file scan partial");
        }
        if self.handles_partial {
            text.push_str(" · process scan partial");
        }
        text
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FollowSchedule {
    epoch: u64,
    selected: BTreeMap<PathBuf, u64>,
    reviews: BTreeMap<PathBuf, HeaderStamp>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HeaderStamp {
    modified: SystemTime,
    len: u64,
    file: Option<open_transcripts::FileIdentity>,
    review: bool,
}

#[derive(Debug)]
struct Candidate {
    path: PathBuf,
    modified: SystemTime,
    open: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FollowDiscovery {
    /// Registry-backed declarations. New bundled sources use this list; the
    /// named fields below remain solely to read configs written by v2 builds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) targets: Vec<FollowTarget>,
    #[serde(default, skip_serializing_if = "path_buf_is_empty")]
    pub(crate) codex_root: PathBuf,
    #[serde(default, skip_serializing_if = "path_buf_is_empty")]
    pub(crate) pi_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claude_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kiro_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opencode_db: Option<PathBuf>,
    pub(crate) sessions_per_harness: usize,
}

fn path_buf_is_empty(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

/// Data-only discovery contribution shipped in the adapter catalog. Parsing
/// stays in the child adapter; the app only needs to know how to find bounded
/// candidate files and assign their stable host-owned source identities.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FollowTarget {
    pub(crate) namespace: String,
    pub(crate) plugin_id: String,
    pub(crate) root: PathBuf,
    pub(crate) shape: SourceShape,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceShape {
    JsonlTree,
    CodexJsonlTree,
    KiroJsonlTree,
    SqliteFile,
}

impl DiscoveryRegistration {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let slug = |value: &str| {
            !value.is_empty()
                && value.len() <= 32
                && value.as_bytes()[0].is_ascii_lowercase()
                && value.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte)
                })
        };
        if !slug(&self.namespace)
            || !safe_relative_path(&self.default_root)
            || self
                .macos_default_root
                .as_ref()
                .is_some_and(|path| !safe_relative_path(path))
            || !safe_optional_relative_path(&self.env_suffix)
            || !safe_optional_relative_path(&self.fallback_suffix)
            || self
                .root_env
                .as_deref()
                .is_some_and(|name| !valid_env_name(name))
            || self
                .fallback_base_env
                .as_deref()
                .is_some_and(|name| !valid_env_name(name))
        {
            return Err("invalid adapter discovery registration".into());
        }
        Ok(())
    }
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        && path.to_string_lossy().len() <= 1_024
}

fn safe_optional_relative_path(path: &Path) -> bool {
    path.as_os_str().is_empty() || safe_relative_path(path)
}

fn valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn discovery_root(registration: &DiscoveryRegistration) -> Result<PathBuf, String> {
    registration.validate()?;
    let root = if let Some(value) = registration
        .root_env
        .as_deref()
        .and_then(env::var_os)
        .filter(|value| !value.is_empty())
    {
        let root = PathBuf::from(value);
        if !root.is_absolute() {
            return Err(format!(
                "discovery root environment variable for {} must be absolute",
                registration.namespace
            ));
        }
        root.join(&registration.env_suffix)
    } else if let Some(value) = registration
        .fallback_base_env
        .as_deref()
        .and_then(env::var_os)
        .filter(|value| !value.is_empty())
    {
        let root = PathBuf::from(value);
        if !root.is_absolute() {
            return Err(format!(
                "discovery root environment variable for {} must be absolute",
                registration.namespace
            ));
        }
        root.join(&registration.fallback_suffix)
    } else {
        let home = env::var_os("HOME").ok_or("set HOME for default harness discovery")?;
        let home = PathBuf::from(home);
        if !home.is_absolute() {
            return Err("HOME must be an absolute path for harness discovery".into());
        }
        #[cfg(target_os = "macos")]
        if let Some(default_root) = &registration.macos_default_root {
            home.join(default_root)
        } else {
            home.join(&registration.default_root)
        }
        #[cfg(not(target_os = "macos"))]
        home.join(&registration.default_root)
    };
    if root.to_string_lossy().len() > 4_096 {
        return Err("resolved discovery root exceeds the path limit".into());
    }
    Ok(root)
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiscoveryRegistration {
    pub(crate) namespace: String,
    pub(crate) root_env: Option<String>,
    /// Relative to HOME when `root_env` is unset.
    pub(crate) default_root: PathBuf,
    pub(crate) macos_default_root: Option<PathBuf>,
    /// Appended to `root_env` when that variable names a base directory.
    #[serde(default)]
    pub(crate) env_suffix: PathBuf,
    pub(crate) fallback_base_env: Option<String>,
    #[serde(default)]
    pub(crate) fallback_suffix: PathBuf,
    pub(crate) shape: SourceShape,
}

impl FollowDiscovery {
    pub(crate) fn roots(&self) -> impl Iterator<Item = &Path> {
        self.targets().into_iter().map(|(_, _, path, _)| path)
    }

    pub(crate) fn resolve_paths(&mut self, resolve: impl Fn(PathBuf) -> PathBuf) {
        for target in &mut self.targets {
            target.root = resolve(std::mem::take(&mut target.root));
        }
        if self.targets.is_empty() {
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
    }

    pub(crate) fn plugins(&self) -> impl Iterator<Item = String> + '_ {
        self.targets()
            .into_iter()
            .map(|(_, plugin, _, _)| plugin.to_owned())
    }

    pub(crate) fn validate_catalog_targets(
        &self,
        catalog: &BTreeMap<String, super::ValidatedAdapter>,
    ) -> Result<(), String> {
        if self.targets.is_empty() {
            return Ok(());
        }
        let mut namespaces = BTreeSet::new();
        for target in &self.targets {
            let adapter = catalog.get(&target.plugin_id).ok_or_else(|| {
                format!(
                    "follow discovery requires plugin {} in the catalog",
                    target.plugin_id
                )
            })?;
            let registration = adapter.discovery.as_ref().ok_or_else(|| {
                format!(
                    "plugin {} does not declare discovery support",
                    target.plugin_id
                )
            })?;
            if target.namespace != registration.namespace || target.shape != registration.shape {
                return Err(format!(
                    "follow discovery declaration differs from catalog for {}",
                    target.plugin_id
                ));
            }
            if !namespaces.insert(target.namespace.as_str()) {
                return Err("follow discovery contains a duplicate namespace".into());
            }
        }
        Ok(())
    }

    fn targets(&self) -> Vec<(&str, &str, &Path, SourceShape)> {
        if !self.targets.is_empty() {
            return self
                .targets
                .iter()
                .map(|target| {
                    (
                        target.namespace.as_str(),
                        target.plugin_id.as_str(),
                        target.root.as_path(),
                        target.shape,
                    )
                })
                .collect();
        }
        [
            (
                "codex",
                agenturm_runtime::CODEX_PLUGIN_ID,
                Some(self.codex_root.as_path()),
                SourceShape::CodexJsonlTree,
            ),
            (
                "pi",
                PI_PLUGIN_ID,
                Some(self.pi_root.as_path()),
                SourceShape::JsonlTree,
            ),
            (
                "claude",
                CLAUDE_PLUGIN_ID,
                self.claude_root.as_deref(),
                SourceShape::JsonlTree,
            ),
            (
                "kiro",
                KIRO_PLUGIN_ID,
                self.kiro_root.as_deref(),
                SourceShape::KiroJsonlTree,
            ),
            (
                "opencode",
                OPENCODE_PLUGIN_ID,
                self.opencode_db.as_deref(),
                SourceShape::SqliteFile,
            ),
        ]
        .into_iter()
        .filter_map(|(name, plugin, path, shape)| Some((name, plugin, path?, shape)))
        .collect()
    }

    pub(crate) fn from_catalog(
        plugins: &BTreeMap<String, super::ValidatedAdapter>,
        sessions_per_harness: usize,
    ) -> Result<Self, String> {
        let mut targets = Vec::new();
        for (plugin_id, adapter) in plugins {
            let Some(registration) = &adapter.discovery else {
                continue;
            };
            let root = discovery_root(registration)?;
            targets.push(FollowTarget {
                namespace: registration.namespace.clone(),
                plugin_id: plugin_id.clone(),
                root,
                shape: registration.shape,
            });
        }
        targets.sort_by(|left, right| left.namespace.cmp(&right.namespace));
        if targets
            .windows(2)
            .any(|pair| pair[0].namespace == pair[1].namespace)
        {
            return Err("plugin catalog contains duplicate discovery namespaces".into());
        }
        Ok(Self {
            targets,
            codex_root: PathBuf::new(),
            pi_root: PathBuf::new(),
            claude_root: None,
            kiro_root: None,
            opencode_db: None,
            sessions_per_harness,
        })
    }

    pub(crate) fn claude_root(&self) -> Option<&Path> {
        self.targets()
            .into_iter()
            .find(|(namespace, _, _, _)| *namespace == "claude")
            .map(|(_, _, root, _)| root)
            .or(self.claude_root.as_deref())
    }

    pub(crate) fn codex_home(&self) -> Option<PathBuf> {
        let root = self
            .targets()
            .into_iter()
            .find(|(namespace, _, _, _)| *namespace == "codex")
            .map(|(_, _, root, _)| root)
            .unwrap_or(self.codex_root.as_path());
        (root.file_name()? == "sessions")
            .then(|| root.parent().map(Path::to_owned))
            .flatten()
    }

    pub(crate) fn sources(&self) -> Result<Vec<super::CollectionSourceInput>, String> {
        self.select(
            &mut FollowSchedule::default(),
            &Default::default(),
            super::MAX_CONFIGURED_SOURCES,
        )
        .map(|(sources, _)| sources)
    }

    pub(crate) fn follow_sources(
        &self,
        schedule: &mut FollowSchedule,
        capacity: usize,
    ) -> Result<(Vec<super::CollectionSourceInput>, DiscoveryCoverage), String> {
        self.select(schedule, &open_transcripts::current(), capacity)
    }

    /// Recover configured native members for retained source IDs independently
    /// of polling slots. Callers still verify each member's durable receipt.
    pub(crate) fn authorize_retained_sources(
        &self,
        wanted: &BTreeSet<String>,
    ) -> Result<Vec<AuthorizedSource>, String> {
        if wanted.len() > MAX_RETAINED_AUTHORITY_SOURCES {
            return Err("retained discovery exceeds the 256 source limit".into());
        }
        let open = open_transcripts::OpenTranscripts::default();
        let reviews = BTreeMap::new();
        let mut next_reviews = BTreeMap::new();
        let mut coverage = DiscoveryCoverage::default();
        let mut sources = Vec::new();
        for (kind, plugin, root, shape) in self.targets() {
            if !wanted.iter().any(|id| {
                id.strip_prefix("harness.")
                    .and_then(|rest| rest.split_once('.'))
                    .is_some_and(|(provider, _)| provider == kind)
            }) {
                continue;
            }
            if shape == SourceShape::SqliteFile {
                for path in regular_database(root)
                    .map_err(|error| format!("scan {kind} sessions failed: {error}"))?
                {
                    let entry = source_entry(kind, plugin, &path)?;
                    if wanted.contains(&entry.source_id) {
                        sources.push(AuthorizedSource {
                            source_id: entry.source_id,
                            plugin_id: entry.plugin_id,
                            source_root: entry.source_root,
                        });
                    }
                }
                continue;
            }
            let mut scan = TranscriptScan {
                harness: kind,
                shape,
                open: &open,
                reviews: &reviews,
                next_reviews: &mut next_reviews,
                coverage: &mut coverage,
            };
            for candidate in scan
                .files(root)
                .map_err(|error| format!("scan {kind} sessions failed: {error}"))?
            {
                let entry = source_entry(kind, plugin, &candidate.path)?;
                if wanted.contains(&entry.source_id) {
                    sources.push(AuthorizedSource {
                        source_id: entry.source_id,
                        plugin_id: entry.plugin_id,
                        source_root: entry.source_root,
                    });
                }
            }
        }
        Ok(sources)
    }

    fn select(
        &self,
        schedule: &mut FollowSchedule,
        open: &open_transcripts::OpenTranscripts,
        capacity: usize,
    ) -> Result<(Vec<super::CollectionSourceInput>, DiscoveryCoverage), String> {
        let mut openings = Vec::new();
        let mut catch_up = Vec::new();
        let mut coverage = DiscoveryCoverage {
            handles_partial: open.partial,
            ..DiscoveryCoverage::default()
        };
        let mut next_reviews = BTreeMap::new();
        let mut available = BTreeSet::new();
        for (kind, plugin, root, shape) in self.targets() {
            let candidates = match shape {
                SourceShape::SqliteFile => regular_database(root).map(|paths| {
                    coverage.eligible += paths.len();
                    paths
                        .into_iter()
                        .map(|path| Candidate {
                            path,
                            modified: SystemTime::UNIX_EPOCH,
                            open: false,
                        })
                        .collect()
                }),
                SourceShape::JsonlTree
                | SourceShape::CodexJsonlTree
                | SourceShape::KiroJsonlTree => {
                    let mut scan = TranscriptScan {
                        harness: kind,
                        shape,
                        open,
                        reviews: &schedule.reviews,
                        next_reviews: &mut next_reviews,
                        coverage: &mut coverage,
                    };
                    scan.files(root)
                }
            }
            .map_err(|error| format!("scan {kind} sessions failed: {error}"))?;
            let mut candidates = candidates;
            candidates.sort_unstable_by(|left, right| {
                let key = |item: &Candidate| {
                    (
                        schedule.selected.get(&item.path).copied().unwrap_or(0),
                        Reverse(item.modified),
                    )
                };
                key(left).cmp(&key(right)).then(left.path.cmp(&right.path))
            });
            available.extend(candidates.iter().map(|candidate| candidate.path.clone()));
            let (opened, historical): (Vec<_>, Vec<_>) =
                candidates.into_iter().partition(|candidate| candidate.open);
            coverage.open_found += opened.len();
            openings.push((kind, plugin, opened.into_iter()));
            catch_up.push((
                kind,
                plugin,
                historical.into_iter().take(self.sessions_per_harness),
            ));
        }
        let mut sources = Vec::with_capacity(capacity);
        if !openings.is_empty() {
            let first = (schedule.epoch % openings.len() as u64) as usize;
            openings.rotate_left(first);
            catch_up.rotate_left(first);
        }
        allocate(&mut openings, capacity, &mut sources)?;
        coverage.open_selected = sources.len();
        allocate(&mut catch_up, capacity, &mut sources)?;
        coverage.selected = sources.len();
        // Reserved built-in review slots never displace an ordinary harness.
        let mut review_paths = next_reviews
            .iter()
            .filter(|(_, stamp)| stamp.review)
            .collect::<Vec<_>>();
        review_paths.sort_by_key(|(path, stamp)| {
            (
                schedule.selected.get(*path).copied().unwrap_or(0),
                Reverse(stamp.modified),
                (*path).clone(),
            )
        });
        for (path, _) in review_paths.into_iter().take(2) {
            let Some((namespace, plugin, _, _)) =
                self.targets().into_iter().find(|(_, _, root, shape)| {
                    *shape == SourceShape::CodexJsonlTree && path.starts_with(root)
                })
            else {
                continue;
            };
            let mut source = source_entry(namespace, plugin, path)?;
            source.source_id = format!(
                "review.{namespace}.{:x}",
                Sha256::digest(config_path(path)?.as_bytes())
            );
            available.insert(path.clone());
            sources.push(source);
            coverage.selected_reviews += 1;
        }
        schedule.epoch = schedule.epoch.saturating_add(1);
        schedule.selected.retain(|path, _| available.contains(path));
        for source in &sources {
            schedule
                .selected
                .insert(source.source_root.clone(), schedule.epoch);
        }
        schedule.reviews = next_reviews;
        Ok((sources, coverage))
    }
}

fn allocate(
    groups: &mut [(&str, &str, impl Iterator<Item = Candidate>)],
    capacity: usize,
    sources: &mut Vec<super::CollectionSourceInput>,
) -> Result<(), String> {
    while sources.len() < capacity {
        let before = sources.len();
        for (kind, plugin, paths) in &mut *groups {
            if let Some(candidate) = paths.next() {
                sources.push(source_entry(kind, plugin, &candidate.path)?);
                if sources.len() == capacity {
                    break;
                }
            }
        }
        if sources.len() == before {
            break;
        }
    }
    Ok(())
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
    let selection = FollowDiscovery::from_catalog(&installed, sessions_per_harness)?;
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

fn regular_database(path: &Path) -> Result<Vec<PathBuf>, String> {
    match fs::canonicalize(path) {
        Ok(path) if path.is_file() => Ok(vec![path]),
        Ok(_) => Err("OpenCode database path is not a regular file".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.to_string()),
    }
}

struct TranscriptScan<'a> {
    harness: &'a str,
    shape: SourceShape,
    open: &'a open_transcripts::OpenTranscripts,
    reviews: &'a BTreeMap<PathBuf, HeaderStamp>,
    next_reviews: &'a mut BTreeMap<PathBuf, HeaderStamp>,
    coverage: &'a mut DiscoveryCoverage,
}

impl TranscriptScan<'_> {
    fn files(&mut self, root: &Path) -> Result<Vec<Candidate>, String> {
        let root = match fs::canonicalize(root) {
            Ok(root) => root,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };
        let entries = fs::read_dir(root).map_err(|error| error.to_string())?;
        let mut directories = vec![(entries, 0)];
        let mut candidates = Vec::new();
        let mut entries = 0;
        let mut paths = 0;
        let mut path_bytes = 0;
        let started = Instant::now();
        while let Some((directory, depth)) = directories.last_mut() {
            let Some(entry) = directory.next() else {
                directories.pop();
                continue;
            };
            entries += 1;
            if entries > MAX_DISCOVERY_ENTRIES || started.elapsed() > MAX_SCAN_TIME {
                self.coverage.scan_partial = true;
                break;
            }
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            let path = entry.path();
            if kind.is_dir() {
                if matches!(self.shape, SourceShape::KiroJsonlTree)
                    && matches!(
                        entry.file_name().to_str(),
                        Some("cli" | "snapshots" | ".index")
                    )
                {
                    continue;
                }
                if *depth < MAX_DISCOVERY_DEPTH {
                    let next_depth = *depth + 1;
                    match fs::read_dir(path) {
                        Ok(entries) => directories.push((entries, next_depth)),
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => return Err(error.to_string()),
                    }
                } else {
                    self.coverage.scan_partial = true;
                }
                continue;
            }
            if !kind.is_file() || !is_transcript(&path, self.shape) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(error.to_string()),
            };
            let modified = metadata.modified().map_err(|error| error.to_string())?;
            let file = open_transcripts::file_identity(&metadata);
            let open = self.open.contains(self.harness, file);
            let Ok(valid_path) = config_path(&path) else {
                self.coverage.scan_partial = true;
                continue;
            };
            if !open
                && (paths >= MAX_TRACKED_TRANSCRIPTS
                    || path_bytes + valid_path.len() > MAX_TRACKED_PATH_BYTES)
            {
                self.coverage.scan_partial = true;
                continue;
            }
            paths += 1;
            path_bytes += valid_path.len();
            if matches!(self.shape, SourceShape::CodexJsonlTree)
                && self.is_review(
                    &path,
                    HeaderStamp {
                        modified,
                        len: metadata.len(),
                        file,
                        review: false,
                    },
                )
            {
                self.coverage.excluded_reviews += 1;
                continue;
            }
            self.coverage.eligible += 1;
            candidates.push(Candidate {
                path,
                modified,
                open,
            });
        }
        Ok(candidates)
    }

    fn is_review(&mut self, path: &Path, mut stamp: HeaderStamp) -> bool {
        if let Some(prior) = self.reviews.get(path)
            && prior.file == stamp.file
            && prior.len == stamp.len
            && prior.modified == stamp.modified
        {
            stamp.review = prior.review;
        } else {
            match codex_review(path, stamp.file) {
                Ok(review) => stamp.review = review,
                Err(error) if error.kind() == ErrorKind::NotFound => return false,
                Err(_) => {
                    self.coverage.scan_partial = true;
                    return false;
                }
            }
        }
        let review = stamp.review;
        self.next_reviews.insert(path.to_owned(), stamp);
        review
    }
}

fn codex_review(
    path: &Path,
    identity: Option<open_transcripts::FileIdentity>,
) -> std::io::Result<bool> {
    #[derive(Deserialize)]
    struct Header {
        #[serde(rename = "type")]
        kind: String,
        payload: MetadataRow,
    }
    #[derive(Deserialize)]
    struct MetadataRow {
        #[serde(default)]
        source: Option<serde_json::Value>,
        #[serde(default)]
        thread_source: Option<String>,
    }

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || open_transcripts::file_identity(&metadata) != identity {
        return Err(std::io::Error::other(
            "transcript identity changed during discovery",
        ));
    }
    let mut line = Vec::new();
    BufReader::new(file.take(MAX_CODEX_HEADER_BYTES)).read_until(b'\n', &mut line)?;
    if !line.ends_with(b"\n") {
        return Ok(false);
    }
    let Ok(header) = serde_json::from_slice::<Header>(&line) else {
        return Ok(false);
    };
    Ok(header.kind == "session_meta"
        && (header.payload.thread_source.as_deref() == Some("guardian_review")
            || header
                .payload
                .source
                .as_ref()
                .and_then(|source| source.pointer("/subagent/other"))
                .and_then(serde_json::Value::as_str)
                == Some("guardian")))
}

fn is_transcript(path: &Path, shape: SourceShape) -> bool {
    if path.extension().is_none_or(|ext| ext != "jsonl") {
        return false;
    }
    if !matches!(shape, SourceShape::KiroJsonlTree) {
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
    fn retained_authority_recovers_exact_configured_members_outside_polling_slots() {
        let directory =
            env::temp_dir().join(format!("agenturm-discover-retained-{}", std::process::id()));
        let codex = directory.join("codex/sessions/2026/11");
        let claude = directory.join("claude/projects/fixture");
        let pi = directory.join("pi");
        for path in [&codex, &claude, &pi] {
            fs::create_dir_all(path).unwrap();
        }
        let codex_header = r#"{"type":"session_meta","payload":{"id":"session"}}"#;
        let write = |path: &Path, first: &str, modified| {
            fs::write(path, format!("{first}\n")).unwrap();
            File::open(path)
                .unwrap()
                .set_times(
                    FileTimes::new()
                        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(modified)),
                )
                .unwrap();
        };
        let retained = codex.join("retained.jsonl");
        let review = codex.join("review.jsonl");
        let claude_retained = claude.join("retained.jsonl");
        let pi_retained = pi.join("retained.jsonl");
        write(&retained, codex_header, 1);
        for index in 0..20 {
            write(
                &codex.join(format!("newer-{index}.jsonl")),
                codex_header,
                10 + index,
            );
        }
        write(
            &review,
            r#"{"type":"session_meta","payload":{"source":{"subagent":{"other":"guardian"}}}}"#,
            100,
        );
        write(
            &claude_retained,
            r#"{"type":"user","sessionId":"native-claude"}"#,
            1,
        );
        write(&pi_retained, r#"{"type":"session","id":"native-pi"}"#, 1);
        let selection = FollowDiscovery {
            targets: vec![],
            codex_root: directory.join("codex/sessions"),
            pi_root: pi,
            claude_root: Some(directory.join("claude/projects")),
            kiro_root: None,
            opencode_db: None,
            sessions_per_harness: 4,
        };
        let selected = selection.sources().unwrap();
        assert!(!selected.iter().any(|entry| entry.source_root == retained));
        let expected = [
            source_entry("codex", agenturm_runtime::CODEX_PLUGIN_ID, &retained).unwrap(),
            source_entry("claude", CLAUDE_PLUGIN_ID, &claude_retained).unwrap(),
            source_entry("pi", PI_PLUGIN_ID, &pi_retained).unwrap(),
        ];
        let mut wanted = expected
            .iter()
            .map(|entry| entry.source_id.clone())
            .collect::<BTreeSet<_>>();
        wanted.insert(
            source_entry("codex", agenturm_runtime::CODEX_PLUGIN_ID, &review)
                .unwrap()
                .source_id,
        );
        wanted.insert(
            source_entry("pi", PI_PLUGIN_ID, &pi_retained)
                .unwrap()
                .source_id,
        );
        wanted.insert("harness.codex.unconfigured".into());
        let authorized = selection.authorize_retained_sources(&wanted).unwrap();
        assert_eq!(authorized.len(), expected.len());
        for entry in &expected {
            assert!(authorized.iter().any(|source| (
                source.source_id.as_str(),
                source.plugin_id.as_str(),
                source.source_root.as_path()
            ) == (
                entry.source_id.as_str(),
                entry.plugin_id.as_str(),
                entry.source_root.as_path()
            )));
        }
        let over_limit = (0..=MAX_RETAINED_AUTHORITY_SOURCES)
            .map(|index| format!("harness.codex.{index}"))
            .collect();
        assert!(selection.authorize_retained_sources(&over_limit).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

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
            targets: vec![],
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

        let mut selection = FollowDiscovery {
            targets: vec![],
            codex_root: root.clone(),
            pi_root: directory.join("pi-missing"),
            claude_root: None,
            kiro_root: None,
            opencode_db: None,
            sessions_per_harness: 2,
        };
        let result: Vec<_> = selection
            .sources()
            .unwrap()
            .into_iter()
            .map(|source| source.source_root)
            .collect();
        selection.codex_root = root.join("missing");
        assert!(selection.sources().unwrap().is_empty());
        fs::remove_dir_all(&directory).unwrap();

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

    #[cfg(target_os = "linux")]
    #[test]
    fn follow_keeps_old_open_sessions_and_rotates_around_review_files_and_capacity() {
        let directory =
            env::temp_dir().join(format!("agenturm-discover-rotation-{}", std::process::id()));
        let codex = directory.join("codex/sessions");
        let pi = directory.join("pi");
        fs::create_dir_all(&codex).unwrap();
        fs::create_dir(&pi).unwrap();
        let writer = |root: &Path, name: String, seconds, head: &str| {
            let path = root.join(name);
            fs::write(&path, format!("{head}\n")).unwrap();
            File::open(&path)
                .unwrap()
                .set_times(
                    FileTimes::new()
                        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
                )
                .unwrap();
            path
        };
        let regular = r#"{"type":"session_meta","payload":{"source":"cli"}}"#;
        let idle = (0..7)
            .map(|n| writer(&codex, format!("idle-{n}.jsonl"), n + 1, regular))
            .collect::<Vec<_>>();
        let history = (0..6).map(|n| writer(&codex, format!("history-{n}.jsonl"), n + 20,
            r#"{"type":"session_meta","payload":{"source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}}}"#)).collect::<Vec<_>>();
        let review = writer(
            &codex,
            "newest-review.jsonl".into(),
            100,
            r#"{"type":"session_meta","payload":{"source":{"subagent":{"other":"guardian"}}}}"#,
        );
        let mut open_files = idle.clone();
        open_files.push(review.clone());
        let open = open_transcripts::OpenTranscripts::from_files("codex", &open_files);
        let selection = FollowDiscovery {
            targets: vec![],
            codex_root: codex.clone(),
            pi_root: pi.clone(),
            claude_root: None,
            kiro_root: None,
            opencode_db: None,
            sessions_per_harness: 4,
        };
        let mut scheduler = FollowSchedule::default();
        let mut visited = BTreeSet::new();
        for _ in 0..2 {
            let (batch, coverage) = selection.select(&mut scheduler, &open, 10).unwrap();
            let paths = batch
                .into_iter()
                .map(|source| source.source_root)
                .collect::<BTreeSet<_>>();
            assert!(idle.iter().all(|path| paths.contains(path)));
            assert_eq!(coverage.eligible, 13);
            assert_eq!(coverage.open_selected, 7);
            assert_eq!(coverage.open_found, 7);
            assert_eq!(coverage.excluded_reviews, 1);
            assert_eq!(coverage.selected_reviews, 1);
            assert!(paths.contains(&review));
            assert_eq!(coverage.selected, 10);
            assert!(coverage.notice().contains("3 queued"));
            visited.extend(paths);
        }
        assert!(history.iter().all(|path| visited.contains(path)));

        // When only one catch-up slot remains, another harness still gets a turn.
        let pi_member = writer(
            &pi,
            "pi.jsonl".into(),
            150,
            r#"{"type":"session","id":"p"}"#,
        );
        let history_batches = (0..2)
            .flat_map(|_| selection.select(&mut scheduler, &open, 8).unwrap().0)
            .filter(|source| {
                !idle.contains(&source.source_root)
                    && !source.source_id.starts_with("review.codex.")
            })
            .map(|source| source.source_root)
            .collect::<Vec<_>>();
        assert_eq!(history_batches.len(), 2);
        assert!(history_batches.contains(&pi_member));
        assert!(history_batches.iter().any(|path| history.contains(path)));

        // A newly open member competes fairly even when open handles exhaust capacity.
        let newcomer = writer(&codex, "newcomer.jsonl".into(), 200, regular);
        open_files.push(newcomer.clone());
        let open = open_transcripts::OpenTranscripts::from_files("codex", &open_files);
        let mut seen_open = BTreeSet::new();
        for _ in 0..2 {
            let (batch, coverage) = selection.select(&mut scheduler, &open, 4).unwrap();
            assert_eq!(coverage.open_found, 8);
            assert_eq!(coverage.open_selected, 4);
            assert_eq!(coverage.selected, 4);
            assert!(coverage.notice().contains("4/8 open handles"));
            seen_open.extend(
                batch
                    .into_iter()
                    .filter(|source| !source.source_id.starts_with("review.codex."))
                    .map(|source| source.source_root),
            );
        }
        assert_eq!(seen_open.len(), 8);
        assert!(seen_open.contains(&newcomer));
        let (after_restart, _) = selection
            .select(&mut FollowSchedule::default(), &open, 10)
            .unwrap();
        assert!(
            after_restart
                .iter()
                .any(|source| source.source_root == newcomer)
        );
        assert_eq!(selection.codex_home(), Some(directory.join("codex")));
        fs::remove_dir_all(directory).unwrap();
    }
}
