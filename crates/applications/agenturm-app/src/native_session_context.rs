//! Native roster context remains separate from durable observation identities.

#[cfg(target_os = "linux")]
mod linux;

use agenturm_kernel::ExtensionId;
use agenturm_store::{SessionPage, Store};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use super::discovery::{AuthorizedSource, SourceAuthority};

type SessionKey = (ExtensionId, String);

#[derive(Default)]
pub(super) struct NativeContext {
    pub(super) title: Option<String>,
    pub(super) agent_name: Option<String>,
    pub(super) agent_role: Option<String>,
    pub(super) agent_path: Option<String>,
    pub(super) model_provider: Option<String>,
    pub(super) workspace: Option<String>,
    pub(super) partial_title: bool,
}

pub(super) fn load(
    store: Option<&Store>,
    history: &Path,
    page: &SessionPage,
    authority: Option<&SourceAuthority>,
) -> BTreeMap<SessionKey, NativeContext> {
    #[cfg(target_os = "linux")]
    return linux::load(store, history, page, authority);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, history, page, authority);
        BTreeMap::new()
    }
}

const MAX_VALIDATED_MEMBERS: usize = 256;

/// The active collector has an immutable validated configuration. Keep a
/// bounded list of its previously selected members within that startup, so
/// rotation does not remove names from the roster immediately.
#[derive(Default)]
pub(super) struct StartupMembers {
    follow: Option<super::discovery::FollowDiscovery>,
    codex_home: Option<PathBuf>,
    members: VecDeque<AuthorizedSource>,
}

impl StartupMembers {
    pub(super) fn update(&mut self, authority: Option<SourceAuthority>) -> Option<SourceAuthority> {
        let Some(authority) = authority else {
            *self = Self::default();
            return None;
        };
        if self.codex_home != authority.codex_home || self.follow != authority.follow {
            self.members.clear();
        }
        self.follow = authority.follow;
        self.codex_home = authority.codex_home;
        for source in authority.sources {
            self.members
                .retain(|member| member.source_id != source.source_id);
            self.members.push_back(source);
            if self.members.len() > MAX_VALIDATED_MEMBERS {
                self.members.pop_front();
            }
        }
        Some(SourceAuthority {
            follow: self.follow.clone(),
            sources: self.members.iter().cloned().collect(),
            codex_home: self.codex_home.clone(),
        })
    }
}

/// An explicit local history can use the operator's collection config.
pub(super) fn configured_sources(history: &Path) -> Option<SourceAuthority> {
    let path = std::env::var_os("AGENTURM_CONTENT_CONFIG").map_or_else(
        || history.with_file_name("sources.json"),
        std::path::PathBuf::from,
    );
    let config = super::load_collection_config(&path).ok()?;
    let codex_home = config
        .follow
        .as_ref()
        .and_then(|follow| codex_home_from_sessions_root(&follow.selection.codex_root))
        .or_else(|| {
            config
                .sources
                .iter()
                .find(|source| source.plugin_id == agenturm_runtime::CODEX_PLUGIN_ID)
                .and_then(|source| {
                    source
                        .source_root
                        .ancestors()
                        .find(|path| path.file_name().is_some_and(|name| name == "sessions"))
                        .and_then(codex_home_from_sessions_root)
                })
        });
    Some(SourceAuthority {
        follow: config
            .follow
            .as_ref()
            .map(|follow| follow.selection.clone()),
        sources: config
            .sources
            .into_iter()
            .filter(|source| source.artifact_sha256.is_some())
            .map(|source| AuthorizedSource {
                source_id: source.source_id,
                plugin_id: source.plugin_id,
                source_root: source.source_root,
            })
            .collect(),
        codex_home,
    })
}

fn codex_home_from_sessions_root(root: &Path) -> Option<PathBuf> {
    (root.file_name()? == "sessions")
        .then(|| root.parent().map(Path::to_owned))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_codex_sources_find_only_a_sessions_ancestor() {
        let home = Path::new("/configured/codex");
        let source = home.join("sessions/2026/11/rollout.jsonl");
        let inferred = source
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == "sessions"))
            .and_then(codex_home_from_sessions_root);
        assert_eq!(inferred, Some(home.to_owned()));
        assert!(codex_home_from_sessions_root(Path::new("/configured/transcripts")).is_none());
        assert!(
            Path::new("/configured/other/rollout.jsonl")
                .ancestors()
                .find(|path| path.file_name().is_some_and(|name| name == "sessions"))
                .is_none()
        );
    }

    #[test]
    fn startup_members_retain_rotated_bindings_and_clear_across_authority_changes() {
        let mut remembered = StartupMembers::default();
        let make = |home: &str, id: &str| SourceAuthority {
            follow: None,
            sources: vec![AuthorizedSource {
                source_id: id.into(),
                plugin_id: "example.rust-codex-harness".into(),
                source_root: PathBuf::from(home)
                    .join("sessions")
                    .join(format!("{id}.jsonl")),
            }],
            codex_home: Some(PathBuf::from(home)),
        };
        remembered.update(Some(make("/first", "previous"))).unwrap();
        let all = remembered.update(Some(make("/first", "current"))).unwrap();
        assert_eq!(
            all.sources
                .iter()
                .map(|source| source.source_id.as_str())
                .collect::<Vec<_>>(),
            ["previous", "current"]
        );
        let changed = remembered
            .update(Some(make("/changed", "only-new")))
            .unwrap();
        assert_eq!(changed.sources.len(), 1);
        assert_eq!(changed.sources[0].source_id, "only-new");
        assert!(remembered.update(None).is_none());
        let restarted = remembered
            .update(Some(make("/changed", "after-reset")))
            .unwrap();
        assert_eq!(restarted.sources.len(), 1);
        assert_eq!(restarted.sources[0].source_id, "after-reset");
    }
}
