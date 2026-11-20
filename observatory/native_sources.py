"""Declarative native session discovery; parsing and persistence remain separate."""
from __future__ import annotations

import os
import importlib
import pkgutil
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
    """One source format's discovery rules and required format adapter."""

    id: str
    runtime: str
    root: Callable[[], Path]
    accepts_file: Callable[[str | Path], bool] = lambda _path: True
    accepts_directory: Callable[[str], bool] = lambda _name: True
    adapter_factory: Callable[[], NativeSessionAdapter] | None = None
    empty_status_extension: Callable[[], dict] | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.id, str) or not re.fullmatch(r"[a-z][a-z0-9_.-]*", self.id):
            raise ValueError("invalid native source ID")
        if (not isinstance(self.runtime, str) or not self.runtime.strip() or
                not all(map(callable, (self.root, self.accepts_file, self.accepts_directory)))):
            raise ValueError("invalid native source specification")
        if not callable(self.adapter_factory):
            raise ValueError("native source requires an adapter factory")
        if self.empty_status_extension is not None and not callable(self.empty_status_extension):
            raise ValueError("native source empty status extension must be callable")

    def build_adapter(self) -> NativeSessionAdapter:
        adapter = self.adapter_factory()
        if not all(callable(getattr(adapter, name, None)) for name in
                   ("initial_task", "parse", "activity", "observe_state")):
            raise ValueError(f"native source {self.id} has an invalid adapter")
        return adapter


def bundled_adapter_modules() -> tuple[object, ...]:
    """Import only modules in Agenturm's trusted installed adapter package."""
    from . import adapters

    return tuple(importlib.import_module(f"{adapters.__name__}.{item.name}")
                 for item in pkgutil.iter_modules(adapters.__path__))


def builtin_sources() -> tuple[NativeSourceSpec, ...]:
    """Load trusted bundled source factories without a runtime dispatch table.

    Independently installed adapters use explicit process-plugin config;
    project-local Python files are never imported by this discovery path.
    """
    declared = []
    for module in bundled_adapter_modules():
        module_name = module.__name__.rsplit(".", 1)[-1]
        factory = getattr(module, "source_spec", None)
        if factory is None:
            continue
        if not callable(factory):
            raise ValueError(f"invalid bundled source factory in {module_name}")
        spec = factory()
        if not isinstance(spec, NativeSourceSpec):
            raise ValueError(f"invalid bundled source declaration in {module_name}")
        priority = getattr(module, "SOURCE_PRIORITY", 100)
        if type(priority) is not int or not 0 <= priority <= 1000:
            raise ValueError(f"invalid bundled source priority in {module_name}")
        declared.append((priority, spec.id, spec))
    declared.sort(key=lambda item: (item[0], item[1]))
    specs = tuple(item[2] for item in declared)
    if len({spec.id for spec in specs}) != len(specs):
        raise ValueError("duplicate bundled source ID")
    return specs
