"""Trusted, bounded contributions to the existing agent session inspector.

Providers return text only. The shell owns layout, selection and rendering;
an adapter cannot add widgets, actions or new view dispatch here.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Callable


_ID = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")
MAX_PROVIDERS = 8
MAX_ENTRIES = 4
MAX_VALUE = 131072


@dataclass(frozen=True)
class SessionInspectorModel:
    overview_fields: tuple[tuple[str, str], ...] = ()
    detail_sections: tuple[tuple[str, str], ...] = ()


@dataclass(frozen=True)
class SessionInspectorProvider:
    id: str
    matches: Callable[[dict], bool]
    render: Callable[[dict, dict], SessionInspectorModel]


def _safe_entry(entry: object) -> bool:
    if not isinstance(entry, tuple) or len(entry) != 2:
        return False
    label, value = entry
    return (isinstance(label, str) and 0 < len(label) <= 64
            and all(ord(char) >= 32 and ord(char) != 127 for char in label)
            and isinstance(value, str) and 0 < len(value) <= MAX_VALUE
            and all(ord(char) >= 32 or char in "\n\t" for char in value))


def without_label_collisions(entries, reserved):
    """Keep contributed entries whose labels do not repeat shell-owned labels."""
    seen = {label.strip().casefold() for label in reserved}
    result = []
    for label, value in entries:
        key = label.strip().casefold()
        if key not in seen:
            result.append((label, value))
            seen.add(key)
    return tuple(result)


class SessionInspectorRegistry:
    """Compose trusted sections without letting one faulty provider break UI."""

    def __init__(self, providers: tuple[SessionInspectorProvider, ...] = ()):
        if len(providers) > MAX_PROVIDERS:
            raise ValueError("too many inspector providers")
        self._providers: dict[str, SessionInspectorProvider] = {}
        for provider in providers:
            self.register(provider)

    def register(self, provider: SessionInspectorProvider) -> None:
        if (not isinstance(provider, SessionInspectorProvider)
                or not isinstance(provider.id, str) or not _ID.fullmatch(provider.id)
                or provider.id in self._providers or len(self._providers) >= MAX_PROVIDERS
                or not callable(provider.matches) or not callable(provider.render)):
            raise ValueError("invalid inspector provider")
        self._providers[provider.id] = provider

    def for_session(self, item: dict, local_status: dict) -> SessionInspectorModel:
        if not isinstance(item, dict) or not isinstance(local_status, dict):
            return SessionInspectorModel()
        fields: list[tuple[str, str]] = []
        sections: list[tuple[str, str]] = []
        field_labels: set[str] = set()
        section_labels: set[str] = set()
        for provider in self._providers.values():
            try:
                if provider.matches(item) is not True:
                    continue
                model = provider.render(item, local_status)
            except Exception:
                continue
            if (not isinstance(model, SessionInspectorModel)
                    or not isinstance(model.overview_fields, tuple)
                    or not isinstance(model.detail_sections, tuple)
                    or len(model.overview_fields) > MAX_ENTRIES
                    or len(model.detail_sections) > MAX_ENTRIES
                    or not all(_safe_entry(entry) for entry in
                               (*model.overview_fields, *model.detail_sections))):
                continue
            for label, value in model.overview_fields:
                key = label.strip().casefold()
                if key not in field_labels and len(fields) < MAX_ENTRIES:
                    fields.append((label, value))
                    field_labels.add(key)
            for label, value in model.detail_sections:
                key = label.strip().casefold()
                if key not in section_labels and len(sections) < MAX_ENTRIES:
                    sections.append((label, value))
                    section_labels.add(key)
        return SessionInspectorModel(tuple(fields), tuple(sections))
