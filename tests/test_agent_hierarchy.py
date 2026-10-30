"""Hierarchy identity, evidence, graph projection, storage, and scoped queries."""
import io
import json
import os
import sqlite3
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.agent_hierarchy import (CAPABILITIES, HierarchyGraph, branch_marker,
                                         canonical_agent_id, normalize_node, provenance,
                                         tree_prefix)
from observatory.core import Store, normalize_native


def agent_id(native, runtime="Codex", namespace="codex-rollout"):
    return canonical_agent_id(runtime, namespace, native)


def node(native, parent=None, *, runtime="Codex", namespace="codex-rollout", state="running",
         active=False, attention=False, stamp=100, discovery="observed", kind=None,
         relation_state="confirmed", coverage="complete", task=None):
    parent_id = agent_id(parent, runtime, namespace) if parent else None
    result = {
        "agent_id": agent_id(native, runtime, namespace), "runtime": runtime,
        "source_namespace": namespace, "source_id": "source-" + native,
        "native_session_id": native, "native_worker_id": native,
        "parent_agent_id": parent_id,
        "relationship_kind": kind or ("delegated" if parent else "unknown"),
        "relationship_state": relation_state,
        "relationship_provenance": provenance("fixture", "exact metadata", stamp),
        "discovery_state": discovery, "coverage": coverage,
        "state": state, "active": active, "attention": attention, "stamp": stamp,
        "observed_ns": stamp, "task": task or native,
    }
    return result


class HierarchyGraphTests(unittest.TestCase):
    def test_duplicate_observations_merge_and_nested_summary_excludes_announced(self):
        root = node("root", state="completed", stamp=10)
        child = node("child", "root", state="running", active=True, stamp=20)
        grandchild = node("grandchild", "child", state="Waiting", attention=True, stamp=30)
        duplicate = dict(child, discovery_state="observed", stamp=21)
        graph = HierarchyGraph([root, child, duplicate, grandchild])
        self.assertEqual(graph.diagnostics["duplicates"], 1)
        self.assertEqual(graph.summary(agent_id("root"))["working"], 1)
        self.assertEqual(graph.summary(agent_id("root"))["waiting"], 1)
        self.assertEqual(graph.summary(agent_id("root"))["attention"], 1)
        self.assertEqual(graph.summary(agent_id("root"))["last_activity"], 30)
        self.assertEqual(node("announced", "root", discovery="announced", stamp=99)["discovery_state"], "announced")
        announced = node("announced", "root", discovery="announced", stamp=99)
        graph = HierarchyGraph([root, child, announced])
        summary = graph.summary(agent_id("root"))
        self.assertEqual(summary["announced"], 1)
        self.assertEqual(summary["working"], 1)
        self.assertEqual(summary["observed"], 1)

    def test_fork_is_not_a_delegated_edge(self):
        root = node("root")
        fork = node("fork", "root", kind="forked", relation_state="confirmed")
        graph = HierarchyGraph([root, fork])
        self.assertEqual(graph.children_of(agent_id("root")), [])
        self.assertIn(agent_id("fork"), graph.roots())

    def test_conflicting_parent_claims_are_unlinked_with_bounded_claims(self):
        child = node("child", "parent-a")
        competing = node("child", "parent-b")
        graph = HierarchyGraph([node("parent-a"), node("parent-b"), child, competing])
        merged = graph.nodes[agent_id("child")]
        self.assertEqual(merged["relationship_state"], "conflicted")
        self.assertIsNone(merged["parent_agent_id"])
        self.assertIn(agent_id("child"), graph.roots())
        self.assertLessEqual(len(merged["parent_claims"]), 4)

    def test_self_link_cycle_and_missing_parent_do_not_hide_nodes(self):
        self_link = node("self", "self")
        a = node("a", "b")
        b = node("b", "a")
        missing = node("missing", "not-observed")
        graph = HierarchyGraph([self_link, a, b, missing])
        self.assertEqual(graph.nodes[agent_id("self")]["relationship_state"], "invalid")
        self.assertEqual(graph.nodes[agent_id("a")]["relationship_state"], "invalid")
        self.assertEqual(graph.nodes[agent_id("b")]["relationship_state"], "invalid")
        self.assertFalse(graph.nodes[agent_id("missing")]["parent_available"])
        self.assertIn(agent_id("missing"), graph.roots())
        self.assertEqual(len(graph.nodes), 4)

    def test_flatten_respects_collapse_and_search_context(self):
        root = node("root")
        child = node("child", "root", state="running", active=True)
        grandchild = node("grandchild", "child", state="waiting", attention=True)
        graph = HierarchyGraph([root, child, grandchild])
        collapsed = graph.flatten(set(), set(graph.nodes))
        self.assertEqual([row["agent_id"] for row in collapsed], [agent_id("root")])
        expanded = graph.flatten({agent_id("root")}, set(graph.nodes))
        self.assertEqual({row["agent_id"] for row in expanded},
                         {agent_id("root"), agent_id("child")})
        context = graph.context_for_matches({agent_id("grandchild")})
        rows = graph.flatten({agent_id("root"), agent_id("child")}, context)
        self.assertEqual([row["depth"] for row in rows], [0, 1, 2])
        self.assertEqual(len({row["agent_id"] for row in rows}), 3)

    def test_order_hint_keeps_selected_identity_and_appends_new_children(self):
        root = node("root")
        first = node("first", "root", stamp=10)
        second = node("second", "root", stamp=20)
        graph = HierarchyGraph([root, first, second])
        hinted = graph.flatten({agent_id("root")}, order_hint={agent_id("root"): 0,
                                                               agent_id("second"): 1,
                                                               agent_id("first"): 2})
        self.assertEqual([row["agent_id"] for row in hinted],
                         [agent_id("root"), agent_id("second"), agent_id("first")])
        newer = node("newer", "root", stamp=5)
        graph = HierarchyGraph([root, first, second, newer])
        hinted = graph.flatten({agent_id("root")}, order_hint={agent_id("root"): 0,
                                                               agent_id("second"): 1,
                                                               agent_id("first"): 2})
        self.assertEqual([row["agent_id"] for row in hinted],
                         [agent_id("root"), agent_id("second"), agent_id("first"),
                          agent_id("newer")])

    def test_tree_prefix_caps_visual_indent_and_supports_ascii(self):
        prefix = tree_prefix(20, last=True, ancestor_last=[False] * 20)
        self.assertTrue(prefix.startswith("…"))
        self.assertLessEqual(len(prefix), 32)
        ascii_prefix = tree_prefix(2, last=True, ancestor_last=[True, True], ascii_only=True)
        self.assertIn("`-", ascii_prefix)
        self.assertEqual(branch_marker(False, False), "")
        self.assertEqual(branch_marker(True, True, ascii_only=True), "v ")

    def test_thousand_node_forest_uses_one_adjacency_index(self):
        nodes = []
        roots = []
        for group in range(10):
            root = f"root-{group}"
            roots.append(root)
            nodes.append(node(root, stamp=group))
            for child in range(10):
                name = f"child-{group}-{child}"
                nodes.append(node(name, root, state="running", active=True, stamp=100 + group))
                for leaf in range(9):
                    nodes.append(node(f"leaf-{group}-{child}-{leaf}", name,
                                      state="completed", stamp=200 + group))
        started = time.perf_counter()
        graph = HierarchyGraph(nodes)
        elapsed = time.perf_counter() - started
        self.assertEqual(len(graph.nodes), 1010)
        self.assertEqual(graph.summary(agent_id("root-0"))["observed"], 100)
        rows = graph.flatten(set(), set(graph.nodes))
        self.assertEqual(len(rows), 10)
        self.assertLess(elapsed, 5.0)
        for root in roots:
            self.assertTrue(graph.children_of(agent_id(root)))

    def test_capability_matrix_is_explicit_about_unverified_support(self):
        self.assertIn("Codex", CAPABILITIES)
        self.assertIn("Claude Code", CAPABILITIES)
        self.assertIn("limitations", CAPABILITIES["Pi"])
        self.assertTrue(CAPABILITIES["Codex"]["independent_child_evidence"])
        self.assertIn("parent_thread_id", CAPABILITIES["Codex"]["delegation_linking"])


class HierarchyStorageTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.store = Store(self.root / "hierarchy.db")
        self.addCleanup(self.store.close)

    def test_idempotent_upsert_child_before_parent_and_revision_stability(self):
        child = node("child", "parent", stamp=20)
        self.assertEqual(self.store.upsert_agent_nodes([child]), 1)
        revision = self.store.hierarchy_revision()
        rows = self.store.agent_nodes()
        self.assertFalse(rows[0]["parent_available"])
        self.assertEqual(self.store.upsert_agent_nodes([child]), 0)
        self.assertEqual(self.store.hierarchy_revision(), revision)
        self.store.upsert_agent_nodes([node("parent", stamp=10)])
        rows = {row["native_session_id"]: row for row in self.store.agent_nodes()}
        self.assertTrue(rows["child"]["parent_available"])
        self.assertEqual(rows["child"]["relationship_state"], "confirmed")

    def test_announced_node_reconciles_with_observed_child_without_duplicate(self):
        announced = node("child", "parent", discovery="announced", stamp=10)
        self.store.upsert_agent_nodes([node("parent"), announced])
        self.assertEqual(len(self.store.agent_nodes()), 2)
        observed = node("child", "parent", stamp=20)
        self.store.upsert_agent_nodes([observed])
        rows = self.store.agent_nodes()
        self.assertEqual(len(rows), 2)
        child = next(row for row in rows if row["native_session_id"] == "child")
        self.assertEqual(child["discovery_state"], "observed")
        self.assertEqual(child["coverage"], "complete")

    def test_conflicting_parent_claims_are_not_resolved_by_arrival_order(self):
        self.store.upsert_agent_nodes([node("parent-a"), node("parent-b"),
                                       node("child", "parent-a")])
        self.store.upsert_agent_nodes([node("child", "parent-b")])
        child = next(row for row in self.store.agent_nodes()
                     if row["native_session_id"] == "child")
        self.assertEqual(child["relationship_state"], "conflicted")
        self.assertIsNone(child["parent_agent_id"])
        self.assertEqual({claim["parent_agent_id"] for claim in child["parent_claims"]},
                         {agent_id("parent-a"), agent_id("parent-b")})
        self.assertIn(agent_id("child"), self.store.resolve_agent_scope(
            agent_id("child"), include_subagents=False)["agent_ids"])

    def test_late_resolution_and_retention_keep_child_with_unavailable_parent(self):
        day = 86_400_000_000_000
        parent = node("parent", stamp=1)
        child = node("child", "parent", stamp=9 * day + 12 * 3_600_000_000_000)
        self.store.upsert_agent_nodes([parent, child])
        self.store.prune(1, now_ns=10 * day)
        rows = {row["native_session_id"]: row for row in self.store.agent_nodes()}
        self.assertNotIn("parent", rows)
        self.assertIn("child", rows)
        self.assertFalse(rows["child"]["parent_available"])
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM agent_identity").fetchone()[0], 1)

    def test_scoped_queries_isolate_colliding_native_session_ids(self):
        def span(trace, span_id, native, session="shared", service="svc"):
            return normalize_native({
                "trace_id": trace, "span_id": span_id, "parent_span_id": None,
                "name": "agent.run", "start_unix_nano": 1, "end_unix_nano": 2,
                "status": "OK",
                "attributes": {"agent.id": native, "agent.runtime": "SDK",
                               "agent.namespace": service, "session.id": session,
                               "agent.task": native},
                "resource_attributes": {"service.name": service},
                "scope_name": "fixture"}, service)
        self.store.ingest([span("1" * 32, "1" * 16, "root"),
                           span("2" * 32, "2" * 16, "other")])
        self.store.ingest([span("3" * 32, "3" * 16, "child", session="child-session")])
        # Link the child explicitly using a second observation with parent metadata.
        self.store.ingest([normalize_native({
            "trace_id": "4" * 32, "span_id": "4" * 16, "parent_span_id": None,
            "name": "agent.run", "start_unix_nano": 1, "end_unix_nano": 2,
            "status": "OK",
            "attributes": {"agent.id": "child", "agent.parent_id": "root",
                           "agent.runtime": "SDK", "agent.namespace": "svc",
                           "session.id": "child-session", "agent.task": "child"},
            "resource_attributes": {"service.name": "svc"},
            "scope_name": "fixture"}, "svc")])
        root = canonical_agent_id("SDK", "svc", "root")
        other = canonical_agent_id("SDK", "svc", "other")
        scoped = self.store.trace_page(agent_id=root, subtree=True)["traces"]
        self.assertEqual({trace["trace_id"] for trace in scoped}, {"1" * 32, "3" * 32, "4" * 32})
        exact = self.store.trace_page(agent_id=other, subtree=False)["traces"]
        self.assertEqual([trace["trace_id"] for trace in exact], ["2" * 32])
        # Same native session id, isolated by canonical agent evidence.
        self.assertNotIn("2" * 32, {trace["trace_id"] for trace in scoped})

    def test_export_scope_metadata_is_only_added_when_requested(self):
        from observatory.ui_export import investigation_json
        record = {'id': 'event-1', 'subject': 'recorded'}
        plain = json.loads(investigation_json('activity', record))
        self.assertNotIn('hierarchy_scope', plain)
        scoped = json.loads(investigation_json('activity', record, {
            'mode': 'subtree', 'root_agent_id': agent_id('root')}))
        self.assertEqual(scoped['hierarchy_scope']['mode'], 'subtree')
        self.assertEqual(scoped['hierarchy_scope']['root_agent_id'], agent_id('root'))
        self.assertIn('not a family-wide export', scoped['hierarchy_scope']['note'])

    def test_parent_context_lookup_is_bounded_and_structural(self):
        parent = node("parent", stamp=10)
        child = node("child", "parent", stamp=20)
        self.store.upsert_agent_nodes([parent, child])
        contexts = self.store.agent_node_contexts(
            [agent_id("parent"), agent_id("not-present"), "invalid", agent_id("parent")])
        self.assertEqual(set(contexts), {agent_id("parent")})
        context = contexts[agent_id("parent")]
        self.assertTrue(context["retained"])
        self.assertTrue(context["outside_current_window"])
        self.assertNotIn("task", context)
        self.assertNotIn("provenance_payload", json.dumps(context))

    def test_sessions_keep_shared_session_workers_as_separate_agents(self):
        def span(trace, span_id, native):
            return normalize_native({
                "trace_id": trace, "span_id": span_id, "parent_span_id": None,
                "name": "agent.run", "start_unix_nano": 1, "end_unix_nano": 2,
                "status": "OK",
                "attributes": {"agent.id": native, "agent.runtime": "SDK",
                               "agent.namespace": "svc", "session.id": "shared",
                               "agent.task": native},
                "resource_attributes": {"service.name": "svc"},
                "scope_name": "fixture"}, "svc")
        self.store.ingest([span("1" * 32, "1" * 16, "worker-a"),
                           span("2" * 32, "2" * 16, "worker-b")])
        sessions = self.store.sessions()
        self.assertEqual({item["session_id"] for item in sessions}, {"shared"})
        self.assertEqual(len(sessions), 2)
        self.assertEqual({item["agent_id"] for item in sessions},
                         {canonical_agent_id("SDK", "svc", "worker-a"),
                          canonical_agent_id("SDK", "svc", "worker-b")})

    def test_existing_database_opens_and_gets_additive_columns(self):
        path = self.root / "old.db"
        with sqlite3.connect(path) as db:
            db.executescript("""
                CREATE TABLE spans (trace_id TEXT NOT NULL, span_id TEXT NOT NULL,
                    parent_span_id TEXT, name TEXT NOT NULL, start_ns INTEGER NOT NULL,
                    end_ns INTEGER NOT NULL, status TEXT NOT NULL, service TEXT NOT NULL,
                    attributes TEXT NOT NULL, events TEXT NOT NULL,
                    PRIMARY KEY(trace_id,span_id));
                CREATE TABLE logs (digest TEXT PRIMARY KEY, trace_id TEXT, span_id TEXT,
                    time_ns INTEGER NOT NULL, service TEXT NOT NULL,
                    severity_number INTEGER NOT NULL, severity_text TEXT NOT NULL,
                    event_name TEXT NOT NULL, body TEXT NOT NULL, attributes TEXT NOT NULL,
                    resource_attributes TEXT NOT NULL, scope_name TEXT NOT NULL);
                INSERT INTO spans VALUES('1','2',NULL,'agent.run',1,2,'OK','svc','{}','[]');
            """)
        store = Store(path)
        try:
            with store._connect() as db:
                span_columns = {row[1] for row in db.execute("PRAGMA table_info(spans)")}
                log_columns = {row[1] for row in db.execute("PRAGMA table_info(logs)")}
                self.assertIn("agent_id", span_columns)
                self.assertIn("agent_id", log_columns)
                self.assertEqual(db.execute("SELECT COUNT(*) FROM spans").fetchone()[0], 1)
        finally:
            store.close()


class HierarchyParserTests(unittest.TestCase):
    def setUp(self):
        from observatory.core import Store
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.store = Store(self.root / "parser.db")
        self.addCleanup(self.store.close)

    def append(self, session, rows):
        with session.path.open("a") as stream:
            for row in rows:
                stream.write(json.dumps(row) + "\n")
        session.poll()

    def test_codex_parent_child_grandchild_and_spawn_announcement(self):
        from observatory.local_agents import Session
        parent = self.root / "parent.jsonl"
        child = self.root / "child.jsonl"
        grandchild = self.root / "grandchild.jsonl"
        for path in (parent, child, grandchild):
            path.touch()
        parent_session = Session(self.store, parent, "Codex")
        child_session = Session(self.store, child, "Codex")
        grandchild_session = Session(self.store, grandchild, "Codex")
        self.append(parent_session, [
            {"type": "session_meta", "timestamp": "2026-01-01T00:00:00Z",
             "payload": {"id": "parent", "session_id": "parent", "cwd": "/work", "source": "cli"}},
            {"type": "response_item", "timestamp": "2026-01-01T00:00:01Z",
             "payload": {"type": "function_call", "call_id": "spawn-1", "name": "spawn_agent",
                         "arguments": "{}"}},
            {"type": "response_item", "timestamp": "2026-01-01T00:00:02Z",
             "payload": {"type": "function_call_output", "call_id": "spawn-1",
                         "output": json.dumps({"agent_id": "child", "nickname": "Hubble"})}},
        ])
        self.append(child_session, [
            {"type": "session_meta", "timestamp": "2026-01-01T00:00:03Z",
             "payload": {"id": "child", "session_id": "parent", "parent_thread_id": "parent",
                         "source": {"subagent": {"thread_spawn": {
                             "parent_thread_id": "parent", "depth": 1}}}}}])
        self.append(grandchild_session, [
            {"type": "session_meta", "timestamp": "2026-01-01T00:00:04Z",
             "payload": {"id": "grandchild", "session_id": "parent", "parent_thread_id": "child",
                         "source": {"subagent": {"thread_spawn": {
                             "parent_thread_id": "child", "depth": 2}}}}}])
        self.assertEqual(parent_session.announced_agents[agent_id("child")]["discovery_state"], "announced")
        self.assertEqual(child_session.parent_agent_id, agent_id("parent"))
        self.assertEqual(grandchild_session.parent_agent_id, agent_id("child"))
        self.assertEqual(child_session.session_id, "child")
        self.assertNotEqual(child_session.session_id, parent_session.session_id)

    def test_failed_spawn_and_fork_do_not_create_delegation(self):
        from observatory.local_agents import Session
        path = self.root / "fork.jsonl"
        path.touch()
        session = Session(self.store, path, "Codex")
        self.append(session, [
            {"type": "session_meta", "timestamp": "2026-01-01T00:00:00Z",
             "payload": {"id": "fork", "session_id": "root", "forked_from_id": "root",
                         "source": "cli"}},
            {"type": "response_item", "timestamp": "2026-01-01T00:00:01Z",
             "payload": {"type": "function_call", "call_id": "spawn-fail", "name": "spawn_agent"}},
            {"type": "response_item", "timestamp": "2026-01-01T00:00:02Z",
             "payload": {"type": "function_call_output", "call_id": "spawn-fail",
                         "output": json.dumps({"error": "denied"})}},
        ])
        self.assertEqual(session.announced_agents, {})
        self.assertEqual(session.relationship_kind, "forked")
        self.assertEqual(session.relationship_state, "confirmed")
        self.assertNotEqual(session.relationship_kind, "delegated")

    def test_older_codex_subagent_without_parent_is_unresolved_not_root_claim(self):
        from observatory.local_agents import Session
        path = self.root / "old.jsonl"
        path.touch()
        session = Session(self.store, path, "Codex")
        self.append(session, [{"type": "session_meta", "timestamp": "2026-01-01T00:00:00Z",
            "payload": {"id": "old", "session_id": "root",
                        "source": {"subagent": "review"}}}])
        self.assertEqual(session.relationship_state, "unresolved")
        self.assertIsNone(session.parent_agent_id)

    def test_claude_shared_session_workers_and_nested_sidecar(self):
        from observatory.local_agents import Session
        project = self.root / "projects" / "p"
        main = project / "main.jsonl"
        main.parent.mkdir(parents=True)
        main.touch()
        sub = project / "main" / "subagents"
        sub.mkdir(parents=True)
        child = sub / "agent-child.jsonl"
        child.touch()
        (sub / "agent-child.meta.json").write_text(json.dumps({"toolUseId": "tool-1",
                                                                "agentType": "Explore"}))
        nested = sub / "agent-nested.jsonl"
        nested.touch()
        (sub / "agent-nested.meta.json").write_text(json.dumps({"parentAgentId": "child"}))
        main_session = Session(self.store, main, "Claude Code")
        child_session = Session(self.store, child, "Claude Code")
        nested_session = Session(self.store, nested, "Claude Code")
        stamp = "2026-01-01T00:00:00Z"
        self.append(main_session, [{"type": "user", "uuid": "u1", "sessionId": "main",
                                    "timestamp": stamp, "message": {"content": "Parent"}}])
        self.append(child_session, [{"type": "user", "uuid": "u2", "sessionId": "main",
                                     "agentId": "child", "timestamp": stamp,
                                     "message": {"content": "Child"}}])
        self.append(nested_session, [{"type": "user", "uuid": "u3", "sessionId": "main",
                                      "agentId": "nested", "timestamp": stamp,
                                      "message": {"content": "Nested"}}])
        self.assertEqual(main_session.session_id, "main")
        self.assertEqual(child_session.session_id, "child")
        self.assertEqual(child_session.parent_agent_id, agent_id("main", "Claude Code", "claude-code-projects"))
        self.assertEqual(nested_session.parent_agent_id, agent_id("child", "Claude Code", "claude-code-projects"))
        self.assertNotEqual(child_session.session_id, main_session.session_id)

    def test_claude_successful_task_result_announces_exact_child(self):
        from observatory.local_agents import Session
        path = self.root / "main.jsonl"
        path.touch()
        session = Session(self.store, path, "Claude Code")
        stamp = "2026-01-01T00:00:00Z"
        self.append(session, [
            {"type": "user", "uuid": "u1", "sessionId": "main", "timestamp": stamp,
             "message": {"content": "Parent"}},
            {"type": "assistant", "sessionId": "main", "timestamp": stamp,
             "message": {"content": [{"type": "tool_use", "id": "tool-1", "name": "Task",
                                      "input": {"description": "private"}}]}},
            {"type": "user", "sessionId": "main", "timestamp": stamp,
             "toolUseResult": {"agentId": "child-worker"},
             "message": {"content": [{"type": "tool_result", "tool_use_id": "tool-1",
                                      "content": "done"}]}},
        ])
        self.assertIn(agent_id("child-worker", "Claude Code", "claude-code-projects"),
                      session.announced_agents)
        self.assertNotIn("private", json.dumps(session.announced_agents))

    def test_opencode_parent_id_links_child_session_and_orphan_stays_unresolved(self):
        from observatory.opencode import OpenCodeReader
        path = self.root / "opencode.db"
        now_ms = int(time.time() * 1000)
        with sqlite3.connect(path) as db, db:
            db.executescript("""
                CREATE TABLE session(id TEXT PRIMARY KEY, project_id TEXT, parent_id TEXT,
                    title TEXT, directory TEXT, agent TEXT, model TEXT, time_created INTEGER,
                    time_updated INTEGER);
                CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                    time_updated INTEGER, data TEXT);
            """)
            db.executemany("INSERT INTO session VALUES(?,?,?,?,?,?,?,?,?)", [
                ("root", None, None, "Parent", "/work", "build", "{}", now_ms, now_ms),
                ("child", None, "root", "Child", "/work", "explore", "{}", now_ms, now_ms),
                ("orphan", None, None, "Orphan", "/work", "build", "{}", now_ms, now_ms),
            ])
            db.execute("INSERT INTO message VALUES(?,?,?,?,?)",
                       ("m1", "orphan", now_ms, now_ms,
                        json.dumps({"role": "user", "text": "work"})))
        reader = OpenCodeReader(self.store, path=path)
        reader.poll()
        sessions = {item["session_id"]: item for item in reader.status_sessions()}
        self.assertEqual(sessions["root"]["relationship_state"], "confirmed")
        self.assertIsNone(sessions["root"]["parent_agent_id"])
        self.assertEqual(sessions["child"]["relationship_state"], "confirmed")
        self.assertEqual(sessions["child"]["parent_agent_id"],
                         sessions["root"]["agent_id"])

    def test_pi_without_verified_parent_metadata_is_unlinked(self):
        from observatory.local_agents import Session
        path = self.root / "pi.jsonl"
        path.touch()
        session = Session(self.store, path, "Pi")
        self.append(session, [{"type": "session", "id": "pi-1", "cwd": "/work"}])
        self.assertEqual(session.relationship_state, "unresolved")
        self.assertIsNone(session.parent_agent_id)
        explicit = self.root / "pi-child.jsonl"
        explicit.touch()
        child = Session(self.store, explicit, "Pi")
        self.append(child, [{"type": "session", "id": "pi-2", "parentSessionId": "pi-1",
                             "cwd": "/work"}])
        self.assertEqual(child.relationship_kind, "delegated")
        self.assertEqual(child.relationship_state, "confirmed")
        self.assertEqual(child.parent_agent_id,
                         canonical_agent_id("Pi", "pi-sessions", "pi-1"))


class HierarchyIntegrationTests(unittest.TestCase):
    def test_native_discovery_persists_and_serves_scope_without_duplicates(self):
        from observatory.local_agents import LocalAgents
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sessions = root / "codex" / "sessions" / "2026" / "01" / "01"
            sessions.mkdir(parents=True)
            parent = sessions / "rollout-parent.jsonl"
            child = sessions / "rollout-child.jsonl"
            parent.write_text("\n".join(json.dumps(row) for row in [
                {"type": "session_meta", "timestamp": "2026-01-01T00:00:00Z",
                 "payload": {"id": "parent", "session_id": "parent", "cwd": "/work",
                             "source": "cli"}},
                {"type": "event_msg", "timestamp": "2026-01-01T00:00:01Z",
                 "payload": {"type": "task_started", "turn_id": "t1"}},
            ]) + "\n")
            child.write_text("\n".join(json.dumps(row) for row in [
                {"type": "session_meta", "timestamp": "2026-01-01T00:00:02Z",
                 "payload": {"id": "child", "session_id": "parent",
                             "parent_thread_id": "parent", "cwd": "/work",
                             "source": {"subagent": {"thread_spawn": {
                                 "parent_thread_id": "parent", "depth": 1}}}}},
                {"type": "event_msg", "timestamp": "2026-01-01T00:00:03Z",
                 "payload": {"type": "task_started", "turn_id": "t2"}},
            ]) + "\n")
            store = Store(root / "integration.db")
            agents = LocalAgents(store)
            environment = {"CODEX_HOME": str(root / "codex"),
                           "CLAUDE_CONFIG_DIR": str(root / "missing-claude"),
                           "PI_CODING_AGENT_DIR": str(root / "missing-pi")}
            with patch.dict(os.environ, environment):
                agents.discover()
                agents.poll()
                status = agents.status()
            try:
                rows = {(row["native_worker_id"] or row["native_session_id"]): row
                        for row in store.agent_nodes()}
                self.assertEqual(set(rows), {"parent", "child"})
                self.assertEqual(rows["child"]["parent_agent_id"], rows["parent"]["agent_id"])
                self.assertTrue(rows["child"]["parent_available"])
                self.assertEqual(status["hierarchy"]["nodes"], 2)
                scope = store.resolve_agent_scope(rows["parent"]["agent_id"], True)
                self.assertEqual(len(scope["agent_ids"]), 2)
                sessions = {item["session_id"]: item for item in status["sessions"]}
                self.assertEqual(sessions["child"]["parent_agent_id"],
                                 rows["parent"]["agent_id"])
                self.assertEqual(sessions["child"]["relationship_state"], "confirmed")
            finally:
                agents.close()
                store.close()

    def test_claude_subagent_discovery_links_main_and_child_end_to_end(self):
        from observatory.local_agents import LocalAgents
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "claude" / "projects" / "checkout"
            subagents = project / "main-session" / "subagents"
            subagents.mkdir(parents=True)
            stamp = "2026-01-01T00:00:00Z"
            (project / "main-session.jsonl").write_text(json.dumps({
                "type": "user", "uuid": "u1", "sessionId": "main-session",
                "timestamp": stamp, "message": {"content": "Parent task"}}) + "\n")
            (subagents / "agent-worker.jsonl").write_text(json.dumps({
                "type": "user", "uuid": "u2", "sessionId": "main-session",
                "agentId": "worker", "timestamp": stamp,
                "message": {"content": "Child task"}}) + "\n")
            (subagents / "agent-worker.meta.json").write_text(json.dumps({
                "agentType": "Explore", "toolUseId": "tool-1"}))
            store = Store(root / "claude.db")
            agents = LocalAgents(store)
            environment = {"CODEX_HOME": str(root / "missing-codex"),
                           "CLAUDE_CONFIG_DIR": str(root / "claude"),
                           "PI_CODING_AGENT_DIR": str(root / "missing-pi")}
            with patch.dict(os.environ, environment):
                agents.discover()
                agents.poll()
                status = agents.status()
            try:
                rows = {(row["native_worker_id"] or row["native_session_id"]): row
                        for row in store.agent_nodes()}
                self.assertIn("main-session", rows)
                self.assertIn("worker", rows)
                self.assertEqual(rows["worker"]["parent_agent_id"],
                                 rows["main-session"]["agent_id"])
                sessions = {item["session_id"]: item for item in status["sessions"]}
                self.assertEqual(sessions["worker"]["relationship_state"], "confirmed")
                self.assertEqual(sessions["worker"]["parent_agent_id"],
                                 sessions["main-session"]["agent_id"])
            finally:
                agents.close()
                store.close()


if __name__ == "__main__":
    unittest.main()

