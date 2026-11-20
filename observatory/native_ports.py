"""Host-owned evidence operations exposed to trusted native readers.

The facade limits accidental Store coupling. It is not a security boundary for
in-process Python code; an independently installed adapter will need the
separate process protocol specified in the migration plan.
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
               native_source_path: Path | None = None,
               skip_unchanged_native: bool = False) -> object: ...
    def ingest_logs(self, records: list[dict], *, native_retry: bool = False,
                    native_source_path: Path | None = None,
                    expected_receiver: str | None = None,
                    skip_unchanged_native: bool = False) -> object: ...


class NativeEvidenceFacade:
    """Small host API for legacy readers; Store retains policy and SQL authority."""

    __slots__ = ("_store",)

    def __init__(self, store: NativeEvidencePort):
        self._store = store

    def internal_source_ids(self, source_ids: list[str]) -> set[str]:
        return self._store.internal_source_ids(source_ids)

    def native_model_usage(self, source_id: str, trace_id: str, span_id: str) -> dict:
        return self._store.native_model_usage(source_id, trace_id, span_id)

    def register_connection(self, record: dict) -> None:
        self._store.register_connection(record)

    def redact_internal_source(self, source_id: str) -> None:
        self._store.redact_internal_source(source_id)

    def ingest(self, spans: list[dict], *, native_retry: bool = False,
               native_source_path: Path | None = None,
               skip_unchanged_native: bool = False) -> object:
        return self._store.ingest(spans, native_retry=native_retry,
                                  native_source_path=native_source_path,
                                  skip_unchanged_native=skip_unchanged_native,
                                  _trusted_native=True)

    def ingest_logs(self, records: list[dict], *, native_retry: bool = False,
                    native_source_path: Path | None = None,
                    expected_receiver: str | None = None,
                    skip_unchanged_native: bool = False) -> object:
        return self._store.ingest_logs(records, native_retry=native_retry,
                                       native_source_path=native_source_path,
                                       expected_receiver=expected_receiver,
                                       skip_unchanged_native=skip_unchanged_native,
                                       _trusted_native=True)
