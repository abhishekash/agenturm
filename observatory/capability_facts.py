"""Host-owned, bounded facts for trusted capability providers.

Only typed aggregates cross into a pane provider. Source payloads, prompts,
shell commands and paths remain inside the host query path.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from .agent_hierarchy import AGENT_ID
from .ui_contributions import PaneScope

if TYPE_CHECKING:
    from .core import Store


MAX_SIGNAL_EVENTS = 200


@dataclass(frozen=True)
class PromptSignalFacts:
    state: str
    observed_events: int = 0
    user_messages: int = 0
    shell_actions: int = 0
    first_shell_after_user_ms: int | None = None
    window_complete: bool = False


def prompt_signal_facts(store: Store, scope: PaneScope, *, capture_allowed: bool) -> PromptSignalFacts:
    """Count metadata in one exact service/session/agent scope.

    A caller supplies the current capture decision. An absent or denied scope
    returns no facts. A bounded recent page is explicitly marked partial.
    """
    if (capture_allowed is not True or not isinstance(scope, PaneScope)
            or not scope.service or not scope.session_id
            or not AGENT_ID.fullmatch(scope.agent_id)):
        return PromptSignalFacts("unavailable")
    try:
        page = store.activity_page(limit=MAX_SIGNAL_EVENTS, service=scope.service,
                                   session_id=scope.session_id, agent_id=scope.agent_id,
                                   subtree=False, include_reviews=False)
    except (ValueError, RuntimeError):
        return PromptSignalFacts("unavailable")
    events = [event for event in page["activity"]
              if event.get("service") == scope.service
              and event.get("session_id") == scope.session_id
              and event.get("agent_id") == scope.agent_id]
    users = []
    shells = []
    for event in events:
        stamp = event.get("time_unix_nano")
        if type(stamp) is not int or stamp < 0:
            continue
        if event.get("domain") == "message" and event.get("action") == "user":
            users.append(stamp)
        elif event.get("domain") == "shell" and event.get("action") == "execute":
            shells.append(stamp)
    first_ms = None
    if users and shells:
        first_user = min(users)
        later = [stamp for stamp in shells if stamp >= first_user]
        if later:
            first_ms = (min(later) - first_user) // 1_000_000
    complete = not bool(page.get("next_cursor")) and len(events) < MAX_SIGNAL_EVENTS
    return PromptSignalFacts("ready" if complete else "degraded", len(events),
                             len(users), len(shells), first_ms, complete)
