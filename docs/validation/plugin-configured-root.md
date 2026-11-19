# Configured source root checkpoint

19 November 2026. A process source can opt into an explicitly configured filesystem root without adding a harness-specific path rule to the host. Its manifest must provide `configured-root/1` and request the `configured-source-read` grant. Its named TOML source row may set an absolute `root`. The receiver validates the value and manifest before Store creation, and gives only that source process an `AGENTURM_PLUGIN_SOURCE_ROOT` environment variable. The process host continues to start with a minimal environment and verifies the executable hash at launch.

The root is a path parameter, not an access restriction: the child runs with the user's OS permissions and can access other files. The host does not scan the configured path or infer a harness from it. The adapter must handle a missing or changing path and preserve resumable cursors. No packaged Pi adapter or full native evidence parity is claimed by this checkpoint.

Validation: 27 focused configuration, process, supervision and runtime tests passed. A real child fixture verified the exact configured root reached it; missing feature, relative, oversized and malformed path cases were rejected. `compileall`, HTML parsing and `git diff --check` passed. The next gate is a separately packaged source adapter exercising this path and the existing ownership switch.
