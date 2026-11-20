"""Trusted native ingest scopes shared by Store admission and capture fences.

Only installed bundled adapter modules can contribute these declarations.
They do not grant an independently installed process plugin native ownership.
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass


_SCOPE = re.compile(r"[a-z][a-z0-9_.]{0,79}\Z")
_LEGACY_SCOPES = ("observatory.local_agents", "observatory.opencode")
_OWNER_TRIGGERS = tuple(
    f"native_capture_{kind}_{action}_v1"
    for kind in ("span", "log") for action in ("insert", "update", "delete"))


@dataclass(frozen=True)
class NativeIngestScope:
    name: str

    def __post_init__(self) -> None:
        if type(self.name) is not str or not _SCOPE.fullmatch(self.name):
            raise ValueError("invalid native ingest scope")


@dataclass(frozen=True)
class NativeIngestScopes:
    names: tuple[str, ...]

    def __post_init__(self) -> None:
        names = self.names
        if (type(names) is not tuple or not 1 <= len(names) <= 16
                or any(type(name) is not str or not _SCOPE.fullmatch(name) for name in names)
                or len(set(names)) != len(names)):
            raise ValueError("invalid native ingest scope registry")
        object.__setattr__(self, "names", tuple(sorted(names)))

    @classmethod
    def bundled(cls) -> NativeIngestScopes:
        from .native_sources import bundled_adapter_modules

        names = []
        for module in bundled_adapter_modules():
            factory = getattr(module, "native_ingest_scopes", None)
            if factory is None:
                continue
            if not callable(factory):
                raise ValueError("invalid native ingest scope declaration")
            declared = factory()
            if (type(declared) is not tuple or len(declared) > 16
                    or any(not isinstance(item, NativeIngestScope) for item in declared)):
                raise ValueError("invalid native ingest scope declaration")
            names.extend(item.name for item in declared)
        return cls(tuple(names))

    def contains(self, name: object) -> bool:
        return type(name) is str and name in self.names

    @property
    def sql(self) -> str:
        # The validated grammar excludes quotes, whitespace and SQL syntax.
        return ",".join(f"'{name}'" for name in self.names)

    @property
    def marker(self) -> str:
        return json.dumps(self.names, separators=(",", ":"))

    def require_current(self, db) -> None:
        row = db.execute(
            "SELECT value FROM schema_meta WHERE key='native_capture_scopes_v1'").fetchone()
        if row is None or row[0] != self.marker:
            raise ValueError("native ingest scope registry changed; restart this Store")

    def install_capture_fences(self, db) -> None:
        """Atomically replace scope-sensitive triggers on each Store startup.

        On a scope-set change, prior owner claims are cleared before new
        triggers become visible. Re-ingestion can establish new claims.
        """
        current = self.marker
        row = db.execute("SELECT value FROM schema_meta WHERE key='native_capture_scopes_v1'").fetchone()
        prior = row[0] if row else json.dumps(_LEGACY_SCOPES, separators=(",", ":"))
        present = {item[0] for item in db.execute(
            "SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'native_capture_%_v1'")}
        complete = set(_OWNER_TRIGGERS) <= present
        if row is not None and prior == current and complete:
            return
        clear_owners = prior != current or not complete
        guards = self.sql
        ddl = """
            CREATE TABLE IF NOT EXISTS native_capture_owner_v1 (
                kind TEXT NOT NULL, owner_key TEXT NOT NULL, source_id TEXT NOT NULL,
                PRIMARY KEY(kind,owner_key));
        """
        for name in _OWNER_TRIGGERS:
            ddl += f"DROP TRIGGER IF EXISTS {name};\n"
        ddl += f"""
            CREATE TRIGGER native_capture_span_insert_v1 AFTER INSERT ON spans
                WHEN NEW.scope_name IN ({guards}) BEGIN
                DELETE FROM native_capture_owner_v1
                WHERE kind='span' AND owner_key=NEW.trace_id||':'||NEW.span_id; END;
            CREATE TRIGGER native_capture_span_update_v1 AFTER UPDATE ON spans
                WHEN OLD.scope_name IN ({guards}) OR NEW.scope_name IN ({guards}) BEGIN
                DELETE FROM native_capture_owner_v1 WHERE kind='span'
                AND owner_key IN (OLD.trace_id||':'||OLD.span_id,NEW.trace_id||':'||NEW.span_id); END;
            CREATE TRIGGER native_capture_span_delete_v1 AFTER DELETE ON spans
                WHEN OLD.scope_name IN ({guards}) BEGIN
                DELETE FROM native_capture_owner_v1
                WHERE kind='span' AND owner_key=OLD.trace_id||':'||OLD.span_id; END;
            CREATE TRIGGER native_capture_log_insert_v1 AFTER INSERT ON logs
                WHEN NEW.scope_name IN ({guards}) BEGIN
                DELETE FROM native_capture_owner_v1 WHERE kind='log' AND owner_key=NEW.digest; END;
            CREATE TRIGGER native_capture_log_update_v1 AFTER UPDATE ON logs
                WHEN OLD.scope_name IN ({guards}) OR NEW.scope_name IN ({guards}) BEGIN
                DELETE FROM native_capture_owner_v1 WHERE kind='log'
                AND owner_key IN (OLD.digest,NEW.digest); END;
            CREATE TRIGGER native_capture_log_delete_v1 AFTER DELETE ON logs
                WHEN OLD.scope_name IN ({guards}) BEGIN
                DELETE FROM native_capture_owner_v1 WHERE kind='log' AND owner_key=OLD.digest; END;
        """
        if clear_owners:
            ddl += "DELETE FROM native_capture_owner_v1;\n"
        ddl += ("INSERT OR REPLACE INTO schema_meta(key,value) "
                f"VALUES('native_capture_scopes_v1','{current}');\n")
        # executescript commits any earlier schema setup before BEGIN. This
        # DDL, claim reset and marker then commit as one SQLite transaction.
        db.executescript("BEGIN IMMEDIATE;\n" + ddl + "COMMIT;")
