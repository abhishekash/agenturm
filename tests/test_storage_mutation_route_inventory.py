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


def _sqlite_callable(node: ast.AST) -> str | None:
    """Recognize the standard module and its public dbapi2 spelling."""
    if not isinstance(node, ast.Attribute) or node.attr not in {"connect", "Connection"}:
        return None
    owner = node.value
    if (isinstance(owner, ast.Name) and owner.id == "sqlite3"
            or isinstance(owner, ast.Attribute) and owner.attr == "dbapi2"
            and isinstance(owner.value, ast.Name) and owner.value.id == "sqlite3"):
        return f"sqlite3.{node.attr}"
    return None


def scan_tree(tree: ast.AST, path: str) -> tuple[Counter[tuple[str, str, str]], list[str]]:
    """Return (path, lexical owner, access API) counts and unreviewed aliases."""
    parents = {child: parent for parent in ast.walk(tree)
               for child in ast.iter_child_nodes(parent)}
    sites: Counter[tuple[str, str, str]] = Counter()
    aliases: list[str] = []
    imported: dict[ast.AST, dict[str, str]] = {}

    def scope(node: ast.AST) -> ast.AST:
        current = parents.get(node)
        while current is not None and not isinstance(
            current, (ast.Module, ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)
        ):
            current = parents.get(current)
        return current or tree

    def imported_api(node: ast.AST, name: str) -> str | None:
        current = scope(node)
        while current is not None:
            found = imported.get(current, {}).get(name)
            if found is not None:
                return found
            if isinstance(current, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
                arguments = current.args
                parameters = (*arguments.posonlyargs, *arguments.args,
                              *arguments.kwonlyargs)
                if name in {item.arg for item in parameters} or (
                    arguments.vararg is not None and arguments.vararg.arg == name
                ) or (arguments.kwarg is not None and arguments.kwarg.arg == name):
                    return None
            current = scope(current) if current is not tree else None
        return None

    def in_annotation(node: ast.AST) -> bool:
        current = node
        while current in parents:
            parent = parents[current]
            if ((isinstance(parent, (ast.FunctionDef, ast.AsyncFunctionDef))
                 and parent.returns is current)
                    or (isinstance(parent, ast.arg) and parent.annotation is current)
                    or (isinstance(parent, ast.AnnAssign) and parent.annotation is current)):
                return True
            current = parent
        return False

    # Resolve direct `from sqlite3 import Connection` calls within their
    # lexical scope. Type-only imports remain valid; calls become sites.
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and node.module in {"sqlite3", "sqlite3.dbapi2"}:
            for name in node.names:
                if name.name in {"connect", "Connection"}:
                    imported.setdefault(scope(node), {})[name.asname or name.name] = (
                        f"sqlite3.{name.name}")
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for name in node.names:
                if (name.name.split(".")[0] in {"aiosqlite", "apsw"}
                        or name.name.startswith("sqlite3.") and name.asname is not None
                        or name.name == "sqlite3" and name.asname not in (None, "sqlite3")):
                    aliases.append(f"{path}:{node.lineno}: unreviewed SQLite import")
        elif isinstance(node, ast.ImportFrom):
            if (node.module or "").split(".")[0] in {"aiosqlite", "apsw"} or (
                node.module in {"sqlite3", "sqlite3.dbapi2"}
                and any(name.name in {"connect", "dbapi2", "*"} for name in node.names)):
                aliases.append(f"{path}:{node.lineno}: unreviewed SQLite import")
            if node.module in {"sqlite3", "sqlite3.dbapi2"}:
                for name in node.names:
                    if name.name == "Connection" and name.asname not in (None, "Connection"):
                        aliases.append(f"{path}:{node.lineno}: unreviewed SQLite constructor alias")
            if any(name.name in CONSTRUCTORS and name.asname not in (None, name.name)
                   for name in node.names):
                aliases.append(f"{path}:{node.lineno}: unreviewed store constructor alias")
        elif isinstance(node, (ast.Assign, ast.AnnAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            if any(isinstance(target, ast.Name) and imported_api(node, target.id)
                   for target in targets):
                aliases.append(f"{path}:{node.lineno}: imported SQLite callable reassigned")
            if isinstance(node.value, ast.Name) and imported_api(node, node.value.id):
                aliases.append(f"{path}:{node.lineno}: indirect SQLite callable assignment")
        elif isinstance(node, ast.Attribute) and _sqlite_callable(node) and not in_annotation(node):
            parent = parents.get(node)
            if not isinstance(parent, ast.Call) or parent.func is not node:
                aliases.append(f"{path}:{node.lineno}: indirect SQLite callable reference")
        if not isinstance(node, ast.Call):
            continue
        called = node.func
        api = None
        if isinstance(called, ast.Attribute):
            api = _sqlite_callable(called)
            if api is None and called.attr in METHODS:
                api = called.attr
            elif api is None and called.attr in CONSTRUCTORS:
                api = f"{called.attr}()"
        elif isinstance(called, ast.Name):
            imported_call = imported_api(node, called.id)
            if imported_call is not None:
                api = imported_call
            elif called.id in CONSTRUCTORS:
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

    def test_sqlite_constructor_and_indirect_imports_cannot_hide_opens(self):
        for source, api in (
            ("import sqlite3\ndef open_db(): return sqlite3.Connection('x.db')",
             "sqlite3.Connection"),
            ("from sqlite3 import Connection\ndef open_db(): return Connection('x.db')",
             "sqlite3.Connection"),
            ("import sqlite3\ndef open_db(): return sqlite3.dbapi2.connect('x.db')",
             "sqlite3.connect"),
        ):
            with self.subTest(source=source):
                sites, aliases = scan_tree(ast.parse(source), "new.py")
                self.assertEqual(aliases, [])
                self.assertEqual(sites[("new.py", "open_db", api)], 1)

        for source in (
            "import sqlite3\nopener = sqlite3.connect\nopener('x.db')",
            "import sqlite3\ndef supplier(): return sqlite3.connect",
            "from sqlite3.dbapi2 import connect\nconnect('x.db')",
            "import sqlite3.dbapi2 as db\ndb.connect('x.db')",
            "from sqlite3 import Connection as C\nC('x.db')",
            "from sqlite3 import Connection\nConnection = object\nConnection('x.db')",
        ):
            with self.subTest(source=source):
                _, aliases = scan_tree(ast.parse(source), "new.py")
                self.assertTrue(aliases, "indirect SQLite open must require review")

        # A function-local import must not classify an unrelated scope's call.
        sites, aliases = scan_tree(ast.parse(
            "def one():\n    from sqlite3 import Connection\n    return Connection('x.db')\n"
            "def two(Connection):\n    return Connection('not sqlite')\n"), "new.py")
        self.assertEqual(aliases, [])
        self.assertEqual(sites, Counter({("new.py", "one", "sqlite3.Connection"): 1}))
        sites, aliases = scan_tree(ast.parse(
            "from sqlite3 import Connection\n"
            "def other(Connection): return Connection('not sqlite')\n"), "new.py")
        self.assertEqual((sites, aliases), (Counter(), []))

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
