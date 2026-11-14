# Typed investigation UI

14 November 2026 · Stage G cockpit implementation. A single modal lets an engineer query retained recorded paths, commands or user instructions and open the pinned exact source. It extends the existing Activity investigation loop. The coordinator owns the Activity entry point, command-palette action and API/Store integration.

## Read recorded evidence, then inspect its source

The default form has a query type, authored single-line query and Search. Optional exact-producer and recorded-failure filters are behind a keyboard-accessible Filters control. Search is explicit; editing a query cancels any pending old read and clears its old results. Path matching keeps the recorded spelling; command/instruction queries retain the API's literal phrase semantics.

Results show recorded text, producer and recorded result. They never invent an instruction, infer a persisted file effect from an attempted operation, or present absence as proof of no activity. The response's coverage and match semantics remain visible. Unsupported receiver capability, unavailable indexing, malformed replies and read errors produce an explicit unavailable state rather than an empty-success conclusion.

Enter opens the selected exact source in the same modal. Source text leads the reader; full source identifiers and owner/execution metadata follow it. Recorded fields are rendered as plain text with their field labels, never terminal markup or executable commands. Source ID, receiver, producer, actor, session, trace, span and turn remain routing metadata. Source availability and clipping are explicit.

## Keyboard and supported floor

| Context | Keys |
| --- | --- |
| Query | Type normally; Enter or Search reads; Tab/Shift+Tab moves controls. |
| Results | Up/Down chooses; Page Up/Down pages; Home/End first/last row; Enter opens the exact source. |
| More / Filters | Tab to the labelled control; Enter activates it. |
| Exact source | Up/Down, Page Up/Down and Home/End read; Escape returns to results. |
| Cancel / back | Ctrl+C cancels the current read or backs out; Escape closes results after leaving source. |

The 60×18 floor is tested using actual laid-out regions. The main form, result rows, More, Filters and Back remain visible. Extra filters are initially collapsed; source mode hides the form while retaining authored input. Shorter/smaller terminals get a resize message and a working Escape path. No source modal is stacked over another source modal. Global copy/export shortcuts are shadowed in this read-only flow; ordinary `c` and `x` continue to type normally in the query field.

## Exact-source and capture boundaries

Search runs `collect_investigation()` in a worker, with one bounded authenticated page at a time. Exact-source reads use `collect_source()` with the encoded original pinned reference. Receiver and every actual returned source entry's producer/actor/session/trace/span/turn must match. A correct original reference cannot authorize a foreign returned source record. There is no new namespace fallback or path-based execution.

The modal pins its receiver for its lifetime. Workers have generation and closed-screen fences, and publication rechecks known live policy. A 250 ms policy check watches every result reference, the selected original reference and every actual source scope. Capture denial, capture-revision changes or receiver replacement clear derived rows/source text, cancel the old generation and prevent late publication. Restoring capture does not revive an invalidated generation. A deliberate fresh Search can read currently permitted evidence in the same receiver; receiver replacement requires reopening. Authored query/filter input is retained separately.

Reads stop at 50 records per page, five pages, 250 displayed records and 2 MiB cumulative response payload. Duplicate events or repeated cursors stop the search. A page that would exceed the cumulative payload budget is not appended, and More is disabled with an explicit limit message. Source responses use the G helper's 64 KiB response cap; formatted field display is also bounded at 64 KiB / 800 fields. Control characters cannot become terminal commands. Source clipping is disclosed.

## Verification

The final focused gate passed **17 tests in 19.229 seconds**: nine UI/helper cases plus eight exact-source CLI cases. It includes actual ObservatoryApp rendering at 60×18, visible-control/result-region assertions, normal text entry, Home/End/Page navigation, source scrolling, same-modal return, five-page limits, cumulative payload limits, policy-revision invalidation, peer-only denial/restoration, wrong owner/execution/receiver, unsupported capability, blocked late search/source results and cancellation/close/reopen fences. All six actual source scope fields have negative tests for foreign values and omitted keys. A final two-case helper gate passed in 0.006 seconds after tightening formatted-source byte budgets. Results and source terminal frames were visually inspected at 60×18. The self-contained report passed 1280 px/light and 390 px/dark Chrome layout checks with no page overflow or external assets; mobile rendering was visually inspected.

Fixtures are synthetic. No personal telemetry, preferences, source review notes or existing task records are changed. Existing G CLI modules are dependencies; this delivery edits only the new UI module, its tests and this report. Root integration must guard duplicate opening, provide the existing Activity/palette entry point and validate against the actual Store API. This report makes no new index-cost or latency claim.
