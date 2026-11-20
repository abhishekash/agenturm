# Member-scoped process source gaps

Recorded 20 November 2026. This checkpoint narrows a gap from one Codex directory file to that file's recorded sessions when the receiver can prove every current session's member ownership. The Python Store remains the only writer, policy authority, and acknowledgment authority. The process adapter remains opt-in; the bundled Codex reader remains the default. No performance gain is claimed.

## Receiver-owned contract

A `source-discovery/1` batch carries one validated, opaque 64-character member ID. Admission writes that member ID on each evidence row and binds each native session ID to one member in `plugin_session_member_v1`. The evidence, ownership, source cursor, member cursor, dirty sequence, and gap invalidation commit in one SQLite transaction before the worker receives an acknowledgment. Revisions cannot move an event to another member. A native session ID cannot move between members, including after its evidence has been removed by retention. Capture revocation erases ownership with the evidence and projection. The durable ownership table is capped at 2,000 sessions per source; admission refuses a new identity at the cap before advancing the cursor.

A member gap invalidates prior evidence sessions owned by that member, including any still-authoritative projected session whose evidence is absent but whose durable binding remains. Their state becomes Unknown with `source_gap`, usage and relationship are cleared, and their Activity is suppressed. A healthy sibling stays visible. Existing retention invalidation keeps `retention_gap` precedence; a pruned session is already Unknown even if no evidence rows remain. New sessions admitted after a gap can be shown, while an old invalidated ID remains Unknown.

## Legacy and damaged provenance

The schema adds `member_id=''` to older evidence rows. It performs no startup scan or backfill over historical Stores. Single-file process sources keep their existing source-wide gap behavior and do not consume the new ownership capacity. If retained legacy evidence, an ambiguous binding, a mismatched binding, or a still-authoritative projected row without ownership exists, a member gap falls back to source-wide invalidation. This conservative fallback also applies when a directory reader from the previous release resumes its old evidence. A later member batch may correct an old unowned session, but its binding and new evidence remain explicitly ambiguous (`member_id=''`), so a future member gap still invalidates the whole source. A previously owned native session ID cannot move to another member. Disabling and re-enabling capture removes old evidence and permits precise ownership for **future** facts, without replaying old history.

The member ID is a hash of the configured relative file path, not a filesystem security boundary. The host validates consistency and provenance, but it cannot independently prove the adapter's file-to-session claim. A missing/unreadable discovery root still reports degraded source health without rewriting prior session outcomes; that separate cutover gate remains open.

## Verification

Store tests cover two members, removal and rewrite, exact replay, stale lease, an actual previous-schema upgrade, source-wide legacy fallback, capped ownership, cross-member rejection, retention pruning, stale projection without evidence, capture revocation, and Activity/usage/relationship projection. Installed Codex directory tests verify that a missing or rewritten file leaves its healthy sibling Completed with Activity intact. The read-only Rust reducer's version 3 source-gap fixture remains unchanged: its input is the host's durable invalidated session IDs, regardless of which member caused them.

```sh
uv run python -m unittest tests.test_plugin_discovery tests.test_plugin_source_gap \
  tests.test_plugin_admission tests.test_plugin_projection tests.test_plugin_runtime \
  tests.test_rust_shadow_snapshot -q
AGENTURM_RUST_CODEX_BIN=/path/to/agenturm-codex-harness \
  uv run python -m unittest tests.test_rust_codex_directory_plugin -q
```

Full native-to-process Review/export parity, whole-root failure disposition, Rust storage authority, and the default reader cutover remain separate gates.
