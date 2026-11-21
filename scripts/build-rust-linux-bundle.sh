#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || $(uname -s) != Linux ]]; then
    echo "Usage on Linux: $0 <output-directory>" >&2
    exit 2
fi

output=$1
mkdir -p -- "$output"
output=$(cd -- "$output" && pwd -P)
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd -- "$repo"

target_dir=${CARGO_TARGET_DIR:-"$repo/target"}
binaries=(agenturm agenturm-plugin-host agenturm-codex-harness agenturm-pi-harness)
build_args=()
for binary in "${binaries[@]}"; do
    build_args+=(--bin "$binary")
done
cargo build --release --locked --target-dir "$target_dir" "${build_args[@]}"

version=$("$target_dir/release/agenturm" --version)
name="agenturm-${version#agenturm }-linux-$(uname -m)"
if [[ -e "$output/$name.tar.gz" || -e "$output/$name.tar.gz.sha256" ]]; then
    echo "Bundle output already exists in $output" >&2
    exit 1
fi
staging=$(mktemp -d -- "$output/.agenturm-bundle.XXXXXXXX")
trap 'rm -rf -- "$staging"' EXIT
bundle="$staging/$name"
mkdir -p -- "$bundle/bin"
for binary in "${binaries[@]}"; do
    install -m 0755 -- "$target_dir/release/$binary" "$bundle/bin/$binary"
done
install -m 0644 -- LICENSE "$bundle/LICENSE"
install -m 0644 -- docs/rfcs/rust-linux-bundle.md "$bundle/README.md"
install -m 0644 -- docs/rfcs/rust-linux-bundle.html "$bundle/README.html"

codex_sha=$(sha256sum < "$bundle/bin/agenturm-codex-harness")
pi_sha=$(sha256sum < "$bundle/bin/agenturm-pi-harness")
cat > "$bundle/agenturm-plugins.json" <<JSON
{
  "version": 1,
  "plugins": [
    {
      "plugin_id": "example.rust-codex-harness",
      "adapter": "bin/agenturm-codex-harness",
      "sha256": "${codex_sha%% *}",
      "protocol": 1,
      "features": ["source-adapter/1", "configured-root/1"],
      "observation_format": "agenturm-observations/1"
    },
    {
      "plugin_id": "example.rust-pi-harness",
      "adapter": "bin/agenturm-pi-harness",
      "sha256": "${pi_sha%% *}",
      "protocol": 1,
      "features": ["source-adapter/1", "configured-root/1"],
      "observation_format": "agenturm-observations/1"
    }
  ]
}
JSON

(
    cd -- "$staging"
    tar -czf "$name.tar.gz" -- "$name"
    sha256sum -- "$name.tar.gz" > "$name.tar.gz.sha256"
)
ln -T -- "$staging/$name.tar.gz" "$output/$name.tar.gz"
ln -T -- "$staging/$name.tar.gz.sha256" "$output/$name.tar.gz.sha256"
printf '%s\n' "$output/$name.tar.gz" "$output/$name.tar.gz.sha256"
