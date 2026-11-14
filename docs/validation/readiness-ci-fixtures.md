# CI fixtures respect explicit derived readiness

This test-only follow-up to PR88 fixes three failing test identities reported by CI run `43597775376` on Python 3.11–3.14. It is based on `f124e2e` and does not relax product readiness, privacy, source-snapshot or receiver checks.

The real local-monitoring journey now waits, with at most 100 checks separated by 50 ms waits and an explicit migration-error check, for the receiver's normal asynchronous indexing to finish. Its metrics call is then an ordinary verified read rather than an implicit request to drain migration. The existing pause, exact failed-operation selection, trace navigation and return-to-Agents assertions remain.

Native storage tests can inspect raw text and persisted summary/activity/error-reference/Findings text in one read-only transaction without invoking a guarded public Sessions read or repairing pending data. Public Sessions inspection remains enabled by default. Only deliberately pending redaction and legacy-edit setup use the direct inspection separately.

The redaction regression still proves the sensitive original prompt is present before redaction. With refresh deliberately blocked, it then checks all inspected raw/derived text is scrubbed, summary and Findings caches contain zero rows, and public Sessions raises `IndexNotReady`. It verifies scrubbed text again after normal summary refresh, explicitly drains remaining Activity invalidations through the existing path, then retains its final ready/public Sessions privacy assertion. Nothing waits for refresh before checking immediate redaction.

The legacy-upgrade regression inspects its deliberately unsafe raw edit and asserts public Sessions is unavailable before upgrade. The upgraded Store must be ready, retain the durable metadata-only source policy and pass the original no-sensitive-text public assertion. Other native retry, old-writer rejection, usage-retention and exact-source checks remain intact.

## Verification

```sh
python -m unittest tests.test_native_storage_retry tests.test_local_monitoring -v
```

All **24 tests passed in 38.882 seconds** locally on Python 3.12.3. Independent read-only peer review found no blocker and confirmed the immediate privacy assertions are stronger, not deferred or removed. New-head integration CI remains a separate gate; this local pass does not claim all CI jobs or resource targets are green. No runtime files, personal telemetry, preferences, performance window or long soak were changed or used.
