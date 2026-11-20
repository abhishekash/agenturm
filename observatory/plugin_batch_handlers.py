"""Trusted host dispatch for versioned source batch envelopes.

Only a host-constructed registry selects a validator. A child may advertise a
feature, but it cannot install code or select a response shape for the host.
The ordinary v1 batch remains strict when no envelope feature is required.
"""
from __future__ import annotations

import re
from collections.abc import Callable, Iterable, Mapping
from types import MappingProxyType

from .plugin_manifest import FEATURE
from .plugin_wire import (PollBatch, ProtocolError, validate_batch,
                          validate_content_batch)


BatchValidator = Callable[[dict, str], PollBatch]
_CONTENT_REFERENCE_FEATURE = re.compile(
    r"[a-z][a-z0-9.-]*-content-ref/[1-9][0-9]*\Z")


def _validate_ordinary(message: dict, source_id: str) -> PollBatch:
    return validate_batch(message, source_id=source_id)


def _validate_pi_content(message: dict, source_id: str) -> PollBatch:
    batch = validate_content_batch(message, source_id=source_id)
    # A child nominates source ranges; the receiver later opens and proves
    # the source itself before durable admission.
    from .plugin_pi_content import validate_refs
    validate_refs(batch.content_refs, batch)
    return batch


class BatchHandlerRegistry:
    """Immutable set of host-owned validators for alternate batch envelopes."""

    def __init__(self, handlers: Mapping[str, BatchValidator] | None = None):
        validated = {}
        for feature, validator in (handlers or {}).items():
            if (not isinstance(feature, str) or not FEATURE.fullmatch(feature)
                    or not callable(validator)):
                raise ValueError("invalid host batch handler")
            validated[feature] = validator
        self._handlers = MappingProxyType(validated)

    def with_handler(self, feature: str, validator: BatchValidator) -> BatchHandlerRegistry:
        """Compose another trusted handler before starting a process."""
        if feature in self._handlers:
            raise ValueError("duplicate host batch handler")
        return BatchHandlerRegistry({**self._handlers, feature: validator})

    def select(self, required_features: Iterable[str]) -> BatchValidator:
        features = frozenset(required_features)
        if any(not isinstance(feature, str) or not FEATURE.fullmatch(feature)
               for feature in features):
            raise ValueError("invalid required batch features")
        unknown = [feature for feature in features
                   if _CONTENT_REFERENCE_FEATURE.fullmatch(feature)
                   and feature not in self._handlers]
        if unknown:
            raise ProtocolError("unregistered content reference batch feature")
        selected = [self._handlers[feature] for feature in features
                    if feature in self._handlers]
        if len(selected) > 1:
            raise ProtocolError("conflicting batch envelope features")
        return selected[0] if selected else _validate_ordinary


DEFAULT_BATCH_HANDLERS = BatchHandlerRegistry().with_handler(
    "pi-content-ref/1", _validate_pi_content)
