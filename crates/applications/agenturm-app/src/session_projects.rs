//! Optional roster groups from explicit operator-saved task workspaces.

use agenturm_kernel::ExtensionId;
use agenturm_store::{SessionPage, Store, WorkspaceAnchor, WorkspaceLedger};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

type SessionKey = (ExtensionId, String);

pub(super) fn load(
    store: Option<&Store>,
    workspace: &Path,
    page: &SessionPage,
) -> BTreeMap<SessionKey, String> {
    let Some(store) = store.filter(|_| !page.sessions.is_empty()) else {
        return BTreeMap::new();
    };
    let keys = page
        .sessions
        .iter()
        .map(|row| (row.harness_id.clone(), row.session_id.clone()))
        .collect();
    let roots = WorkspaceLedger::open(workspace)
        .and_then(|ledger| ledger.session_task_project_roots(store, page.snapshot_id))
        .unwrap_or_default();
    projects(&keys, roots)
}

fn projects(
    keys: &BTreeSet<SessionKey>,
    roots: Vec<(WorkspaceAnchor, Option<String>)>,
) -> BTreeMap<SessionKey, String> {
    let mut values = BTreeMap::<_, Option<String>>::new();
    for (anchor, root) in roots {
        let key = (anchor.harness_id, anchor.session_id);
        if keys.contains(&key) {
            values
                .entry(key)
                .and_modify(|prior| {
                    if *prior != root {
                        *prior = None;
                    }
                })
                .or_insert(root);
        }
    }
    values
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_candidates_require_an_exact_unambiguous_session_root() {
        let anchor = |source: &str, session: &str| WorkspaceAnchor {
            harness_id: ExtensionId::parse(source).unwrap(),
            session_id: session.into(),
            worker_id: None,
            turn_id: None,
        };
        let a = anchor("harness.codex", "same");
        let b = anchor("harness.pi", "same");
        let c = anchor("harness.codex", "third");
        let d = anchor("harness.codex", "fourth");
        let foreign = anchor("harness.kiro", "same");
        let key = |anchor: &WorkspaceAnchor| (anchor.harness_id.clone(), anchor.session_id.clone());
        let keys = [&a, &b, &c, &d].into_iter().map(key).collect();
        let roots = vec![
            (a.clone(), Some("/work/codex".into())),
            (b.clone(), Some("/work/pi".into())),
            (a.clone(), Some("/work/codex".into())),
            (foreign, Some("/unloaded".into())),
            (b.clone(), Some("/work/conflict".into())),
            (b.clone(), Some("/work/pi".into())),
            (c.clone(), Some("/work/third".into())),
            (c.clone(), None),
            (c, Some("/work/third".into())),
        ];
        let observed = projects(&keys, roots);
        assert_eq!(observed, [(key(&a), "/work/codex".into())].into());
    }
}
