"""OpenCode's pre-capture-version operation fact compatibility rule."""
from __future__ import annotations

from ..operation_facts import decode


class OpenCodeLegacyOperationContributor:
    id = "opencode-legacy-operation"

    def matches(self, span: dict, attributes: dict) -> bool:
        return (span.get("scope_name") == "observatory.opencode"
                and span.get("service") == "opencode"
                and attributes.get("agent.runtime") == "OpenCode")

    def contribute(self, attributes: dict, domain: str, evidence: dict) -> bool:
        current_native = (type(attributes.get("observatory.operation.capture_version")) is int
                          and attributes["observatory.operation.capture_version"] == 1)
        if current_native:
            return True
        legacy = decode(attributes.get("agent.activity.detail"))
        if isinstance(legacy, dict) and str(domain).casefold() == "shell":
            if isinstance(legacy.get("command"), str):
                evidence["commands"].append(("opencode.command", legacy["command"]))
            if "exit_code" in legacy:
                evidence["exits"].append(("opencode.metadata.exit", legacy["exit_code"]))
        evidence.update(input_clipped=True, output_clipped=True)
        return False
