"""Canonical agent identity, delegation evidence, and tree projection.

Hierarchy is an observation model, not a process model.  A node is one observed
agent execution; an edge is a delegation that a runtime recorded explicitly.
Span, message, process, fork, and project relationships are deliberately kept
separate.

The module is UI-independent so discovery, storage, the HTTP API, and the
terminal cockpit all resolve identity the same way.
"""
from __future__ import annotations

import hashlib
import re

AGENT_ID = re.compile(r"^ag1_[0-9a-f]{32}$")
MAX_PROVENANCE = 512
MAX_PARENT_CLAIMS = 4
MAX_TRAVERSAL_NODES = 10000
MAX_TREE_DEPTH = 128

RELATIONSHIP_KINDS = ("delegated", "forked", "unknown")
RELATIONSHIP_STATES = ("confirmed", "unresolved", "conflicted", "invalid")
DISCOVERY_STATES = ("observed", "announced")
COVERAGE_STATES = ("complete", "partial", "unknown")

# Verified against the public/runtime formats documented in docs/agent-hierarchy.md.
CAPABILITIES = {
    "Codex": {
        "child_discovery": "separate rollout transcripts; exact parent_thread_id",
        "delegation_linking": "exact: session_meta.parent_thread_id and source.subagent.thread_spawn",
        "independent_child_evidence": True,
        "limitations": "Older subagent rollouts without parent_thread_id remain unlinked; forks are not delegation.",
    },
    "Claude Code": {
        "child_discovery": "subagent sidecar transcripts under <session>/subagents/agent-*.jsonl",
        "delegation_linking": "exact: transcript path parent session and sidecar parentAgentId/toolUseId",
        "independent_child_evidence": True,
        "limitations": "Sidecars are optional on older versions; missing parentAgentId falls back to the session directory.",
    },
    "OpenCode": {
        "child_discovery": "child rows in the session table",
        "delegation_linking": "exact: session.parent_id",
        "independent_child_evidence": True,
        "limitations": "Detached child sessions without a retained session row remain unlinked observations.",
    },
    "Pi": {
        "child_discovery": "explicit parent/session fields only",
        "delegation_linking": "conditional: explicit parentSessionId/parentAgentId metadata",
        "independent_child_evidence": False,
        "limitations": "No documented subagent transcript format was verified; unlinked sessions are labelled unknown.",
    },
    "SDK / OTLP": {
        "child_discovery": "explicit producer attributes only",
        "delegation_linking": "conditional: agent.id / agent.parent_id / agent.relationship.kind",
        "independent_child_evidence": True,
        "limitations": "A nested span alone is not proof of a subagent. Schedules that omit the convention remain unlinked.",
    },
}


def canonical_agent_id(runtime, namespace, native_id, worker_id=None):
    """Return an opaque, stable identity for one execution.

    ``namespace`` distinguishes producers that may reuse a native session ID
    (for example two OpenCode databases).  ``worker_id`` is needed when one
    runtime session hosts several agents, as Claude Code does.
    """
    if not isinstance(runtime, str) or not runtime.strip():
        raise ValueError("runtime is required")
    if not isinstance(native_id, str) or not native_id.strip():
        raise ValueError("native execution id is required")
    material = "\0".join((runtime.strip(), str(namespace or runtime).strip(),
                          native_id.strip(), str(worker_id or "").strip()))
    return "ag1_" + hashlib.sha256(material.encode("utf-8", "replace")).hexdigest()[:32]


def provenance(adapter, reference, observed_ns=None):
    """Bounded, non-content provenance for one relationship observation."""
    pieces = [str(adapter or "unknown")[:80], str(reference or "unreported")[:240]]
    if observed_ns:
        try:
            pieces.append(str(int(observed_ns)))
        except (TypeError, ValueError, OverflowError):
            pass
    return " · ".join(pieces)[:MAX_PROVENANCE]


def default_relationship(*, parent_agent_id=None, kind="unknown", state="unresolved",
                         reference="no parent metadata", adapter="unknown",
                         observed_ns=None, discovery="observed", coverage="unknown"):
    """Fields common to every node, including honest unknown roots."""
    return {
        "parent_agent_id": parent_agent_id,
        "relationship_kind": kind if kind in RELATIONSHIP_KINDS else "unknown",
        "relationship_state": state if state in RELATIONSHIP_STATES else "unresolved",
        "relationship_provenance": provenance(adapter, reference, observed_ns),
        "discovery_state": discovery if discovery in DISCOVERY_STATES else "observed",
        "coverage": coverage if coverage in COVERAGE_STATES else "unknown",
    }


def normalize_node(node):
    """Validate the hierarchy portion of a source/session entry.

    Non-hierarchy fields are preserved.  Invalid identity is rejected rather
    than guessed, so a malformed producer cannot create a phantom tree.
    """
    result = dict(node)
    agent_id = result.get("agent_id")
    if not isinstance(agent_id, str) or not AGENT_ID.fullmatch(agent_id):
        raise ValueError("invalid agent_id")
    result["agent_id"] = agent_id
    parent = result.get("parent_agent_id")
    result["parent_agent_id"] = parent if isinstance(parent, str) and AGENT_ID.fullmatch(parent) else None
    for key, allowed, fallback in (("relationship_kind", RELATIONSHIP_KINDS, "unknown"),
                                   ("relationship_state", RELATIONSHIP_STATES, "unresolved"),
                                   ("discovery_state", DISCOVERY_STATES, "observed"),
                                   ("coverage", COVERAGE_STATES, "unknown")):
        if result.get(key) not in allowed:
            result[key] = fallback
    result["relationship_provenance"] = str(
        result.get("relationship_provenance") or "not reported")[:MAX_PROVENANCE]
    claims = result.get("parent_claims")
    result["parent_claims"] = _claims(claims)
    return result


def _claims(value):
    if not isinstance(value, list):
        return []
    claims = []
    for item in value[:MAX_PARENT_CLAIMS]:
        if not isinstance(item, dict):
            continue
        parent = item.get("parent_agent_id")
        if not isinstance(parent, str) or not AGENT_ID.fullmatch(parent):
            continue
        claims.append({
            "parent_agent_id": parent,
            "provenance": str(item.get("provenance") or "")[:MAX_PROVENANCE],
            "observed_ns": _int(item.get("observed_ns")),
        })
    return claims


def _int(value):
    try:
        return max(0, int(value))
    except (TypeError, ValueError, OverflowError):
        return 0


def _is_observed(node):
    return node.get("discovery_state") != "announced"


def node_attention(node):
    """Shared attention rule used for counts and badges.

    The evidence-based session classifier sets ``attention`` explicitly when
    available.  This fallback keeps backend summaries useful without importing
    the terminal module.
    """
    if "attention" in node:
        return bool(node.get("attention"))
    state = str(node.get("state") or "")
    reason = str(node.get("waiting_for") or "").casefold()
    if state == "Waiting":
        return any(marker in reason for marker in
                   ("approval", "permission", "user input", "input request"))
    return state in {"Possibly stalled", "Error", "Failed", "Incomplete"}


def node_active(node):
    if "active" in node:
        return bool(node.get("active"))
    return str(node.get("state") or "") in {"Working", "Processing", "Tool running", "Working (quiet)"}


class HierarchyGraph:
    """One adjacency index per hierarchy revision.

    All traversal is iterative and bounded.  Duplicate observations are merged
    by canonical identity; a conflicting parent claim is preserved and the node
    is projected as unlinked until it can be resolved.
    """

    def __init__(self, nodes):
        self.nodes = {}
        self.diagnostics = {
            "duplicates": 0, "conflicts": 0, "cycles": 0, "invalid": 0,
            "missing_parents": 0, "omitted_nodes": 0, "announced": 0,
        }
        self._merge(nodes)
        self.children = {agent_id: [] for agent_id in self.nodes}
        self._summary_cache = {}
        self._link()
        self._build_children()

    def _merge(self, nodes):
        for raw in nodes or ():
            try:
                node = normalize_node(raw)
            except (ValueError, TypeError):
                self.diagnostics["invalid"] += 1
                continue
            agent_id = node["agent_id"]
            previous = self.nodes.get(agent_id)
            if previous is None:
                self.nodes[agent_id] = node
                continue
            self.diagnostics["duplicates"] += 1
            merged = _merge_observation(previous, node)
            if merged.get("relationship_state") == "conflicted":
                self.diagnostics["conflicts"] += 1
            self.nodes[agent_id] = merged
        if len(self.nodes) > MAX_TRAVERSAL_NODES:
            keep = sorted(self.nodes, key=lambda key: (
                _int(self.nodes[key].get("stamp")), key), reverse=True)[:MAX_TRAVERSAL_NODES]
            self.diagnostics["omitted_nodes"] = len(self.nodes) - len(keep)
            self.nodes = {key: self.nodes[key] for key in keep}
        for node in self.nodes.values():
            if node.get("discovery_state") == "announced":
                self.diagnostics["announced"] += 1

    def _link(self):
        for agent_id, node in self.nodes.items():
            node["parent_available"] = False
            parent = node.get("parent_agent_id")
            if not parent:
                if node.get("relationship_state") == "confirmed":
                    node["relationship_kind"] = node.get("relationship_kind") or "unknown"
                continue
            if parent == agent_id:
                node["relationship_state"] = "invalid"
                node["relationship_provenance"] = (
                    str(node.get("relationship_provenance") or "") + " · self-link rejected")[:MAX_PROVENANCE]
                node["parent_agent_id"] = None
                self.diagnostics["invalid"] += 1
                continue
            if node.get("relationship_state") in {"conflicted", "invalid"}:
                continue
            if parent not in self.nodes:
                self.diagnostics["missing_parents"] += 1
                node["parent_available"] = False
                continue
            node["parent_available"] = True
        # Cycle detection over confirmed delegation edges.  Forks and unknown
        # relationships do not form a tree edge.  Build the candidate
        # adjacency once so wide synthetic forests stay linear in edges.
        candidates = {agent_id: [] for agent_id in self.nodes}
        for child, node in self.nodes.items():
            parent = node.get("parent_agent_id")
            if (parent in candidates and parent != child and
                    node.get("relationship_state") == "confirmed" and
                    node.get("relationship_kind") == "delegated"):
                candidates[parent].append(child)
        for children in candidates.values():
            children.sort()
        self._candidate_children_map = candidates
        color = {}
        for start in sorted(self.nodes):
            if color.get(start, 0):
                continue
            stack = [(start, iter(candidates[start]))]
            color[start] = 1
            path = [start]
            while stack:
                node_id, children = stack[-1]
                try:
                    child = next(children)
                except StopIteration:
                    color[node_id] = 2
                    stack.pop()
                    if path and path[-1] == node_id:
                        path.pop()
                    continue
                state = color.get(child, 0)
                if state == 1:
                    cycle = path[path.index(child):] if child in path else [child]
                    self.diagnostics["cycles"] += 1
                    for member in cycle:
                        item = self.nodes[member]
                        item["relationship_state"] = "invalid"
                        item["parent_agent_id"] = None
                        item["parent_available"] = False
                        item["relationship_provenance"] = (
                            str(item.get("relationship_provenance") or "") +
                            " · cycle rejected")[:MAX_PROVENANCE]
                    continue
                if state == 2:
                    continue
                color[child] = 1
                path.append(child)
                stack.append((child, iter(candidates[child])))

    def _build_children(self):
        for agent_id, node in self.nodes.items():
            parent = node.get("parent_agent_id")
            if (parent in self.children and node.get("relationship_state") == "confirmed" and
                    node.get("relationship_kind") == "delegated" and parent != agent_id):
                self.children[parent].append(agent_id)
        for parent, children in self.children.items():
            children.sort(key=lambda child: self.sibling_order_key(child))

    def children_of(self, agent_id):
        return list(self.children.get(agent_id, ()))

    def parent_of(self, agent_id):
        node = self.nodes.get(agent_id) or {}
        parent = node.get("parent_agent_id")
        return parent if parent in self.nodes else None

    def roots(self):
        return sorted((agent_id for agent_id, node in self.nodes.items()
                       if not node.get("parent_available") or
                       node.get("relationship_state") != "confirmed" or
                       node.get("relationship_kind") != "delegated"),
                      key=self.root_order_key)

    def root_order_key(self, agent_id):
        summary = self.summary(agent_id)
        node = self.nodes[agent_id]
        return (-summary["attention"], -summary["last_activity"],
                -_int(node.get("stamp")), agent_id)

    def sibling_order_key(self, agent_id):
        summary = self.summary(agent_id)
        node = self.nodes[agent_id]
        return (-summary["attention"], not _is_observed(node), -summary["last_activity"],
                -_int(node.get("stamp")), agent_id)

    def ancestors(self, agent_id, *, include_self=False):
        result = []
        seen = set()
        current = agent_id
        while current and current in self.nodes and current not in seen and len(result) < MAX_TREE_DEPTH:
            seen.add(current)
            if current != agent_id or include_self:
                result.append(current)
            current = self.parent_of(current)
        return list(reversed(result))

    def descendants(self, agent_id, *, include_self=False):
        result = []
        seen = set()
        stack = [agent_id]
        while stack and len(result) < MAX_TRAVERSAL_NODES:
            current = stack.pop()
            if current in seen or current not in self.nodes:
                continue
            seen.add(current)
            if current != agent_id or include_self:
                result.append(current)
            stack.extend(reversed(self.children_of(current)))
        return result

    def subtree_ids(self, agent_id, include_self=True):
        return self.descendants(agent_id, include_self=include_self)

    def summary(self, agent_id, *, include_self=False):
        """Counts unique observed descendants; announced placeholders never work."""
        key = (agent_id, bool(include_self))
        cached = self._summary_cache.get(key)
        if cached is not None:
            return dict(cached)
        ids = self.descendants(agent_id, include_self=include_self)
        counts = {"working": 0, "waiting": 0, "failed": 0, "attention": 0,
                  "observed": 0, "announced": 0, "unknown": 0, "last_activity": 0}
        coverage = "complete"
        for item_id in ids:
            node = self.nodes[item_id]
            if not _is_observed(node):
                counts["announced"] += 1
                coverage = "unknown" if coverage == "complete" else coverage
                continue
            counts["observed"] += 1
            if node_active(node):
                counts["working"] += 1
            state = str(node.get("state") or "")
            if state == "Waiting":
                counts["waiting"] += 1
            if state in {"Error", "Failed", "Incomplete"}:
                counts["failed"] += 1
            if node_attention(node):
                counts["attention"] += 1
            stamp = _int(node.get("stamp") or node.get("last_record_ns"))
            counts["last_activity"] = max(counts["last_activity"], stamp)
            if node.get("coverage") == "partial" and coverage == "complete":
                coverage = "partial"
            elif node.get("coverage") == "unknown":
                coverage = "unknown"
        counts["coverage"] = coverage
        counts["descendants"] = max(0, len(ids) - (1 if include_self else 0))
        self._summary_cache[key] = counts
        return dict(counts)

    def path_to(self, agent_id, allowed=None):
        """Root-to-node breadcrumb, optionally stopping at allowed context."""
        path = []
        for item in self.ancestors(agent_id, include_self=True):
            if allowed is not None and item not in allowed:
                break
            path.append(item)
        return path

    def context_for_matches(self, matches):
        """Matching nodes plus ancestor context needed to reveal their paths."""
        context = set()
        for agent_id in matches:
            if agent_id not in self.nodes:
                continue
            context.update(self.ancestors(agent_id, include_self=True))
        return context

    def flatten(self, expanded, allowed=None, *, max_depth=MAX_TREE_DEPTH, order_hint=None,
                parent_overrides=None, sort_key=None):
        """Return a deterministic visible projection of the tree.

        The underlying relationship tree is never truncated.  ``depth`` is
        returned for presentation; only visual indentation is capped by the UI.
        ``order_hint`` freezes sibling/root order while live badges update;
        newly discovered nodes sort after known siblings until explicit refresh.
        ``parent_overrides`` freezes only visual placement after a reparenting
        observation; subtree summaries still use the actual recorded edges.
        ``sort_key`` sorts natural root/sibling order; the UI uses it for the
        fixed working-first, terminated-last ordering.
        """
        expanded = expanded if isinstance(expanded, set) else set(expanded or ())
        allowed = set(self.nodes) if allowed is None else set(allowed)
        hint = order_hint or {}
        rows = []
        seen = set()
        parent_overrides = parent_overrides or {}
        effective_parent = {}
        for agent_id, node in self.nodes.items():
            actual = node.get("parent_agent_id")
            parent = parent_overrides.get(agent_id, actual)
            if (parent in self.nodes and parent != agent_id and
                    node.get("relationship_state") == "confirmed" and
                    node.get("relationship_kind") == "delegated"):
                effective_parent[agent_id] = parent
            else:
                effective_parent[agent_id] = None
        # A visual override can never introduce a cycle; fall back to the
        # recorded edge when an old placement would loop.
        for agent_id, parent in list(effective_parent.items()):
            current, visited = parent, set()
            while current:
                if current in visited or current == agent_id:
                    effective_parent[agent_id] = self.nodes[agent_id].get("parent_agent_id")
                    break
                visited.add(current)
                current = effective_parent.get(current)
        projection_children = {agent_id: [] for agent_id in self.nodes}
        projection_roots = []
        for agent_id, parent in effective_parent.items():
            if parent in projection_children:
                projection_children[parent].append(agent_id)
            else:
                projection_roots.append(agent_id)
        for node in self.nodes.values():
            if node.get("relationship_kind") != "delegated" or node.get("relationship_state") != "confirmed":
                if node["agent_id"] not in projection_roots:
                    projection_roots.append(node["agent_id"])

        def order_key(agent_id):
            natural = sort_key(agent_id) if sort_key else self.root_order_key(agent_id)
            return (hint.get(agent_id, len(hint) + 1), natural)

        def children_of(agent_id):
            return sorted((child for child in projection_children.get(agent_id, ())
                           if child in allowed and effective_parent.get(child) == agent_id),
                          key=order_key)

        def walk(agent_id, depth, ancestor_last):
            if agent_id in seen or len(rows) >= MAX_TRAVERSAL_NODES or depth > max_depth:
                return
            seen.add(agent_id)
            children = children_of(agent_id)
            rows.append({
                "agent_id": agent_id,
                "depth": depth,
                "children": children,
                "has_children": bool(children),
                "expanded": agent_id in expanded,
                "last": ancestor_last[-1] if ancestor_last else True,
                "ancestor_last": list(ancestor_last),
            })
            if agent_id in expanded:
                for index, child in enumerate(children):
                    walk(child, depth + 1, [*ancestor_last, index == len(children) - 1])

        roots = [root for root in projection_roots if root in allowed]
        roots.sort(key=order_key)
        for root in roots:
            walk(root, 0, [True])
        # Allowed fragments whose parent edge points outside the allowed set are
        # shown as unlinked roots rather than silently dropped.  A child merely
        # hidden by a collapsed ancestor must stay hidden.
        for agent_id in sorted(allowed, key=order_key):
            if agent_id in seen:
                continue
            node = self.nodes[agent_id]
            parent = node.get("parent_agent_id")
            if (parent in allowed and node.get("parent_available") and
                    node.get("relationship_state") == "confirmed" and
                    node.get("relationship_kind") == "delegated"):
                continue
            walk(agent_id, 0, [True])
        return rows

    def scope_ids(self, agent_id, include_subagents=True):
        if agent_id not in self.nodes:
            return []
        if not include_subagents:
            return [agent_id]
        return self.subtree_ids(agent_id, include_self=True)


def _merge_observation(previous, incoming):
    """Merge duplicate observations without inventing or silently resolving parents."""
    result = dict(previous)
    incoming = dict(incoming)
    observed = _is_observed(incoming)
    # Observed evidence is preferable to an announcement; otherwise keep the
    # latest observation but never discard a prior explicit parent claim.
    if observed or not _is_observed(previous):
        preferred, other = incoming, previous
    else:
        preferred, other = previous, incoming
    result.update(preferred)

    old_parent = previous.get("parent_agent_id")
    new_parent = incoming.get("parent_agent_id")
    old_confirmed = previous.get("relationship_state") == "confirmed"
    new_confirmed = incoming.get("relationship_state") == "confirmed"
    claims = _claims(previous.get("parent_claims")) + _claims(incoming.get("parent_claims"))
    if old_confirmed and old_parent:
        claims.append({"parent_agent_id": old_parent,
                       "provenance": previous.get("relationship_provenance") or "",
                       "observed_ns": _int(previous.get("stamp"))})
    if new_confirmed and new_parent:
        claims.append({"parent_agent_id": new_parent,
                       "provenance": incoming.get("relationship_provenance") or "",
                       "observed_ns": _int(incoming.get("stamp"))})
    unique = []
    for claim in claims:
        if claim not in unique:
            unique.append(claim)
    result["parent_claims"] = unique[-MAX_PARENT_CLAIMS:]
    if old_confirmed and new_confirmed and old_parent and new_parent and old_parent != new_parent:
        result["parent_agent_id"] = None
        result["relationship_state"] = "conflicted"
        result["relationship_provenance"] = (
            f"conflicting parents: {old_parent} vs {new_parent}; " +
            str(result.get("relationship_provenance") or ""))[:MAX_PROVENANCE]
        return result
    if previous.get("relationship_state") == "conflicted" or incoming.get("relationship_state") == "conflicted":
        result["relationship_state"] = "conflicted"
        result["parent_agent_id"] = None
        return result
    if previous.get("relationship_state") == "invalid" or incoming.get("relationship_state") == "invalid":
        result["relationship_state"] = "invalid"
        result["parent_agent_id"] = None
        return result
    # Prefer a confirmed relationship over an unresolved observation.
    if old_confirmed and not new_confirmed:
        result["parent_agent_id"] = old_parent
        result["relationship_state"] = "confirmed"
        result["relationship_kind"] = previous.get("relationship_kind")
        result["relationship_provenance"] = previous.get("relationship_provenance")
    elif new_confirmed:
        result["parent_agent_id"] = new_parent
        result["relationship_state"] = "confirmed"
        result["relationship_kind"] = incoming.get("relationship_kind")
        result["relationship_provenance"] = incoming.get("relationship_provenance")
    return result


def tree_prefix(depth, *, last=True, ancestor_last=(), ascii_only=False):
    """Draw a bounded tree prefix with an explicit depth fallback.

    Indentation is capped at eight levels.  Deeper nodes keep the relationship
    in their ``depth`` metadata and display a depth marker instead of clipping
    the tree itself.
    """
    depth = max(0, int(depth))
    glyph = {
        "branch": "|- " if ascii_only else "├─ ",
        "last": "`- " if ascii_only else "└─ ",
        "pipe": "|  " if ascii_only else "│  ",
        "deep": ".. " if ascii_only else "…  ",
    }
    cap = 8
    visible = min(depth, cap)
    prefix = ""
    if depth > cap:
        prefix = glyph["deep"]
        visible = cap
    for level in range(1, visible):
        is_last = ancestor_last[level] if level < len(ancestor_last) else False
        prefix += "   " if is_last else glyph["pipe"]
    if depth:
        prefix += glyph["last"] if last else glyph["branch"]
    return prefix


def branch_marker(expanded, has_children, *, ascii_only=False):
    if not has_children:
        return ""
    if ascii_only:
        return "v " if expanded else "> "
    return "▼ " if expanded else "▸ "
