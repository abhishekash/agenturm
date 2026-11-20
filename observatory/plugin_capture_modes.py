"""Host-owned capture-mode declarations for explicit process sources.

The TOML file and child process only name a mode. Trusted receiver code builds
this immutable registry before parsing configuration or starting a worker.
The trusted mode binds reference families to receiver-owned admission providers.
"""
from __future__ import annotations

from dataclasses import dataclass
from types import MappingProxyType
from typing import TYPE_CHECKING

from .plugin_manifest import FEATURE, PLUGIN_ID
from .plugin_wire import ProtocolError

if TYPE_CHECKING:
    from .plugin_admission import SourceLease
    from .plugin_manifest import PluginManifest
    from .plugin_supervisor import PluginSource


@dataclass(frozen=True)
class CaptureMode:
    name: str
    required_features: frozenset[str] = frozenset()
    replaces: tuple[str, ...] | None = None
    requires_root: bool = False
    required_grants: frozenset[str] = frozenset()
    reference_feature: str | None = None
    reference_family: str | None = None
    grant_type: type | None = None
    reference_provider_id: str | None = None

    def __post_init__(self) -> None:
        if (not isinstance(self.name, str) or not PLUGIN_ID.fullmatch(self.name)
                or type(self.required_features) is not frozenset
                or any(not isinstance(feature, str) or not FEATURE.fullmatch(feature)
                       for feature in self.required_features)
                or type(self.requires_root) is not bool
                or self.replaces is not None and (
                    not isinstance(self.replaces, tuple) or
                    any(not isinstance(name, str) or not PLUGIN_ID.fullmatch(name)
                        for name in self.replaces))
                or type(self.required_grants) is not frozenset
                or any(not isinstance(grant, str) or not PLUGIN_ID.fullmatch(grant)
                       for grant in self.required_grants)
                or self.reference_feature is not None and (
                    not isinstance(self.reference_feature, str)
                    or not FEATURE.fullmatch(self.reference_feature)
                    or self.reference_feature not in self.required_features)
                or self.reference_family not in (None, "content", "command", "reference")
                or (self.reference_family is None) != (self.reference_feature is None)
                or (self.grant_type is None) != (self.reference_feature is None)
                or (self.reference_provider_id is None) != (self.reference_feature is None)
                or self.reference_provider_id is not None and (
                    not isinstance(self.reference_provider_id, str)
                    or not PLUGIN_ID.fullmatch(self.reference_provider_id))
                or self.grant_type is not None and (
                    not callable(getattr(self.grant_type, "fingerprint_for_source", None))
                    or not callable(getattr(self.grant_type, "from_source", None)))):
            raise ValueError("invalid trusted capture mode")

    def validate_config(self, manifest: PluginManifest, *, replaces: tuple[str, ...],
                        source_root: str | None, grants: tuple[str, ...],
                        features: tuple[str, ...]) -> None:
        required = self.required_features
        if ((self.replaces is not None and replaces != self.replaces)
                or self.requires_root and source_root is None
                or not self.required_grants <= set(grants)
                or not self.required_grants <= set(manifest.grants)
                or not required <= set(features)
                or not required <= set(manifest.provides)):
            raise ValueError(f"{self.name} capture requires configured features and grants")

    def fingerprint(self, source: PluginSource) -> str:
        if self.grant_type is None:
            return ""
        return self.grant_type.fingerprint_for_source(source)

    def grant(self, source: PluginSource, lease: SourceLease):
        if self.grant_type is None:
            return None
        return self.grant_type.from_source(source, lease)


class CaptureModeRegistry:
    """Immutable declarations installed by the trusted receiver, never TOML."""

    def __init__(self, modes: tuple[CaptureMode, ...] = (CaptureMode("metadata-only"),)) -> None:
        if (not isinstance(modes, tuple) or len(modes) > 16
                or any(not isinstance(mode, CaptureMode) for mode in modes)):
            raise ValueError("invalid trusted capture modes")
        names = [mode.name for mode in modes]
        features = [mode.reference_feature for mode in modes if mode.reference_feature]
        providers = [mode.reference_provider_id for mode in modes
                     if mode.reference_provider_id]
        if (len(set(names)) != len(names) or len(set(features)) != len(features)
                or len(set(providers)) != len(providers)
                or names.count("metadata-only") != 1):
            raise ValueError("duplicate or missing trusted capture mode")
        metadata = next(mode for mode in modes if mode.name == "metadata-only")
        if metadata.reference_feature is not None or metadata.grant_type is not None:
            raise ValueError("metadata capture cannot authorize references")
        self._modes = MappingProxyType({mode.name: mode for mode in modes})
        self._reference_features = frozenset(features)

    def with_mode(self, mode: CaptureMode) -> CaptureModeRegistry:
        if not isinstance(mode, CaptureMode) or mode.name in self._modes:
            raise ValueError("duplicate or invalid trusted capture mode")
        return CaptureModeRegistry((*self._modes.values(), mode))

    def get(self, name: str) -> CaptureMode:
        try:
            return self._modes[name]
        except (KeyError, TypeError) as exc:
            raise ValueError("unsupported plugin capture mode") from exc

    def validate_config(self, name: str, manifest: PluginManifest, *,
                        replaces: tuple[str, ...], source_root: str | None,
                        grants: tuple[str, ...], features: tuple[str, ...]) -> CaptureMode:
        mode = self.get(name)
        mode.validate_config(manifest, replaces=replaces, source_root=source_root,
                             grants=grants, features=features)
        for feature in self._reference_features:
            if (feature in features) != (feature == mode.reference_feature):
                raise ValueError("reference feature requires matching explicit capture mode")
        return mode

    def select_session(self, source: PluginSource | None,
                       required_features: tuple[str, ...]) -> CaptureMode:
        mode = self.get(source.capture_mode if source is not None else "metadata-only")
        for feature in self._reference_features:
            if (feature in required_features) != (feature == mode.reference_feature):
                raise ProtocolError("reference feature requires trusted configuration")
        return mode

    @staticmethod
    def require_source(mode: CaptureMode, source: PluginSource) -> None:
        """Recheck host policy at startup before a source lease is activated."""
        try:
            mode.validate_config(source.manifest, replaces=source.replaces,
                                 source_root=source.source_root,
                                 grants=source.allowed_grants,
                                 features=source.runtime_features)
        except ValueError as exc:
            raise ProtocolError("capture source differs from trusted mode policy") from exc


from .plugin_pi_content import PiContentGrant  # noqa: E402 - trusted built-ins
from .plugin_pi_command import PiCommandGrant  # noqa: E402 - trusted built-ins

_PI_DIRECTORY = frozenset({"source-discovery/1", "source-discovery-root-gap/1",
                           "configured-root/1"})
DEFAULT_CAPTURE_MODES = CaptureModeRegistry((
    CaptureMode("metadata-only"),
    CaptureMode("pi-review-text-v1", _PI_DIRECTORY | {"pi-content-ref/1"},
                ("pi",), True, frozenset({"configured-source-read"}),
                "pi-content-ref/1", "content", PiContentGrant, "pi-content"),
    CaptureMode("pi-review-text-command-v1", _PI_DIRECTORY | {"pi-command-ref/1"},
                ("pi",), True, frozenset({"configured-source-read"}),
                "pi-command-ref/1", "command", PiCommandGrant, "pi-command"),
))
