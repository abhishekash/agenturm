# Kiro session-inspector contribution

20 November 2026. The cockpit now composes Kiro's permission view through a trusted session-inspector provider. Generic `terminal_ui.py` renders bounded overview fields and detail sections returned by the registry; it does not name Kiro or inspect `kiro_folders`. The Kiro formatter and provider live in `ui_kiro_inspector.py`, and `terminal.py` registers them at the cockpit composition root.

The existing inspector layout is unchanged. A Kiro session with recorded permissions still shows the same compact **Permissions** field and the same detailed **Permissions** section. A source that disappears from the current snapshot no longer supplies those sections. The provider reads only current snapshot metadata, keeps no cached folder paths, and follows the collector's metadata-only redaction of permission subjects and folder names. No new pane, key binding, source read, or content capture was added.

The registry accepts at most eight trusted providers, four fields and four detail sections total, bounded labels and text. A failing or invalid provider cannot prevent the rest of the inspector from rendering. A synthetic second runtime contributes its own field and section through the same UI path without a runtime-specific branch.

Validation: `python -m unittest tests.test_ui_session_inspector_provider tests.test_kiro_permissions tests.test_kiro_sessions -q` — 30 tests passed locally. Live Textual tests exercised the Kiro compact and details views at 60×18 and 80×24, source disappearance, and a synthetic provider at 60×18. Existing Kiro permission parsing, metadata-only redaction, and session tests remain green. This is an in-process trusted UI contribution, not a general untrusted extension execution boundary.
