"""Declarative native session discovery; parsing and persistence remain separate."""
from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from .native_adapter import NativeSessionAdapter


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
    adapter_factory: Callable[[], NativeSessionAdapter] | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.id, str) or not re.fullmatch(r"[a-z][a-z0-9_.-]*", self.id):
            raise ValueError("invalid native source ID")
        if (not isinstance(self.runtime, str) or not self.runtime.strip() or
                not all(map(callable, (self.root, self.accepts_file, self.accepts_directory)))):
            raise ValueError("invalid native source specification")
        if self.adapter_factory is not None and not callable(self.adapter_factory):
            raise ValueError("invalid native source adapter")

    def build_adapter(self) -> NativeSessionAdapter | None:
        if self.adapter_factory is None:
            return None
        adapter = self.adapter_factory()
        if not all(callable(getattr(adapter, name, None)) for name in
                   ("initial_task", "parse", "activity", "observe_state")):
            raise ValueError(f"native source {self.id} has an invalid adapter")
        return adapter


def builtin_sources() -> tuple[NativeSourceSpec, ...]:
    from .adapters.codex import CodexAdapter
    from .adapters.claude import ClaudeAdapter
    from .adapters.pi import PiAdapter
    return (
        NativeSourceSpec("codex", "Codex", lambda: Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex"))) / "sessions",
                         adapter_factory=CodexAdapter),
        NativeSourceSpec("claude", "Claude Code", lambda: Path(os.environ.get("CLAUDE_CONFIG_DIR", str(Path.home() / ".claude"))) / "projects",
                         adapter_factory=ClaudeAdapter),
        NativeSourceSpec("pi", "Pi", lambda: Path(os.environ.get("PI_CODING_AGENT_DIR", str(Path.home() / ".pi" / "agent"))) / "sessions",
                         adapter_factory=PiAdapter),
        NativeSourceSpec("kiro", "Kiro", lambda: kiro_home() / "sessions", kiro_source,
                         lambda name: name not in KIRO_SKIPPED_DIRECTORIES),
    )
