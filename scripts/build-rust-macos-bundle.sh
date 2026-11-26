#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    echo "Usage on an Apple Silicon Mac: $0 <output-directory>" >&2
    exit 2
fi

output=$1
mkdir -p -- "$output"
output=$(cd -- "$output" && pwd -P)
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd -- "$repo"

target=aarch64-apple-darwin
export MACOSX_DEPLOYMENT_TARGET=13.0
target_dir=${CARGO_TARGET_DIR:-"$repo/target"}
adapters=(codex pi claude kiro opencode)
binaries=(agenturm agenturm-plugin-host)
for adapter in "${adapters[@]}"; do binaries+=("agenturm-$adapter-harness"); done
build_args=()
for binary in "${binaries[@]}"; do build_args+=(--bin "$binary"); done
cargo build --release --locked --target "$target" --target-dir "$target_dir" "${build_args[@]}"

version=$("$target_dir/$target/release/agenturm" --version)
name="agenturm-${version#agenturm }-macos-aarch64"
if [[ -e "$output/$name.tar.gz" || -e "$output/$name.tar.gz.sha256" ]]; then
    echo "Bundle output already exists in $output" >&2
    exit 1
fi
staging=$(mktemp -d "$output/.agenturm-macos-bundle.XXXXXXXX")
trap 'rm -rf -- "$staging"' EXIT
bundle="$staging/$name"
mkdir -p "$bundle/bin"
for binary in "${binaries[@]}"; do
    install -m 0755 "$target_dir/$target/release/$binary" "$bundle/bin/$binary"
done
install -m 0644 LICENSE "$bundle/LICENSE"
install -m 0644 docs/rfcs/rust-macos-bundle.md "$bundle/README.md"
install -m 0755 scripts/install-rust-macos.sh "$bundle/install-rust-macos.sh"

python3 - "$bundle/agenturm-plugins.json" "$bundle/bin" "${adapters[@]}" <<'PY'
import hashlib, json, pathlib, sys
output, bindir, *adapters = sys.argv[1:]
plugins = []
for adapter in adapters:
    relative = f"bin/agenturm-{adapter}-harness"
    digest = hashlib.sha256((pathlib.Path(bindir) / relative.removeprefix("bin/")).read_bytes()).hexdigest()
    plugins.append({"plugin_id": f"example.rust-{adapter}-harness", "adapter": relative,
                    "sha256": digest, "protocol": 1,
                    "features": ["source-adapter/1", "configured-root/1"],
                    "observation_format": "agenturm-observations/1"})
pathlib.Path(output).write_text(json.dumps({"version": 1, "plugins": plugins}, indent=2) + "\n")
PY

cat > "$bundle/release-metadata.json" <<'JSON'
{
  "schema_version": 1,
  "target": "aarch64-apple-darwin",
  "architectures": ["arm64"],
  "minimum_macos": "13.0",
  "distribution": "direct-github-release",
  "signature": "unsigned",
  "notarization": "none",
  "integrity": "HTTPS download plus SHA-256 sidecar",
  "collection_mode": "foreground-cockpit",
  "unsupported": ["x86_64-apple-darwin", "Rosetta", "systemd", "managed-background-service", "Linux-only-process-inspection", "workspace-backup-restore", "private-content-capture"]
}
JSON

(cd "$staging" && tar -czf "$name.tar.gz" "$name" && shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256")
mv "$staging/$name.tar.gz" "$output/$name.tar.gz"
mv "$staging/$name.tar.gz.sha256" "$output/$name.tar.gz.sha256"
printf '%s\n' "$output/$name.tar.gz" "$output/$name.tar.gz.sha256"
