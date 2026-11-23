# Paired handoff, request routing, and evidence return

On November 23, 2026, Python 0.2.8 and a Rust 0.3.5 development build ran in
tmux 3.4 at 120×32 and 60×18 with `TERM=xterm-256color`. Each run used a
separate tmux socket, home, XDG directories and history. The Python run used
an isolated loopback receiver. The fixtures were synthetic Codex recordings
and a trace with a known source, session and span identity.

| Artifact | SHA-256 |
| --- | --- |
| Python 0.2.8 wheel | `4fda5471d0a6993e61908949ffc86d7ff28322928071c2463deea774848fba2e` |
| Rust 0.3.5 development executable | `ae327bef91d6f40d275195ca9a94028d0ae75fb493ab08a90b830d62649f8072` |
| Rust plugin host from the 0.3.4 bundle | `175cbe31fbb6723cff9981b54b4fb8c3196788976d4e8f084f76a20554986ac9` |
| Rust Codex adapter from the 0.3.4 bundle | `02988f288d2fa81554c7a2c30fb4340fdd0883c53eb02eb85cd444d9f7714e23` |

The wheel's Python modules matched the tag at
`2802d79930728bb331db1d4d718d6dea1c9c374d` byte for byte. The Rust executable was built from
`603d71ec44be82500599d9711dd85a0419131f98`. The four terminal runs
exited with status 0 and restored their original termios settings.

| Journey | Python 0.2.8 | Rust 0.3.5 |
| --- | --- | --- |
| Keyboard handoff export | From the selected `d3-main` session, `b` → `n` saved a linked task. Enter → `h` opened the task brief. Tab accepted a new path and reached **Write MD + HTML**. Escape returned to the same task and task list. At 60×18 the screen displayed its 60×20 minimum. The run resized to 60×20 for editing and export, then returned to 60×18. | From the selected native `d3-main` session, `b` → `n` saved a linked task. After selecting `d3-approval` in Agents, `b` → `h` opened a handoff from `d3-main` to `d3-approval`. Ctrl+S saved the note and `x` exported both files. The same handoff remained selected. Escape restored the destination agent row. Both viewports completed the export at their original size. |
| Verified live approval route | `w`, select the approval, Enter, then `g` focused the verified fixture pane. Returning with tmux restored the same approval detail. At 60×18, Escape returned to the queue and Enter reopened the same pending request. | `w` showed the exact pending `d3-approval` row and `turn-first`. `l` focused the verified fixture pane. Returning with tmux restored the same request. `s` marked it seen and left it pending. Escape restored the original `d3-main` Agents row. |
| Related evidence and return | From MCP Calls, `t` followed the synthetic `d3-related` call to span `5555555555555555`. Alt+Left returned to the same MCP selection. A second open displayed the same span at each size. | From native Activity, `8` opened the scope's exact linked trace and span `4444444444444444`. Escape restored the same Activity content and then the same Agents row at each size. |

Each export produced Markdown and HTML at mode `0600`. Both files contained
the authored note and the expected session link or handoff endpoints. The
HTML escaped the fixture note's angle brackets.

The route target was a labeled synthetic local process in its own tmux pane.
Its actual command line carried `codex resume d3-approval` and it held the
matching fixture rollout open. Each app's keyboard route changed the active
pane from `@0/%0` to `@1/%1`. The fixture performed no provider work.

The qualification host retained the manifest, pane captures, outcomes and
export hashes under `/tmp/d3-retained-final`. This record covers the paths in
the table. The earlier archive record covers the other paired journeys.
[Paired Rust 0.3.4 qualification](rust-034-paired-keyboard.md)
