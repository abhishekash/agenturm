# Rust cockpit acceptance with local native sessions

The reported Rust 0.3.8 cockpit showed three rows. The running executable identified on the host was a bare launch of the extracted 0.3.8 bundle with its own `XDG_STATE_HOME`. Its schema 17 history held 21 source and session pairs after polling. The launch state was separate from the older default database. The temporary launch directory closed before a current-cursor comparison could be made.

## Released baseline

Replaying discovery with the exact 0.3.8 binary and isolated state selected four Codex transcripts. In one sample three of the four were ordinary subagents of one native session. The same default cap selected three old main Claude transcripts and one sidechain. An explicitly configured collector admitted eight source and session pairs, including two distinct Claude sources with the same native session ID. The default managed history admitted four Codex sessions in the same recurring sample. The visible Claude recordings were older than the 14-day managed retention window. Subagent classification came from native header metadata. File times and retained rows did not establish which agent processes were still open.

The actual Claude historical control carries a native `custom-title` and a current recorded `cwd` within its bounded transcript. Its file had 259,858 bytes and its matching title records ended at offsets 71,140 and 259,629. None of the 17 available Claude project transcripts exceeded 8 MiB. The historical fixture establishes real native admission and can check the title and project projection. It does not establish contemporaneous live Claude collection or qualify a prefix beyond 8 MiB.

## Run the checks

Use an installed bundle with its sibling plugin host and adapter catalog. Pass alternate native homes only when needed. The processes under test use disposable state and an isolated configuration. The checks never edit live native logs or the running cockpit database and do not call a model provider.

```sh
python3 scripts/validate_rust_real_session_roster.py --app /path/to/bundle/bin/agenturm audit
python3 scripts/validate_rust_real_session_roster.py --app /path/to/bundle/bin/agenturm verify --expected /path/to/private-codex-manifest.json
python3 scripts/validate_rust_real_session_roster.py --app /path/to/bundle/bin/agenturm historical --expected /path/to/private-claude-manifest.json
python3 scripts/validate_rust_real_session_roster.py --app /path/to/bundle/bin/agenturm mutation
```

For `verify`, the manifest has a `sessions` array of five to twenty expected source and native session pairs and an `excluded` array with at least one exact guardian review. A session record needs `harness`, `path` relative to the provider’s `sessions` or `projects` directory, its native `session` UUID, `title`, and absolute `project`. Supply `kind` as `subagent` for at least one ordinary worker. An optional `agent` checks the supporting name on the roster. An optional `status` checks an independently established display state. Guardian controls have `harness`, `path`, `session`, and `kind` set to `review`. Obtain titles from native display metadata rather than from unread or captured messages. Keep the manifest private and outside the checkout.

The recorded-native check uses the real default cockpit and a fresh managed history, then verifies retained exact sources across reopen at 120×32, 80×24 and 60×18. It compares full title and project fields from a temporary loaded-view export and opens sessions by human name or context. The inspector must identify the exact source and session again after returning with `Esc`. Regular named subagents remain eligible. Guardian reviews must stay out of new admission and the roster.

Add `--require-live` only with independently observed `pid` values for at least five distinct source and session pairs. The checker requires exact open device and inode matches. A single native process may hold several transcripts. The observed host sample had 12 non-guardian transcripts held by two Codex processes. That sample establishes open transcript ownership without deciding whether each session was Working or Inactive. Use an independently established `status` to check the roster state. Without `--require-live`, the result covers recorded native roots.

`historical` accepts one ordinary Claude record with `harness`, `path`, and `session`. Include `title` and `project` to check the full presentation at all three widths, and optionally `status`. It uses a separately configured store so managed retention does not retroactively admit stale sessions into the live cockpit. `mutation` creates disposable metadata records and checks an exact native title change, distinct sources with the same native ID, state catch-up, and restart. Its results do not count as real concurrent sessions.

To examine an identified running store, use `inspect-existing --db /path/to/history.sqlite`. It reports source and session IDs, counts and verified native classifications. It leaves private message bodies, native title text, credentials and full terminal frames outside its output.
