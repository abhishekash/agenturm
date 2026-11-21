# Diagnostic evidence in the inspector

The inspector Overview now includes the latest typed diagnostic beside the
diagnostic count. It reports the safe code first, then severity and source ID,
and its explicit scope (`source` or the associated session ID). It does not
add another pane or duplicate the diagnostic history list.

When multiple diagnostics are loaded, the displayed item is selected by event
time with stable source/event tie-breakers. The history query remains available
for older records. Source-scoped diagnostics remain excluded from session-only
queries, as verified by the store and query tests.

Validation covers source and session output values and the shared TUI renderer
at 80×24 and 60-column sizes, plus wide and too-small terminal layouts.
