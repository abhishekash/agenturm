"""Declarative native session discovery; parsing and persistence remain separate."""
from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Callable


KIRO_SKIPPED_DIRECTORIES = frozenset({"cli", "snapshots", ".index"})


def kiro_home() -> Path:
    """Kiro honours an absolute KIRO_HOME and otherwise uses ~/.kiro."""
    configured = os.environ.get("KIRO_HOME", "").strip()
    if configured and Path(configured).is_absolute() and Path(configured).resolve() != Path(configured).resolve().parent:
        return Path(configured)
    return Path.home() / ".kiro"


def kiro_source(path: str | Path) -> bool:
    """Accept only the current Kiro session layout, never legacy cli/ histories."""
    path = Path(path)
    if path.name == "messages.jsonl":
        return path.parent.name.startswith("sess_")
    return path.parent.name == "sub-executions" and path.parent.parent.name.startswith("sess_")


@dataclass(frozen=True)
class NativeSourceSpec:
    """One source format's discovery rules, evaluated on every discovery pass."""

    id: str
    runtime: str
    root: Callable[[], Path]
    accepts_file: Callable[[str | Path], bool] = lambda _path: True
    accepts_directory: Callable[[str], bool] = lambda _name: True

    def __post_init__(self) -> None:
        if not isinstance(self.id, str) or not re.fullmatch(r"[a-z][a-z0-9_.-]*", self.id):
            raise ValueError("invalid native source ID")
        if (not isinstance(self.runtime, str) or not self.runtime.strip() or
                not all(map(callable, (self.root, self.accepts_file, self.accepts_directory)))):
            raise ValueError("invalid native source specification")


def builtin_sources() -> tuple[NativeSourceSpec, ...]:
    return (
        NativeSourceSpec("codex", "Codex", lambda: Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex"))) / "sessions"),
        NativeSourceSpec("claude", "Claude Code", lambda: Path(os.environ.get("CLAUDE_CONFIG_DIR", str(Path.home() / ".claude"))) / "projects"),
        NativeSourceSpec("pi", "Pi", lambda: Path(os.environ.get("PI_CODING_AGENT_DIR", str(Path.home() / ".pi" / "agent"))) / "sessions"),
        NativeSourceSpec("kiro", "Kiro", lambda: kiro_home() / "sessions", kiro_source,
                         lambda name: name not in KIRO_SKIPPED_DIRECTORIES),
    )
