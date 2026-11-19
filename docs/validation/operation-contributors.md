# Operation contribution checkpoint

The generic span-to-activity projection now asks a trusted `OperationContributorRegistry` for source-specific facts. OpenCode's legacy shell-command and exit-code compatibility rule lives in `observatory/adapters/opencode_operation.py`; current capture-version OpenCode records continue to suppress a duplicate generic detail body. The registry can accept a separately supplied contributor without another branch in `core.py`.

This is not the external plugin API. Contributors run in process, receive span attributes, and must be trusted. The future capability/adapter host will pass sanitized scoped facts across its process boundary.

Validation: 40 contributor, operation-hydration and OpenCode regression tests passed locally on Python 3.12; the earlier 60-test source/schema set also passed. No storage schema or public API changed.
