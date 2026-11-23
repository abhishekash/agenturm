# Paired Rust and Python keyboard qualification

On November 23, 2026, the installed Python 0.2.8 wheel and the extracted Rust
0.3.4 Linux x86-64 archive ran in separate tmux 3.4 instances at 120×32 and
60×18 with `TERM=xterm-256color`. Each run used an isolated home, state
directory, and synthetic Codex recordings. The recordings contain a parent and
child session, two prompts, an interrupted turn, a later completion, MCP and edit
events, a synthetic pull request result, and a pending approval. A fixture trace
was imported through each application's supported path.

| Artifact | SHA-256 |
| --- | --- |
| Python `agenturm-0.2.8-py3-none-any.whl` | `4fda5471d0a6993e61908949ffc86d7ff28322928071c2463deea774848fba2e` |
| Rust `agenturm-0.3.4-linux-x86_64.tar.gz` | `74f85b9d0b1f0c9c2afc9dbcc9e97119087777b03deed1203a0072e28878a18c` |
| Extracted Rust executable | `96284c7473565b011621b3f1325bb21acb193821a1f21d1c7141f5b7fd2b7893` |

The Python wheel's Python modules matched tag `2802d79930728bb331db1d4d718d6dea1c9c374d`
byte for byte. The Rust archive was built from `097b2ab458e18fac00e609ddd7f95fe0248f3e90`.
Every completed terminal run exited with status 0 and restored the original
termios settings.

| Journey | Python 0.2.8 | Rust 0.3.4 |
| --- | --- | --- |
| Agents and selection | Search narrowed to the fixture session and opened its inspector. The child was recorded in the parent row. Keyboard child expansion remains unverified. | Search narrowed the list. Left and Right collapsed and expanded the recorded child. Escape from the session overview returned to the same highlighted Codex row. Active search can shift focus to an OTLP row with the same session ID. |
| Interrupted work | The fixture session's Details and Review opened. Exact interrupted-turn selection and return remain unverified. | Activity → `t` → Enter opened `turn-first` with interrupted state, `codex_task_aborted` and the synthetic PR result. Escape restored the same highlighted turn picker. The picker also showed the later completion. |
| Retained prompts | The wide index and compact picker opened the request reader. Scrolling, resize and F1 return stayed in the request reader. | `p` → Enter revealed the older and latest prompts. Escape returned to the picker. The reader stayed open across resize. Revoking approval denied another reveal. |
| MCP and linked traces | MCP and Traces opened. Related-evidence exact selection and return remain unverified. | The MCP picker preserved its row across disclosure. Revoking only the MCP grant denied reveal. Renewing it restored the fixture response at both widths. Session Traces showed the fixture trace and span IDs and returned to Activity. |
| Review, decisions and handoff | Review and Saved Tasks opened. At 60×18 the task editor requested at least 60×20 and stated that edits were preserved. | Review saved an evidence note containing literal `q?1` and displayed it after saving. The linked task reappeared in Saved Tasks. The keyboard Markdown and HTML export journey remains unverified in both applications. |
| Pending request | The queue showed the approval. The trial opened the previously selected completed row, so the pending request's route remains unverified. | Workflow selected the pending approval. Locate reported that no unique live tmux pane was verified. Mark seen left the request open, pending native resolution. |
| Usage and Host | Global Metrics and Host were reached in the narrow run. The full paired unit and history comparison remains open. | Global Usage and Host opened and returned to Agents at both widths. Historical host metrics and reported cost were outside this fixture. |
| Help and terminal lifecycle | F1 opened keyboard help and Escape returned to Agents and to the prompt reader. | In the final archive F1 then Escape restored the selected Agents row or inspector pane. The candidate before this fix exited from Agents help on Escape. |

The comparison used explicit local cockpit launches. The bare launch
empty-to-populated path, full keyboard handoff export, verified live request
routing, related-evidence focus restoration, and repeated host and usage
checks remain open in the [cutover inventory](../rfcs/rust-cutover-workflows.md).
This record qualifies the listed terminal behaviors for the archive. The
default-entry cutover remains gated on the remaining required workflows.

The qualification host retained synthetic terminal frames and manifests under
`/tmp/d3-journeys-python`, `/tmp/d3-journeys-rust`, and `/tmp/d3-mcp-final`.

The follow-up records keyboard handoff export, live approval routing, and evidence return. [Paired Rust 0.3.5 keyboard qualification](rust-035-retained-keyboard.md)
