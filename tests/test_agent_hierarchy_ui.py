"""Headless hierarchy presentation and navigation using synthetic sessions."""
import time
import unittest
from types import SimpleNamespace

from textual.widgets import DataTable, Input, Select, TabbedContent

from observatory.agent_hierarchy import canonical_agent_id
from observatory.terminal import Feed, Snapshot
from observatory.terminal_ui import ObservatoryApp, sources


def agent_node(native, parent, task, state="running", *, active=False, waiting="",
               stamp=None, runtime="Codex", service="codex", discovery="observed"):
    namespace = "codex-rollout" if runtime == "Codex" else "claude-code-projects"
    stamp = stamp if stamp is not None else time.time_ns()
    attention = state == "Waiting" and any(marker in waiting.casefold()
                                           for marker in ("input", "approval", "permission"))
    return {
        "source_id": "s-" + native, "id": "s-" + native,
        "agent_id": canonical_agent_id(runtime, namespace, native),
        "runtime": runtime, "service": service, "session_id": native,
        "source_namespace": namespace, "native_session_id": native,
        "native_worker_id": native if runtime == "Claude Code" else None,
        "parent_agent_id": canonical_agent_id(runtime, namespace, parent) if parent else None,
        "relationship_kind": "delegated" if parent else "unknown",
        "relationship_state": "confirmed", "relationship_provenance": "fixture exact relationship",
        "discovery_state": discovery, "coverage": "complete", "project": "checkout-api",
        "task": task, "run_state": state, "waiting_for": waiting, "stamp": stamp,
        "last_record_ns": stamp,
        "last_activity": {"time_unix_nano": stamp, "domain": "agent", "action": "update",
                          "subject": task, "result": "observed"},
        "active": active, "attention": attention,
    }


def hierarchy_snapshot():
    now = time.time_ns()
    root = agent_node("root", None, "Fix checkout regression", "running", active=True, stamp=now)
    waiting = agent_node("c1", "root", "Inspect API logs", "Waiting",
                         waiting="User input", stamp=now - 1_000_000)
    running = agent_node("c2", "root", "Review payment flow", "running",
                         active=True, stamp=now - 2_000_000)
    completed = agent_node("c3", "root", "Update regression tests", "completed", stamp=now - 3_000_000)
    grandchild = agent_node("g1", "c3", "Check retry cases", "running",
                            active=True, stamp=now - 4_000_000)
    activity = [{
        "id": "request-c1", "source_id": "request-c1", "service": "codex",
        "session_id": "c1", "time_unix_nano": now, "domain": "message", "action": "user",
        "subject": "Inspect API logs and decide whether retries should be bounded",
        "detail": "Inspect API logs and decide whether retries should be bounded",
        "result": "observed",
    }, {
        "id": "mcp-c1", "source_id": "mcp-c1", "service": "codex", "session_id": "c1",
        "time_unix_nano": now, "domain": "mcp", "action": "call", "subject": "github / search_issues",
        "mcp_server": "github", "mcp_tool": "search_issues", "result": "success",
        "trace_id": "1" * 32, "span_id": "1" * 16,
    }]
    return Snapshot(
        updated=time.time(),
        totals={"input_tokens": 10, "output_tokens": 5},
        local_status={"sessions": [root, waiting, running, completed, grandchild],
                      "runtime": {"processes": []}, "sources": 5,
                      "hierarchy": {"revision": "h1", "nodes": 5, "announced": 0}},
        activity=activity, mcp=[], findings=[], sessions=[], traces=[], agents=[])


class HierarchyFeed(Feed):
    def __init__(self, snapshot=None):
        super().__init__(SimpleNamespace(endpoint="http://example.invalid"))
        self.snapshot = snapshot or hierarchy_snapshot()
        self.thread = SimpleNamespace(is_alive=lambda: True)
        self.refreshes = 0

    def request_refresh(self):
        self.refreshes += 1


class HierarchyPresentationTests(unittest.IsolatedAsyncioTestCase):
    async def test_core_acceptance_tree_collapse_expand_inspect_and_stable_counts(self):
        feed = HierarchyFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(180, 48)) as pilot:
            await pilot.pause()
            app.set_agent_filter("all")
            await pilot.pause()
            table = app.query_one("#agent-table", DataTable)
            self.assertEqual(table.row_count, 5)
            rendered = "\n".join(table.get_row(key.value)[0].plain for key in table.rows)
            for task in ("Fix checkout regression", "Inspect API logs", "Review payment flow",
                         "Update regression tests", "Check retry cases"):
                self.assertEqual(rendered.count(task), 1)
            self.assertTrue(app.query_one("#stat-sessions").render().plain.startswith("3 working agents"))
            counts_before = app.query_one("#stat-sessions").render().plain
            attention_before = app.query_one("#stat-attention").render().plain

            # Collapse the root; counts describe nodes, not visible rows.
            app.selected_source = "s-root"
            app.refresh_data()
            await pilot.pause()
            app.action_tree_left()
            await pilot.pause()
            self.assertEqual(table.row_count, 1)
            self.assertIn("4 subagents", table.get_row("s-root")[0].plain)
            self.assertIn("1 needs attention", table.get_row("s-root")[0].plain)
            self.assertEqual(app.query_one("#stat-sessions").render().plain, counts_before)
            self.assertEqual(app.query_one("#stat-attention").render().plain, attention_before)

            # Expand and inspect the waiting worker's recorded evidence.
            app.action_tree_right()
            await pilot.pause()
            self.assertEqual(table.row_count, 5)
            app.select_agent("s-c1")
            await pilot.pause()
            self.assertEqual(app.selected_source, "s-c1")
            detail = app.query_one("#agent-detail").plain_text
            self.assertIn("NEEDS REPLY", detail)
            self.assertIn("Inspect API logs", detail)
            self.assertIn("recorded request", " ".join(detail.split()).casefold())

            # The completed parent keeps its own lifecycle outcome; the child is working.
            app.select_agent("s-c3")
            await pilot.pause()
            self.assertEqual(app.table_data["agent-table"]["s-c3"]["state"], "Completed")
            detail = app.query_one("#agent-detail").plain_text
            self.assertIn("direct children", detail.casefold())
            self.assertIn("Check retry cases", detail)
            health = next(item for item in sources(feed.snapshot) if item["source_id"] == "s-g1")
            self.assertTrue(health["active"])
            self.assertNotEqual(health["state"], app.table_data["agent-table"]["s-c3"]["state"])

            # Selection, branch expansion and counts survive a resize.
            app.selected_source = "s-c1"
            app.refresh_data()
            await pilot.resize_terminal(120, 30)
            await pilot.pause()
            self.assertEqual(app.selected_source, "s-c1")
            self.assertIn("s-c1", {key.value for key in table.rows})

    async def test_tree_siblings_follow_working_first_and_updated_sort(self):
        now = time.time_ns()
        root = agent_node("root-sort", None, "Root work", "running", active=True, stamp=now)
        newer = agent_node("child-new", "root-sort", "Newest worker", "running",
                           active=True, stamp=now - 1_000_000)
        older = agent_node("child-old", "root-sort", "Older worker", "running",
                           active=True, stamp=now - 2_000_000)
        waiting = agent_node("child-wait", "root-sort", "Waiting worker", "Waiting",
                             waiting="User input", stamp=now - 3_000_000)
        done = agent_node("child-done", "root-sort", "Terminated worker", "completed",
                          stamp=now)
        snapshot = Snapshot(
            updated=time.time(), totals={}, activity=[], mcp=[], findings=[],
            sessions=[], traces=[], agents=[],
            local_status={"sessions": [root, newer, older, waiting, done],
                          "runtime": {"processes": []}, "sources": 5,
                          "hierarchy": {"revision": "sort", "nodes": 5, "announced": 0}})
        app = ObservatoryApp(HierarchyFeed(snapshot), persist_preferences=False)
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            app.set_agent_filter("all")
            await pilot.pause()
            table = app.query_one("#agent-table", DataTable)
            self.assertEqual([key.value for key in table.rows],
                             ["s-root-sort", "s-child-new", "s-child-old",
                              "s-child-wait", "s-child-done"])
            app.action_toggle_sort()
            await pilot.pause()
            self.assertEqual([key.value for key in table.rows],
                             ["s-root-sort", "s-child-old", "s-child-new",
                              "s-child-wait", "s-child-done"])

    async def test_80x24_keeps_tree_navigation_and_inspector_reachable(self):
        feed = HierarchyFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(80, 24)) as pilot:
            await pilot.pause()
            app.set_agent_filter("all")
            await pilot.pause()
            table = app.query_one("#agent-table", DataTable)
            self.assertEqual(table.row_count, 5)
            header = app.query_one("#stat-sessions").render().plain
            self.assertIn("working agents", header)
            self.assertIn("task group", header)
            app.selected_source = "s-root"
            app.refresh_data()
            await pilot.pause()
            app.action_tree_left()
            await pilot.pause()
            self.assertEqual(table.row_count, 1)
            await pilot.press("right")
            await pilot.pause()
            self.assertEqual(table.row_count, 5)
            await pilot.press("enter")
            await pilot.pause()
            self.assertTrue(app.query_one("#agent-bottom").display)
            self.assertIn("Fix checkout regression", app.query_one("#agent-detail").plain_text)
            await pilot.press("escape")
            await pilot.pause()
            await pilot.resize_terminal(50, 18)
            await pilot.pause()
            self.assertEqual(table.row_count, 5)
            await pilot.press("enter")
            await pilot.pause()
            self.assertTrue(app.query_one("#agent-bottom").display)
            self.assertIn("Fix checkout regression", app.query_one("#agent-detail").plain_text)

    async def test_search_reveals_ancestor_context_and_flat_mode_is_reversible(self):
        feed = HierarchyFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause()
            app.set_agent_filter("all")
            await pilot.pause()
            app.query_one("#search-agents", Input).value = "retry"
            await pilot.pause()
            table = app.query_one("#agent-table", DataTable)
            keys = {key.value for key in table.rows}
            self.assertEqual(keys, {"s-root", "s-c3", "s-g1"})
            self.assertIn("matching session", app.query_one("#search-status-agents").render().plain)
            app.query_one("#search-agents", Input).value = ""
            await pilot.pause()
            self.assertEqual(table.row_count, 5)

            app.action_toggle_tree()
            await pilot.pause()
            self.assertFalse(app.agent_tree_active)
            self.assertEqual(table.row_count, 5)
            self.assertNotIn("├─", "\n".join(table.get_row(key.value)[0].plain for key in table.rows))
            app.action_toggle_tree()
            await pilot.pause()
            self.assertTrue(app.agent_tree_active)

    async def test_new_child_stays_under_parent_and_does_not_reorder_other_roots(self):
        feed = HierarchyFeed()
        second = agent_node("root-two", None, "Independent release review", "completed",
                            stamp=time.time_ns() - 10 ** 12)
        feed.snapshot.local_status["sessions"].append(second)
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause()
            app.set_agent_filter("all")
            await pilot.pause()
            table = app.query_one("#agent-table", DataTable)
            before = [key.value for key in table.rows]
            self.assertEqual(before[-1], "s-root-two")
            newer = agent_node("new-child", "root", "New delegated check", "running",
                               active=True, stamp=time.time_ns())
            feed.snapshot.local_status["sessions"].append(newer)
            app.last_update = -1
            app.refresh_data()
            await pilot.pause()
            keys = [key.value for key in table.rows]
            self.assertIn("s-new-child", keys)
            self.assertEqual(keys[-1], "s-root-two")
            self.assertLess(keys.index("s-new-child"), keys.index("s-root-two"))

    async def test_mouse_disclosure_toggles_without_opening_or_selecting_the_row(self):
        feed = HierarchyFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause()
            app.set_agent_filter("all")
            await pilot.pause()
            table = app.query_one("#agent-table", DataTable)
            self.assertEqual(table.row_count, 5)
            selected = app.selected_source
            await pilot.click("#agent-table", offset=(2, 2))
            await pilot.pause()
            self.assertEqual(table.row_count, 1)
            self.assertFalse(app.query_one("#agents").has_class("expanded"))
            self.assertEqual(app.selected_source, selected)
            await pilot.click("#agent-table", offset=(2, 2))
            await pilot.pause()
            self.assertEqual(table.row_count, 5)
            self.assertFalse(app.query_one("#agents").has_class("expanded"))

    async def test_reparenting_waits_for_explicit_reorder_and_preserves_selection(self):
        feed = HierarchyFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause()
            app.set_agent_filter("all")
            await pilot.pause()
            self.assertEqual(app.table_data["agent-table"]["s-c1"]["_tree_depth"], 1)
            app.select_agent("s-c1")
            worker = next(item for item in feed.snapshot.local_status["sessions"]
                          if item["source_id"] == "s-c1")
            worker["parent_agent_id"] = canonical_agent_id("Codex", "codex-rollout", "c3")
            app.last_update = -1
            app.refresh_data()
            await pilot.pause()
            self.assertEqual(app.selected_source, "s-c1")
            self.assertEqual(app.table_data["agent-table"]["s-c1"]["_tree_depth"], 1)
            self.assertTrue(app.query_one("#refresh-order").display)
            await pilot.click("#refresh-order")
            await pilot.pause()
            self.assertEqual(app.table_data["agent-table"]["s-c1"]["_tree_depth"], 2)
            self.assertEqual(app.selected_source, "s-c1")

    async def test_evidence_scope_switch_propagates_canonical_identity(self):
        feed = HierarchyFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause()
            app.set_agent_filter("all")
            await pilot.pause()
            app.query_one("#scope", Select).value = "s-root"
            await pilot.pause()
            app.query_one("#evidence-scope", Select).value = "subtree"
            await pilot.pause()
            self.assertTrue(feed.scope_subtree)
            self.assertEqual(feed.scope_agent_id,
                             canonical_agent_id("Codex", "codex-rollout", "root"))
            app.action_page("activity")
            await pilot.pause()
            self.assertTrue(app.query_one("#controls").has_class("evidence-active"))
            self.assertIn("subtree", app.scope_label())

    async def test_collapsing_ancestor_with_selected_child_selects_ancestor(self):
        feed = HierarchyFeed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause()
            app.set_agent_filter("all")
            await pilot.pause()
            app.select_agent("s-g1")
            await pilot.pause()
            app.toggle_agent_branch("s-root")
            await pilot.pause()
            self.assertEqual(app.selected_source, "s-root")
            self.assertIn("Fix checkout regression", app.query_one("#agent-detail").plain_text)


class ReviewAndAnnouncementTests(unittest.TestCase):
    def test_hidden_review_parent_does_not_leak_into_child_row_or_inspector(self):
        snapshot = hierarchy_snapshot()
        review = agent_node("review", None, "Approval review", "completed")
        review.update(service="codex-review", runtime="Codex")
        child = agent_node("review-child", "review", "Review child", "running", active=True)
        snapshot.local_status["sessions"].extend([review, child])
        visible = sources(snapshot, include_reviews=False)
        self.assertNotIn("review", {item["session_id"] for item in visible})
        self.assertIn("review-child", {item["session_id"] for item in visible})
        self.assertNotIn("Approval review", str(visible))

    def test_retained_parent_context_does_not_create_a_monitored_row(self):
        snapshot = hierarchy_snapshot()
        child = next(item for item in snapshot.local_status["sessions"]
                     if item["source_id"] == "s-c1")
        outside_parent = canonical_agent_id("Codex", "codex-rollout", "outside-window")
        child["parent_agent_id"] = outside_parent
        child["parent_context"] = {"agent_id": outside_parent, "retained": True,
                                   "outside_current_window": True}
        rows = sources(snapshot)
        self.assertEqual(len(rows), 5)
        self.assertEqual(sum(item.get("agent_id") == outside_parent for item in rows), 0)
        self.assertTrue(next(item for item in rows if item["source_id"] == "s-c1")
                        ["parent_context"]["retained"])

    def test_announced_spawn_is_not_working_and_says_status_unavailable(self):
        snapshot = hierarchy_snapshot()
        announced = agent_node("announced", "root", "Spawn recorded", "announced",
                               discovery="announced")
        announced["run_state"] = "announced"
        announced["task"] = "Spawn recorded"
        snapshot.local_status["sessions"].append(announced)
        rows = sources(snapshot)
        row = next(item for item in rows if item["source_id"] == "s-announced")
        self.assertEqual(row["state"], "Spawn recorded")
        self.assertFalse(row["active"])
        self.assertEqual(row["status_evidence"], "Spawn recorded · status unavailable")
