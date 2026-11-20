"""Trusted host hooks for reference-backed plugin evidence admission.

The receiver selects one provider before accepting a child batch. A child may
nominate bounded source ranges, but cannot install or select a verifier. The
ordinary evidence/cursor transaction stays owned by plugin_admission.
"""
from __future__ import annotations

from dataclasses import dataclass
from types import MappingProxyType
import re

from .plugin_wire import ProtocolError

_PROVIDER_ID = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")


@dataclass(frozen=True)
class ReferenceSubmission:
    provider_id: str
    refs: object
    grant: object


@dataclass(frozen=True)
class PreparedReference:
    provider_id: str
    checked: object
    digest: str
    grant: object

    @property
    def requested(self) -> bool:
        return bool(self.checked)


@dataclass(frozen=True)
class ReferenceWriteContext:
    db: object
    lease: object
    batch: object
    prepared: PreparedReference | None
    verified: object
    withheld_reason: str
    inserted_hashes: frozenset[str]
    ambiguous_ids: frozenset[str]
    dirty_seq: int


class ReferenceAdmissionProvider:
    """Host-installed provider; implementations own source-specific SQL/proof."""
    id: str = ""

    def prepare(self, refs, grant, batch, lease) -> PreparedReference:
        raise NotImplementedError

    def verify(self, prepared: PreparedReference, batch, prior_cursor):
        raise NotImplementedError

    def replay(self, db, source_id: str, cursor: str,
               prepared: PreparedReference | None) -> None:
        raise NotImplementedError

    def commit(self, context: ReferenceWriteContext):
        raise NotImplementedError

    def inherited_withheld_reason(self, selected_provider: ReferenceAdmissionProvider | None,
                                  reason: str) -> str:
        """Opt-in status sharing among trusted providers; unrelated modes get none."""
        return ""

    def finalize(self, context: ReferenceWriteContext, outcome) -> None:
        """Optional final receipt/status write after every provider commits."""


class ReferenceAdmissionRegistry:
    """Immutable set of receiver-owned providers, not child or TOML input."""

    def __init__(self, providers: tuple[ReferenceAdmissionProvider, ...]):
        if (type(providers) is not tuple or not 1 <= len(providers) <= 8
                or any(not isinstance(item, ReferenceAdmissionProvider)
                       or type(item.id) is not str
                       or not _PROVIDER_ID.fullmatch(item.id)
                       for item in providers)
                or len({item.id for item in providers}) != len(providers)):
            raise ValueError("invalid trusted reference providers")
        self._providers = MappingProxyType({item.id: item for item in providers})

    @property
    def providers(self) -> tuple[ReferenceAdmissionProvider, ...]:
        return tuple(self._providers.values())

    def with_provider(self, provider: ReferenceAdmissionProvider) -> ReferenceAdmissionRegistry:
        if not isinstance(provider, ReferenceAdmissionProvider):
            raise ValueError("invalid trusted reference provider")
        return ReferenceAdmissionRegistry((*self.providers, provider))

    def select(self, submission: ReferenceSubmission | None,
               batch, lease) -> PreparedReference | None:
        if submission is None:
            return None
        if type(submission) is not ReferenceSubmission:
            raise ProtocolError("trusted reference submission required")
        if (type(submission.provider_id) is not str
                or not _PROVIDER_ID.fullmatch(submission.provider_id)):
            raise ProtocolError("invalid trusted reference provider ID")
        provider = self._providers.get(submission.provider_id)
        if provider is None:
            raise ProtocolError("reference admission provider is unavailable")
        prepared = provider.prepare(submission.refs, submission.grant, batch, lease)
        if (type(prepared) is not PreparedReference
                or prepared.provider_id != provider.id):
            raise ProtocolError("invalid trusted reference preparation")
        return prepared

    def provider(self, prepared: PreparedReference) -> ReferenceAdmissionProvider:
        return self._providers[prepared.provider_id]

    def require_provider(self, provider_id: str | None) -> None:
        if provider_id is not None and provider_id not in self._providers:
            raise ProtocolError("trusted reference admission provider is unavailable")


from .plugin_pi_reference_admission import (  # noqa: E402 - trusted built-ins
    PiCommandAdmission, PiContentAdmission,
)

DEFAULT_REFERENCE_ADMISSIONS = ReferenceAdmissionRegistry((
    PiContentAdmission(), PiCommandAdmission(),
))
