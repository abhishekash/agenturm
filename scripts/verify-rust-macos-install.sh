#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 || $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    echo "Usage on Apple Silicon macOS: $0 <current-archive.tar.gz> [previous-archive.tar.gz]" >&2
    exit 2
fi
current=$1
previous=${2:-}
version_from() {
    local filename=${1##*/}
    [[ $filename =~ ^agenturm-([0-9]+\.[0-9]+\.[0-9]+)-macos-aarch64\.tar\.gz$ ]] || return 1
    printf '%s\n' "${BASH_REMATCH[1]}"
}
current_version=$(version_from "$current") || { echo "Unexpected current archive name" >&2; exit 2; }
previous_version=
if [[ -n $previous ]]; then
    previous_version=$(version_from "$previous") || { echo "Unexpected previous archive name" >&2; exit 2; }
    [[ $current_version != "$previous_version" ]] || { echo "Archive versions must differ" >&2; exit 2; }
fi
archives=("$current")
[[ -z $previous ]] || archives+=("$previous")
for archive in "${archives[@]}"; do
    [[ -f $archive && ! -L $archive && -f $archive.sha256 ]] || { echo "Each archive requires a checksum sidecar" >&2; exit 2; }
    directory=$(cd "$(dirname "$archive")" && pwd -P)
    (cd "$directory" && shasum -a 256 --check "$(basename "$archive").sha256")
done

work=$(mktemp -d "${TMPDIR:-/tmp}/agenturm-macos-install.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
home="$work/user home"
empty_path="$work/empty executable path"
mkdir -p "$home" "$empty_path" "$work/current"
current_name=${current##*/}
current_bundle=${current_name%.tar.gz}
tar -xzf "$current" -C "$work/current" -- "$current_bundle"
if [[ -n $previous ]]; then
    previous_name=${previous##*/}
    previous_bundle=${previous_name%.tar.gz}
    mkdir "$work/previous"
    tar -xzf "$previous" -C "$work/previous" -- "$previous_bundle"
fi

bin_dir="$home/.local/bin"
mkdir -p "$bin_dir"
cat > "$bin_dir/agenturm" <<'SH'
#!/bin/sh
printf '%s\n' 'Existing Agenturm command'
SH
chmod 700 "$bin_dir/agenturm"
before=$(shasum -a 256 "$bin_dir/agenturm" | awk '{print $1}')
history="$home/Library/Application Support/Agenturm/Rust/history.sqlite"
mkdir -p "$(dirname "$history")"
printf 'qualification-history-retained\n' > "$history"

launcher="$bin_dir/agenturm-rust"
if [[ -n $previous ]]; then
    HOME="$home" bash "$work/previous/$previous_bundle/install-rust-macos.sh" install \
        --version "$previous_version" --archive "$previous" --checksum "$previous.sha256"
    [[ $(env -i HOME="$home" PATH="$empty_path" "$launcher" --version) == "agenturm $previous_version" ]]
    HOME="$home" bash "$work/current/$current_bundle/install-rust-macos.sh" update \
        --version "$current_version" --archive "$current" --checksum "$current.sha256"
else
    HOME="$home" bash "$work/current/$current_bundle/install-rust-macos.sh" install \
        --version "$current_version" --archive "$current" --checksum "$current.sha256"
fi
[[ $(env -i HOME="$home" PATH="$empty_path" "$launcher" --version) == "agenturm $current_version" ]]
[[ $(readlink "$launcher") == "$home/Library/Application Support/Agenturm/Rust/releases/agenturm-$current_version-macos-aarch64/bin/agenturm" ]]
[[ $(shasum -a 256 "$bin_dir/agenturm" | awk '{print $1}') == "$before" ]]

HOME="$home" bash "$work/current/$current_bundle/install-rust-macos.sh" uninstall
[[ ! -e $launcher && ! -L $launcher ]]
[[ $(cat "$history") == qualification-history-retained ]]
[[ $(shasum -a 256 "$bin_dir/agenturm" | awk '{print $1}') == "$before" ]]
if [[ -n $previous ]]; then
    printf 'Verified direct-download install and upgrade %s -> %s, launch without Python/toolchain, uninstall, retained history\n' "$previous_version" "$current_version"
else
    printf 'Verified direct-download install %s, launch without Python/toolchain, uninstall, retained history\n' "$current_version"
fi
