"""Trusted source-specific operation fact contributions.

The generic activity projector owns normalization and redaction. These
contributors handle selected legacy source semantics before that projection.
This in-process registry is not an external plugin grant or public SDK.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol


class OperationContributor(Protocol):
    id: str

    def matches(self, span: dict, attributes: dict) -> bool: ...
    def contribute(self, attributes: dict, domain: str, evidence: dict) -> bool: ...


@dataclass(frozen=True)
class OperationContributorRegistry:
    contributors: tuple[OperationContributor, ...]

    def __post_init__(self) -> None:
        contributors = tuple(self.contributors)
        ids = [getattr(item, "id", None) for item in contributors]
        if (any(not isinstance(id_, str) or not id_ for id_ in ids)
                or len(set(ids)) != len(ids)
                or any(not all(callable(getattr(item, name, None)) for name in
                               ("matches", "contribute")) for item in contributors)):
            raise ValueError("invalid operation contributor registry")
        object.__setattr__(self, "contributors", contributors)

    def apply(self, span: dict, attributes: dict, domain: str, evidence: dict) -> bool:
        """Return whether the generic activity detail is superseded."""
        suppress_detail = False
        for contributor in self.contributors:
            if contributor.matches(span, attributes):
                suppress_detail |= contributor.contribute(attributes, domain, evidence) is True
        return suppress_detail

    @classmethod
    def bundled(cls) -> OperationContributorRegistry:
        from .adapters.opencode_operation import OpenCodeLegacyOperationContributor
        return cls((OpenCodeLegacyOperationContributor(),))


BUNDLED_OPERATION_CONTRIBUTORS = OperationContributorRegistry.bundled()
