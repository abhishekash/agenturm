# Linux Rust host wheel checkpoint

Recorded 20 November 2026. This checkpoint packages the opt-in `agenturm-plugin-host` executable inside a Linux x86_64 platform wheel, while keeping the ordinary Agenturm source build and `uv tool install agenturm` path pure Python. It does not enable the Rust host by default or claim a performance gain.

## Build and use

Build the pinned Rust 1.100.0 host and create the native wheel on a Linux x86_64 glibc build machine:

```sh
cargo build --release --locked -p agenturm-plugin-host
AGENTURM_BUNDLE_RUST_HOST=/absolute/path/to/target/release/agenturm-plugin-host \
  uv build --wheel --out-dir dist/native
uv tool install /absolute/path/to/dist/native/agenturm-0.2.8-py3-none-linux_x86_64.whl
agenturm serve --plugin-config /absolute/path/to/plugins.toml \
  --use-bundled-rust-plugin-host
```

The last command explicitly selects the installed Rust host. `plugins.toml` still explicitly selects each child adapter; see the [Rust sample adapter guide](../../examples/rust_sample_harness_plugin/README.md). The existing explicit `--rust-plugin-host PATH --rust-plugin-host-sha256 DIGEST` option also remains available. No executable code is downloaded or compiled at first launch. The wheel build copies a prebuilt ELF, writes its SHA-256 and conservative build-host glibc floor into package metadata, and tags the wheel `py3-none-linux_x86_64`. Before Store creation, Agenturm checks the installed platform and glibc, then verifies the host hash; each worker start rechecks and copies the host and the separately pinned adapter. The wheel uses no Python ABI-specific extension.

| Platform | Install and host status |
| --- | --- |
| Linux x86_64, glibc at least 2.39 | Native wheel built and tested on Ubuntu 24.04 with a clean `uv tool` installation and real receiver. The CLI still uses Python transport unless the bundled-host flag is supplied. |
| Linux x86_64 with older glibc, other Linux architectures or musl | Python package and default transport remain available; the bundled Rust host is not qualified. The explicit glibc check refuses an incompatible bundled host before starting a receiver. |
| macOS, Windows | The pure Python distribution remains available. This checkpoint does not qualify a bundled Rust host or its process cleanup on these platforms. |

The CI workflow produces the Linux native wheel as a reviewable artifact. The tag-triggered PyPI publish workflow still builds the pure source distribution and `py3-none-any` wheel; the native wheel is **not published to PyPI** by this checkpoint. A normal `uv tool install agenturm` therefore continues to use the published Python package. Publishing native wheels requires separate portability and release review.

## Verification

| Check | Result |
| --- | --- |
| `cargo build --release --locked -p agenturm-plugin-host` | Passed with Rust 1.100.0. |
| `AGENTURM_BUNDLE_RUST_HOST=/tmp/agenturm-rust-process-host/target/release/agenturm-plugin-host uv build --wheel --out-dir /tmp/agenturm-rust-host-wheel/native-dist3` | Passed. Native wheel tagged `py3-none-linux_x86_64`, `Root-Is-Purelib: false`; the embedded executable matched `host.json` SHA-256. |
| `uv build --wheel --out-dir /tmp/agenturm-rust-host-wheel/pure-dist2` after a native build in the same tree | Passed. Pure wheel tagged `py3-none-any`, `Root-Is-Purelib: true`; no Rust host or metadata was included. This checks stale build-tree cleanup. |
| `uv build --out-dir /tmp/agenturm-rust-host-wheel/default-dist` with no bundle environment variable | Passed. Produced a source distribution and `py3-none-any` wheel with no host artifact, matching the release workflow's Python-compatible build path. |
| `python3 scripts/validate_linux_rust_host_wheel.py --native-wheel /tmp/agenturm-rust-host-wheel/native-dist3/agenturm-0.2.8-py3-none-linux_x86_64.whl --pure-wheel /tmp/agenturm-rust-host-wheel/pure-dist2/agenturm-0.2.8-py3-none-any.whl --adapter-bin /tmp/agenturm-rust-process-host/target/release/agenturm-sample-harness` | Passed. Both wheels were installed into separate, isolated `uv tool` directories from a directory outside the checkout. The native installed CLI reached a real `/api/health` Completed session through the bundled host with `UV_OFFLINE=1`; the pure CLI worked and reported no bundled host. |

This is a Linux qualification slice, not a multi-platform wheel release. The `linux_x86_64` tag itself does not encode the glibc floor; Agenturm refuses explicit bundled-host activation on older glibc, while its Python paths remain usable. The minimum glibc version for a broader portable native wheel has not been established by `auditwheel`. macOS and Windows build, process cleanup, installer and installed-client gates remain open.

After rebasing on main through #173, the release host and adapter were rebuilt, native then pure wheels were built sequentially, and the same isolated `uv tool` validator passed against `rebased-native` and `rebased-pure`. The build and test directories are disposable; their local paths identify the captured run, not a published distribution.
