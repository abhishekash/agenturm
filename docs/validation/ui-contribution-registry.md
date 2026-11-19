# UI contribution registry checkpoint

Date: 19 November 2026. Scope: trusted, opt-in capability panes in the existing Textual shell.

`PaneContribution` declares an ID, title, up to six column specifications and a scoped query. `PaneRegistry` rejects reserved IDs and duplicates. The headless query contract returns a bounded `PaneModel` with state, summary and rows; it does not import Textual or receive a raw receiver snapshot. The shell owns layout, focus, navigation and generic row/detail rendering. Contributed panes appear in More, can be selected with the same `action_page` route and are omitted when not supplied. A saved view that is no longer registered falls back to Agents. A provider exception shows a generic unavailable state without exposing the exception text.

This is an explicit constructor hook for trusted in-process capabilities, not discovery or execution of external UI code. The provider runs synchronously and must answer from a local, bounded query/cache; production extension supervision, time budgets, scoped query authorization and persistence of per-capability diagnostics remain open. The synthetic Prompt signals pane returns one labeled observation with a coverage denominator. It does not claim to measure prompt quality.

Validation: 5 registry/pilot/navigation tests passed at 60×18 and 100×30, and 31 adjacent TUI usability, investigation and branding tests passed. The pilot covers menu registration, routing, row rendering, provider failure isolation and return to Agents. Full CI remains separate. See the [execution plan](../rfcs/rust-plugin-kernel-plan.md).
