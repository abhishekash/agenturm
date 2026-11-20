"""Host-owned exact Activity readers for trusted reference providers.

Providers are registered by trusted receiver code. The API caller may name a
provider, but durable source ownership, exact Activity identity, receiver,
capture policy, and projected revision are checked before and after the read.
"""
from __future__ import annotations

from dataclasses import dataclass
from types import MappingProxyType
import re
from typing import Callable

from .plugin_activity import _EVENT_KEY
from .workflow_evidence import safe_text

_PROVIDER_ID = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")
MAX_READERS = 8
MAX_CONTENT_BYTES = 16 * 1024


@dataclass(frozen=True)
class ReferenceText:
    """Trusted provider result bound to the host's exact read revision."""

    provider_id: str
    receiver_id: str
    source_id: str
    event_id: str
    lease_epoch: int
    policy_epoch: int
    dirty_seq: int
    text: str
    clipped: bool = False


@dataclass(frozen=True)
class ReferenceReadProvider:
    id: str
    read_exact: Callable[[object, str], ReferenceText | None]


def _binding(store, provider_id: str, event_id: str):
    """Return one current, policy-projected, durably-owned Activity binding."""
    receiver = store.receiver_namespace()
    with store._read() as db:
        row = db.execute("""SELECT a.owner_key,c.receiver_id,c.lease_epoch,c.policy_epoch,
                c.dirty_seq,c.capture_enabled,p.projected_seq,p.policy_epoch
                FROM activity_index_v1 a
                JOIN plugin_source_cursor_v1 c ON c.source_id=a.owner_key
                JOIN plugin_projection_cursor_v1 p ON p.source_id=c.source_id
                JOIN plugin_reference_owner_v1 o ON o.source_id=c.source_id
                WHERE a.event_key=? AND a.kind='plugin' AND o.provider_id=?""",
                         (event_id, provider_id)).fetchall()
        marker = db.execute("SELECT value FROM schema_meta WHERE key='receiver_id'").fetchone()
    if (len(row) != 1 or marker is None or marker[0] != receiver):
        return None
    owner, row_receiver, lease, policy, dirty, enabled, projected, projected_policy = row[0]
    if (row_receiver != receiver or enabled != 1 or lease <= 0 or policy <= 0 or dirty <= 0
            or (projected, projected_policy) != (dirty, policy)):
        return None
    return (receiver, owner, lease, policy, dirty)


class ReferenceReadRegistry:
    """Bounded immutable registry of trusted readers; no manifest installs."""

    def __init__(self, providers: tuple[ReferenceReadProvider, ...]):
        if (type(providers) is not tuple or not 1 <= len(providers) <= MAX_READERS
                or any(not isinstance(item, ReferenceReadProvider)
                       or type(item.id) is not str or not _PROVIDER_ID.fullmatch(item.id)
                       or not callable(item.read_exact) for item in providers)
                or len({item.id for item in providers}) != len(providers)):
            raise ValueError("invalid trusted reference readers")
        self._providers = MappingProxyType({item.id: item for item in providers})

    @property
    def providers(self) -> tuple[ReferenceReadProvider, ...]:
        return tuple(self._providers.values())

    def read_exact(self, store, provider_id: str, event_id: str) -> ReferenceText | None:
        if (type(provider_id) is not str or not _PROVIDER_ID.fullmatch(provider_id)
                or type(event_id) is not str or not _EVENT_KEY.fullmatch(event_id)):
            return None
        provider = self._providers.get(provider_id)
        if provider is None:
            return None
        try:
            before = _binding(store, provider_id, event_id)
            if before is None:
                return None
            result = provider.read_exact(store, event_id)
            after = _binding(store, provider_id, event_id)
            if (not isinstance(result, ReferenceText) or before != after
                    or (result.provider_id, result.receiver_id, result.source_id,
                        result.event_id, result.lease_epoch, result.policy_epoch,
                        result.dirty_seq) != (provider_id, before[0], before[1], event_id,
                                               *before[2:])
                    or type(result.text) is not str
                    or len(result.text.encode("utf-8")) > MAX_CONTENT_BYTES
                    or safe_text(result.text) != result.text or type(result.clipped) is not bool):
                return None
            return result
        except Exception:
            # A trusted provider is still an extension boundary: an ordinary
            # implementation error must not escape into the HTTP handler.
            return None


def _pi_user_text(store, event_id: str) -> ReferenceText | None:
    from .plugin_pi_content_read import Available, read_exact_pi_user_text
    result = read_exact_pi_user_text(store, event_id)
    if not isinstance(result, Available):
        return None
    proof = result.proof
    return ReferenceText("pi-content", proof.receiver_id, proof.source_id, proof.activity_id,
                         proof.lease_epoch, proof.policy_epoch, proof.source_dirty_seq,
                         result.redacted_text)


def _pi_requested_command(store, event_id: str) -> ReferenceText | None:
    from .plugin_pi_command_read import Available, read_exact_pi_requested_command
    result = read_exact_pi_requested_command(store, event_id)
    if not isinstance(result, Available):
        return None
    proof = result.proof
    return ReferenceText("pi-command", proof.receiver_id, proof.source_id, proof.activity_id,
                         proof.lease_epoch, proof.policy_epoch, proof.source_dirty_seq,
                         result.redacted_command)


DEFAULT_REFERENCE_READERS = ReferenceReadRegistry((
    ReferenceReadProvider("pi-content", _pi_user_text),
    ReferenceReadProvider("pi-command", _pi_requested_command),
))
