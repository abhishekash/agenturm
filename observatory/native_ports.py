"""Narrow host-owned operations used by native source readers.

The current Store implements this port. A future adapter must not need a raw
SQLite connection to read previously recorded model usage.
"""
from __future__ import annotations

from pathlib import Path
from typing import Protocol


class NativeEvidencePort(Protocol):
    def internal_source_ids(self, source_ids: list[str]) -> set[str]: ...
    def native_model_usage(self, source_id: str, trace_id: str, span_id: str) -> dict: ...
    def register_connection(self, record: dict) -> None: ...
    def redact_internal_source(self, source_id: str) -> None: ...
    def ingest(self, spans: list[dict], *, native_retry: bool = False,
               native_source_path: Path | None = None) -> object: ...
    def ingest_logs(self, records: list[dict], *, native_retry: bool = False,
                    native_source_path: Path | None = None) -> object: ...
