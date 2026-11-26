# Rust configured harness collection

The Rust CLI can load version 1 or version 2 JSON configuration and poll up to
16 process adapters in one kernel refresh:

```sh
agenturm collect --config ./agenturm-sources.json --db ./agenturm.sqlite
```

Add `--watch` to keep process adapters and the store open between polls. The
default interval is 2 seconds; `--interval-ms` accepts 100 through 60000:

```sh
agenturm collect --config ./agenturm-sources.json --db ./agenturm.sqlite --watch --interval-ms 1000
```

`Ctrl-C` and `SIGTERM` stop the foreground collector cleanly and close its
supervised adapter processes. Configuration is loaded once at startup; restart
the command to apply edits.

Version 1 enumerates plugin identities and executable paths directly. Version 2
selects plugin identities from a separate, explicit catalog. Configuration and
catalog paths may be absolute or relative to the containing file's directory.
Relative paths are resolved before any child process starts.
Source IDs must be unique. Existing adapter and source-root paths are
canonicalized when checking ownership; missing paths use lexical normalization.
The same normalized adapter plus source-root pair cannot be listed twice under
different IDs.

```json
{
  "version": 1,
  "plugin_host": "/opt/agenturm/bin/agenturm-plugin-host",
  "sources": [
    {
      "plugin_id": "example.rust-pi-harness",
      "adapter": "/opt/agenturm/bin/agenturm-pi-harness",
      "source_root": "/home/alex/.pi/agent/sessions",
      "source_id": "harness.pi"
    },
    {
      "plugin_id": "example.rust-codex-harness",
      "adapter": "/opt/agenturm/bin/agenturm-codex-harness",
      "source_root": "/home/alex/.codex/sessions",
      "source_id": "harness.codex"
    }
  ]
}
```

Version 2 keeps executable paths and integrity pins in a separate catalog. The
catalog is user-managed JSON; it is not downloaded or discovered automatically.
Each entry declares the plugin transport protocol, required source-adapter
features, and a lower-case SHA-256 pin. The `observation_format` field selects
the host's batch decoder. The supported format is `agenturm-observations/1`,
which also applies when a catalog entry omits the field or when collection
uses version 1 configuration. The app rejects unsupported formats and
verifies catalog artifacts before opening the SQLite store, then checks the
selected adapter artifact again before each supervised host launch. The host
handshake must match the catalog's declared feature set.

Bundled entries may also declare a `discovery` contribution. `agenturm discover`
uses these declarations to resolve a source root and scan candidate files;
the adapter executable still owns format parsing and emits the shared
`agenturm-observations/1` contract. A new compatible harness can join default
discovery by shipping a catalog entry with a unique namespace, root rule and
one supported scan shape, without adding a provider branch to the app. The
shape describes only generic host discovery (`jsonl_tree`,
`codex_jsonl_tree`, `kiro_jsonl_tree`, or `sqlite_file`). Source-specific
semantics remain in the adapter. Catalogs without a `discovery` field remain
valid for explicitly configured collection.

```json
{
  "version": 2,
  "plugin_host": "/opt/agenturm/bin/agenturm-plugin-host",
  "plugin_catalog": "agenturm-plugins.json",
  "sources": [
    {
      "plugin_id": "example.rust-pi-harness",
      "source_root": "/home/alex/.pi/agent/sessions",
      "source_id": "harness.pi"
    }
  ]
}
```

```json
{
  "version": 1,
  "plugins": [
    {
      "plugin_id": "example.rust-pi-harness",
      "adapter": "/opt/agenturm/bin/agenturm-pi-harness",
      "sha256": "<64 lowercase hexadecimal characters>",
      "protocol": 1,
      "features": ["source-adapter/1", "configured-root/1"],
      "observation_format": "agenturm-observations/1",
      "discovery": {
        "namespace": "pi",
        "root_env": "PI_CODING_AGENT_DIR",
        "default_root": ".pi/agent/sessions",
        "env_suffix": "sessions",
        "shape": "jsonl_tree"
      }
    }
  ]
}
```

Each collection cycle performs one bounded poll per configured source. The
kernel validates each source batch, writes it to the Rust SQLite store, and ACKs
only after the durable commit. A startup or poll failure for one source is reported in the
summary while healthy configured sources continue. In watch mode, failed source
startup is retried at most once per second until it starts or the collector stops. Any source failure makes
the command exit nonzero; successful sources still retain their committed
observations. A durable history read failure is reported as a storage failure
and also makes the command exit nonzero, even if source commits and ACKs already
succeeded. The JSON files are user-managed configuration inputs. Watch mode
runs in the foreground; the command does not scan plugin directories or load
in-process native libraries. Catalog pins detect artifact changes but do not
sandbox adapters: each child runs with the same OS user privileges as Agenturm.

Adapter processes parse harness-specific records and emit normalized batches
in the configured observation format. The host projects usage and privacy-safe
tool, lifecycle, check, message-role and diagnostic observations. Message
bodies and other unsupported fields cause batch rejection. The catalog binds
each configured process to an observation format supported by the host.


### Rolling discovery configuration

Run `agenturm discover --follow [--sessions-per-harness <1..8>]` to generate
a version 2 configuration with a `follow` object in place of `sources`.
It captures the registered discovery targets from the selected bundled
catalog. Relative roots in a hand-written configuration are resolved against
the configuration file, and each target must match its pinned catalog
declaration. Older follow objects with named built-in roots remain readable.
An example follows.

```json
{
  "version": 2,
  "plugin_host": "/opt/agenturm/bin/agenturm-plugin-host",
  "plugin_catalog": "/opt/agenturm/agenturm-plugins.json",
  "follow": {
    "targets": [
      {
        "namespace": "pi",
        "plugin_id": "example.rust-pi-harness",
        "root": "/home/alex/.pi/agent/sessions",
        "shape": "jsonl_tree"
      }
    ],
    "sessions_per_harness": 4
  }
}
```

One-shot collection scans once. Foreground watch and the managed collector
rescan at most once every five seconds and admit new files from the bounded
set. Adapter processes for unchanged paths continue polling. Evicted paths
take their final bounded poll before eviction. They retain history and
resume from their durable cursor if selected again. Backlog beyond the last
poll remains on disk until reselection. A scan error preserves the current selection and is reported in the next
collection summary. Put the database directory outside both roots.
