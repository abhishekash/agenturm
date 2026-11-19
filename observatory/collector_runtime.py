"""One composition root for trusted local source adapters and readers.

The registry is explicit Python configuration. It does not load plugin
manifests or execute independently installed code; that requires the
supervised process host and durable admission contract.
"""
from __future__ import annotations

from dataclasses import dataclass

from .native_readers import NativeReaderSpec, builtin_readers
from .native_sources import NativeSourceSpec, builtin_sources


@dataclass(frozen=True)
class CollectorRegistry:
    sources: tuple[NativeSourceSpec, ...]
    readers: tuple[NativeReaderSpec, ...]

    def __post_init__(self) -> None:
        sources = tuple(self.sources)
        readers = tuple(self.readers)
        if not all(isinstance(spec, NativeSourceSpec) for spec in sources):
            raise TypeError("collector sources must be native source specs")
        if not all(isinstance(spec, NativeReaderSpec) for spec in readers):
            raise TypeError("collector readers must be native reader specs")
        source_ids = [spec.id for spec in sources]
        reader_ids = [spec.id for spec in readers]
        if len(source_ids) != len(set(source_ids)) or len(reader_ids) != len(set(reader_ids)):
            raise ValueError("duplicate collector plugin ID")
        if set(source_ids) & set(reader_ids):
            raise ValueError("source and reader plugin IDs must be unique")
        object.__setattr__(self, "sources", sources)
        object.__setattr__(self, "readers", readers)

    @classmethod
    def bundled(cls) -> CollectorRegistry:
        return cls(builtin_sources(), builtin_readers())

    def without(self, *plugin_ids: str) -> CollectorRegistry:
        disabled = set(plugin_ids)
        known = {spec.id for spec in (*self.sources, *self.readers)}
        if not disabled <= known:
            raise ValueError("unknown collector plugin ID")
        return CollectorRegistry(
            tuple(spec for spec in self.sources if spec.id not in disabled),
            tuple(spec for spec in self.readers if spec.id not in disabled),
        )


def start_local_collector(store: object, *, capture_details: bool,
                          registry: CollectorRegistry | None = None):
    """Construct and start exactly the selected trusted collectors."""
    from .local_agents import LocalAgents

    selected = registry if registry is not None else CollectorRegistry.bundled()
    if not isinstance(selected, CollectorRegistry):
        raise TypeError("collector registry must be a CollectorRegistry")
    return LocalAgents(store, capture_details=capture_details,
                       source_specs=selected.sources, reader_specs=selected.readers).start()
