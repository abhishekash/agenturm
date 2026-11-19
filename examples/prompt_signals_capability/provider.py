"""Trusted Prompt signals pane using only host-supplied aggregate facts."""
from __future__ import annotations

from typing import Callable

from observatory.capability_facts import PromptSignalFacts
from observatory.ui_contributions import PaneContribution, PaneModel, PaneRow, PaneScope


def pane(read_facts: Callable[[PaneScope], PromptSignalFacts]) -> PaneContribution:
    def query(scope: PaneScope) -> PaneModel:
        facts = read_facts(scope)
        if facts.state == "unavailable":
            return PaneModel("unavailable", "Content capture or exact agent scope unavailable")
        coverage = ("complete recent window" if facts.window_complete else
                    "partial recent window")
        rows = [
            PaneRow("users", ("Recorded user messages", str(facts.user_messages), coverage)),
            PaneRow("shell", ("Recorded shell actions", str(facts.shell_actions), coverage)),
            PaneRow("first-shell", ("First shell action after a user message",
                                    str(facts.first_shell_after_user_ms) + " ms"
                                    if facts.first_shell_after_user_ms is not None else "unavailable",
                                    coverage)),
        ]
        return PaneModel(facts.state,
                         f"{facts.observed_events} scoped events · {coverage} · observational signals only",
                         tuple(rows))

    return PaneContribution("prompt-signals", "Prompt signals",
                            (("Signal", 40, 2), ("Observed", 14, 0), ("Coverage", 23, 1)), query)
