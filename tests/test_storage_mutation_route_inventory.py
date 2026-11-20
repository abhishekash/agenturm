"""Review gate for Python SQLite entrypoints before any Rust writer cutover.

This is an inventory, not runtime enforcement. It deliberately tracks both
mutating and read-only connection sites, since a new raw connection can become
a writer without passing through a Store handoff fence.
"""

from __future__ import annotations

import ast
from collections import Counter
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]
INVENTORY = Path(__file__).with_name("storage_mutation_routes.json")
METHODS = frozenset({"_connect", "_transaction", "_write_db", "_source_write"})
CONSTRUCTORS = frozenset({"Store", "WorkflowStore", "TaskStore", "OutboundQueue"})
AUTHORITIES = frozenset({"telemetry", "workflow", "tasks", "outbound", "offline", "read_only"})
EFFECTS = frozenset({"may_write", "read_only", "backup_target", "external_effect"})
CLI_ROUTES = {
    "observatory/__main__.py": {
        "cockpit": "may_write", "serve": "may_write", "migrate-db": "backup_target",
        "run": "external_effect", "import-jsonl": "may_write",
        "import-evals": "may_write", "prune": "may_write",
    },
    "observatory/attention_cli.py": {
        "attention": "may_write", "watch": "may_write", "status": "read_only",
    },
    "observatory/investigation_cli.py": {"investigate": "read_only"},
    "observatory/review_cli.py": {"review": "read_only"},
    "observatory/task_cli.py": {
        "task": "may_write", "draft": "external_effect",
        "approve-draft": "may_write", "list": "read_only", "create": "may_write",
        "show": "read_only", "update": "may_write", "link": "may_write",
        "unlink": "may_write", "snapshot": "external_effect",
        "handoff": "external_effect", "brief": "may_write", "decide": "may_write",
        "decisions": "read_only", "backup": "backup_target", "archive": "may_write",
    },
    "observatory/native_lifecycle.py": {
        "native": "may_write", "bind": "external_effect", "send": "may_write",
        "hook": "may_write", "retry": "may_write",
    },
}


def inventory_rows() -> dict:
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate inventory key: {key}")
            result[key] = value
        return result

    return json.loads(INVENTORY.read_text(encoding="utf-8"), object_pairs_hook=unique)


def scan_tree(tree: ast.AST, path: str) -> tuple[Counter[tuple[str, str, str]], list[str]]:
    """Return (path, lexical owner, access API) counts and unreviewed aliases."""
    parents = {child: parent for parent in ast.walk(tree)
               for child in ast.iter_child_nodes(parent)}
    sites: Counter[tuple[str, str, str]] = Counter()
    aliases: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for name in node.names:
                if name.name in {"aiosqlite", "apsw"} or (
                    name.name == "sqlite3" and name.asname not in (None, "sqlite3")
                ):
                    aliases.append(f"{path}:{node.lineno}: unreviewed SQLite import")
        elif isinstance(node, ast.ImportFrom):
            if node.module in {"aiosqlite", "apsw"} or (
                node.module == "sqlite3"
                and any(name.name in {"connect", "dbapi2", "*"} for name in node.names)
            ):
                aliases.append(f"{path}:{node.lineno}: unreviewed SQLite import")
            if any(name.name in CONSTRUCTORS and name.asname not in (None, name.name)
                   for name in node.names):
                aliases.append(f"{path}:{node.lineno}: unreviewed store constructor alias")
        if not isinstance(node, ast.Call):
            continue
        called = node.func
        api = None
        if isinstance(called, ast.Attribute):
            if (called.attr == "connect" and isinstance(called.value, ast.Name)
                    and called.value.id == "sqlite3"):
                api = "sqlite3.connect"
            elif called.attr in METHODS:
                api = called.attr
            elif called.attr in CONSTRUCTORS:
                api = f"{called.attr}()"
        elif isinstance(called, ast.Name) and called.id in CONSTRUCTORS:
            api = f"{called.id}()"
        if api is None:
            continue
        owners = []
        parent = node
        while parent in parents:
            parent = parents[parent]
            if isinstance(parent, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                owners.append(parent.name)
        sites[(path, ".".join(reversed(owners)) or "<module>", api)] += 1
    return sites, aliases


def scan_repository() -> tuple[Counter[tuple[str, str, str]], list[str]]:
    sites: Counter[tuple[str, str, str]] = Counter()
    aliases: list[str] = []
    for path in sorted((ROOT / "observatory").rglob("*.py")):
        relative = path.relative_to(ROOT).as_posix()
        found, found_aliases = scan_tree(ast.parse(path.read_text(encoding="utf-8")), relative)
        sites.update(found)
        aliases.extend(found_aliases)
    return sites, aliases


def scan_cli_routes(path: str) -> tuple[set[str], list[str]]:
    tree = ast.parse((ROOT / path).read_text(encoding="utf-8"))
    parents = {child: parent for parent in ast.walk(tree)
               for child in ast.iter_child_nodes(parent)}
    names: set[str] = set()
    unknown: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        func = node.func
        parser_call = isinstance(func, ast.Attribute) and func.attr == "add_parser"
        task_action = (path.endswith("/task_cli.py")
                       and isinstance(func, ast.Name) and func.id == "action")
        if not (parser_call or task_action) or not node.args:
            continue
        argument = node.args[0]
        if isinstance(argument, ast.Constant) and type(argument.value) is str:
            names.add(argument.value)
            continue
        if task_action:
            unknown.append(f"{path}:{node.lineno}: dynamic task action")
            continue
        if path.endswith("/task_cli.py") and isinstance(argument, ast.Name) and argument.id == "name":
            # The local action() helper is covered by its literal call sites.
            continue
        parent = node
        while parent in parents:
            parent = parents[parent]
            if (isinstance(parent, ast.For) and isinstance(parent.target, ast.Name)
                    and isinstance(argument, ast.Name) and parent.target.id == argument.id
                    and isinstance(parent.iter, (ast.Tuple, ast.List))
                    and all(isinstance(item, ast.Constant) and type(item.value) is str
                            for item in parent.iter.elts)):
                names.update(item.value for item in parent.iter.elts)
                break
        else:
            unknown.append(f"{path}:{node.lineno}: dynamic parser route")
    return names, unknown


class StorageMutationRouteInventoryTests(unittest.TestCase):
    def test_scan_finds_new_connection_and_writer_sites(self):
        tree = ast.parse("""\
import sqlite3
def new_route():
    store = Store('a.db')
    with store._source_write(), store._connect() as db:
        db.execute('DELETE FROM spans')
    sqlite3.connect('other.db')
""")
        sites, aliases = scan_tree(tree, "observatory/new_route.py")
        self.assertEqual(aliases, [])
        self.assertEqual(sites, Counter({
            ("observatory/new_route.py", "new_route", "Store()"): 1,
            ("observatory/new_route.py", "new_route", "_source_write"): 1,
            ("observatory/new_route.py", "new_route", "_connect"): 1,
            ("observatory/new_route.py", "new_route", "sqlite3.connect"): 1,
        }))
        _, aliases = scan_tree(ast.parse("from sqlite3 import connect"), "new.py")
        self.assertEqual(len(aliases), 1)
        _, aliases = scan_tree(ast.parse("from .core import Store as HiddenStore"), "new.py")
        self.assertEqual(len(aliases), 1)

    def test_every_access_site_has_reviewed_classification(self):
        rows = inventory_rows()
        expected: Counter[tuple[str, str, str]] = Counter()
        for site, classification in rows.items():
            key = tuple(site.split(" | "))
            self.assertEqual(len(key), 3, f"invalid inventory key: {site}")
            self.assertEqual(set(classification), {"count", "authority", "effect"})
            self.assertIn(classification["authority"], AUTHORITIES)
            self.assertIn(classification["effect"], EFFECTS)
            self.assertIs(type(classification["count"]), int)
            self.assertGreater(classification["count"], 0)
            self.assertNotIn(key, expected, f"duplicate inventory site: {key}")
            expected[key] = classification["count"]
        actual, aliases = scan_repository()
        self.assertEqual(aliases, [], "new SQLite API aliases need review")
        unexpected = sorted((key, count) for key, count in (actual - expected).items())
        missing = sorted((key, count) for key, count in (expected - actual).items())
        self.assertEqual((unexpected, missing), ([], []),
                         f"classify new/changed storage access sites; unexpected={unexpected}, missing={missing}")

    def test_cutover_risks_are_explicitly_in_inventory(self):
        rows = inventory_rows()
        for key in (
            ("observatory/core.py", "Store._connect", "sqlite3.connect"),
            ("observatory/workflow_store.py", "WorkflowStore._transaction", "sqlite3.connect"),
            ("observatory/saved_tasks.py", "TaskStore._connect", "sqlite3.connect"),
            ("observatory/saved_tasks.py", "_backup_task_database", "sqlite3.connect"),
            ("observatory/receiver_state.py", "migrate_database", "sqlite3.connect"),
            ("observatory/outbound_delivery.py", "OutboundQueue._dispatch_one_locked", "_connect"),
            ("observatory/native_lifecycle.py", "flush", "_transaction"),
            ("observatory/__main__.py", "main.local_store", "Store()"),
            ("observatory/server.py", "serve", "Store()"),
            ("observatory/task_cli.py", "run_task_command", "TaskStore()"),
        ):
            site = " | ".join(key)
            self.assertIn(site, rows)
            self.assertNotEqual(rows[site]["effect"], "read_only")

    def test_cli_commands_are_classified_before_new_mutation_routes(self):
        for path, classified in CLI_ROUTES.items():
            self.assertTrue(set(classified.values()) <= EFFECTS)
            actual, unknown = scan_cli_routes(path)
            self.assertEqual(unknown, [])
            self.assertEqual(actual, set(classified), f"classify changed CLI routes in {path}")


if __name__ == "__main__":
    unittest.main()
