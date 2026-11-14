# Certified MCP availability: integrated validation

MCP HTTP inventory could return unavailable while unrelated summaries were rebuilding, even though its complete source snapshot was readable. This change admits only unscoped/service/session raw inventory after initial preparation, with receiver and file identity certification before cached reads and inside fresh query snapshots. Agent, subtree and revision selectors remain strict, including blank or duplicate parameters. Healthy caching remains. Global history readiness is unchanged.

## Reviewed delivery

The integration starts at merged main105 `ab0a0496d41b84b21293a8b837f1645140575a8d`. It picks only MCP delivery `7306aeb3128a077bacfc9c66848db62e8203a135` and the two real HTTP diagnostic tests plus next-run registration `37b5cd15c07d6d664ab59fc936190f1732f7319d`. The runtime core/server bytes match the independently reviewed delivery exactly. The [source certificate report](mcp-initial-certificate.html) preserves the original failing cases, selector/cache/file races, genuine capture redaction and healthy-cache diagnostic.

The 63-case author gate passed in 37.014 seconds; independent peer verification ran 20 cases in 4.446 seconds on Python 3.12. These are their original results, not a later interpreter or resource certification. The initial broader adapter error and its read-only transaction-state forwarding remain recorded.

## Root integration checks

The first root Python 3.13 gate had 20 passing assertions in 6.450 seconds but seven unraisable SQLite ResourceWarnings at shutdown. It was **not warning-free acceptance**. A four-case allocation diagnostic identified existing transaction-only contexts at `tests/test_response_cache.py:20`, `:27` and `:42`: four setup connections, two journal-mode connections and one replacement connection. SQLite's transaction context commits or rolls back but does not close the handle.

The fixture correction adds explicit closing around those three contexts and keeps their original transaction ordering and every freshness/version/journal/replacement assertion. No production change or warning suppression was introduced. Independent peer review found no blocker.

After that correction, the normal Python 3.13 gate covering certificate/cache behavior and actual HTTP failures passed **22 tests in 7.307 seconds**. An explicit stderr check found no ResourceWarning or unraisable output. The real loopback HTTP pair also passed separately in 1.012 seconds before the fixture correction. These checks overlap and are not summed. Exact commands and original diagnostic/log hashes are retained in the [integration JSON](mcp-availability-integration.json).

## Functional run remains pending

The prior full-corpus attempt remains failed: history became ready, then active MCP returned 503 before any complete turn or restart. Its unretained response body prevents unique historical cause attribution. This independently proved dependency correction does not relabel that attempt or prove sustained availability.

The [next registration](next-retained-functional-registration.html) requires a new exact frozen application/harness, unchanged corpus and disk reserve, 60 seconds with unchanged two-second phases, and actual reconstruction after both receiver and cockpit replacements. Its template intentionally leaves future target hashes empty. Completed restart operations or elapsed duration alone cannot satisfy that coverage. No new full-corpus run, one-hour pass or resource result is claimed here.

Loaded HTTP cadence/p95, cold readiness, complete-monitor idle CPU, sustained memory/WAL/storage and live/human outcomes remain open. All checks use generated data without providers or personal discovery. Changes remain Unreleased.
