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
install -m 0644 docs/install.md "$bundle/INSTALL.md"
install -m 0755 scripts/install-rust-macos.sh "$bundle/install-rust-macos.sh"

{
    printf '{"version":1,"plugins":[\n'
    separator=""
    for adapter in "${adapters[@]}"; do
        executable="bin/agenturm-$adapter-harness"
        digest=$(shasum -a 256 "$bundle/$executable" | awk '{print $1}')
        case "$adapter" in
            codex) discovery='"namespace":"codex","root_env":"CODEX_HOME","default_root":".codex/sessions","env_suffix":"sessions","shape":"codex_jsonl_tree"' ;;
            pi) discovery='"namespace":"pi","root_env":"PI_CODING_AGENT_DIR","default_root":".pi/agent/sessions","env_suffix":"sessions","shape":"jsonl_tree"' ;;
            claude) discovery='"namespace":"claude","root_env":"CLAUDE_CONFIG_DIR","default_root":".claude/projects","env_suffix":"projects","shape":"jsonl_tree"' ;;
            kiro) discovery='"namespace":"kiro","root_env":"KIRO_HOME","default_root":".kiro/sessions","env_suffix":"sessions","shape":"kiro_jsonl_tree"' ;;
            opencode) discovery='"namespace":"opencode","root_env":"OPENCODE_DB","default_root":".local/share/opencode/opencode.db","macos_default_root":"Library/Application Support/opencode/opencode.db","fallback_base_env":"XDG_DATA_HOME","fallback_suffix":"opencode/opencode.db","shape":"sqlite_file"' ;;
        esac
        printf '%s{"plugin_id":"example.rust-%s-harness","adapter":"%s","sha256":"%s","protocol":1,"features":["source-adapter/1","configured-root/1"],"observation_format":"agenturm-observations/1","discovery":{%s}}' \
            "$separator" "$adapter" "$executable" "$digest" "$discovery"
        separator=$',\n'
    done
    printf '\n]}\n'
} > "$bundle/agenturm-plugins.json"

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
