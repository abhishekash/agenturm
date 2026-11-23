#!/usr/bin/env bash
# Qualify the opt-in installer with an already-built native release archive.
set -euo pipefail
umask 077

if [[ $# -ne 1 || $(uname -s) != Linux ]]; then
    printf 'Usage on Linux: %s <bundle.tar.gz>\n' "$0" >&2
    exit 2
fi
archive=$(realpath -e -- "$1")
architecture=$(uname -m)
filename_pattern='^agenturm-([0-9]+\.[0-9]+\.[0-9]+)-linux-(x86_64|aarch64)\.tar\.gz$'
if [[ ! -f "$archive" || ! -f "$archive.sha256" || ! ${archive##*/} =~ $filename_pattern ]]; then
    printf 'Expected a versioned Linux archive and matching .sha256 sidecar\n' >&2
    exit 2
fi
version=${BASH_REMATCH[1]}
if [[ ${BASH_REMATCH[2]} != "$architecture" ]]; then
    printf 'Run the installer qualification on the archive’s native architecture\n' >&2
    exit 2
fi

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
scratch=$(mktemp -d -- "${TMPDIR:-/tmp}/agenturm installer check.XXXXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

home="$scratch/user home"
prefix="$scratch/managed releases"
bin_dir="$scratch/user commands"
config="$scratch/user configuration"
state="$scratch/user state"
empty_path="$scratch/empty executable path"
offline_bin="$scratch/offline command checks"
mkdir -p -- "$home" "$bin_dir" "$config" "$state/agenturm-rust" "$empty_path" "$offline_bin"

# Any accidental download, Python runtime, or service operation fails the fixture.
cat > "$offline_bin/blocked" <<'SH'
#!/bin/sh
printf '%s\n' "${0##*/}" >> "$FORBIDDEN_COMMANDS"
exit 97
SH
chmod 700 -- "$offline_bin/blocked"
for command in curl wget python python3 sudo systemctl loginctl; do
    ln -s -- blocked "$offline_bin/$command"
done
isolated=(env -i HOME="$home" XDG_DATA_HOME="$scratch/user data" XDG_CONFIG_HOME="$config"
          XDG_STATE_HOME="$state" TMPDIR="$scratch")
installer=("${isolated[@]}" PATH="$offline_bin:/usr/bin:/bin"
           FORBIDDEN_COMMANDS="$scratch/forbidden commands" bash "$script_dir/install-rust.sh")
program=("${isolated[@]}" PATH="$empty_path")
locations=(--prefix "$prefix" --bin-dir "$bin_dir")
release_options=(--version "$version" --archive "$archive" --checksum "$archive.sha256")

existing="$bin_dir/agenturm"
cat > "$existing" <<'SH'
#!/bin/sh
printf '%s\n' 'Existing Agenturm command'
SH
chmod 700 -- "$existing"
existing_hash=$(sha256sum < "$existing")
launcher="$bin_dir/agenturm-rust"
installation="$prefix/releases/${archive##*/}"
installation=${installation%.tar.gz}

[[ $("${installer[@]}" install "${locations[@]}" "${release_options[@]}") == "$launcher" ]]
[[ -L "$launcher" && $(readlink -- "$launcher") == "$installation/bin/agenturm" ]]
[[ $("${program[@]}" "$launcher" --version) == "agenturm $version" ]]
[[ $(sha256sum < "$existing") == "$existing_hash" && $("$existing") == 'Existing Agenturm command' ]]

adapters=(codex pi claude kiro opencode)
binaries=(agenturm agenturm-plugin-host agenturm-connector agenturm-storage-service)
for adapter in "${adapters[@]}"; do
    binaries+=("agenturm-$adapter-harness")
done
for binary in "${binaries[@]}"; do
    file="$installation/bin/$binary"
    [[ -f "$file" && ! -L "$file" && -x "$file" ]]
done
catalog="$installation/agenturm-plugins.json"
jq -e '.version == 1 and (.plugins | length == 5)' "$catalog" > /dev/null
for adapter in "${adapters[@]}"; do
    file="bin/agenturm-$adapter-harness"
    actual=$(sha256sum < "$installation/$file")
    jq -e --arg id "example.rust-$adapter-harness" --arg file "$file" --arg digest "${actual%% *}" '
        [.plugins[] | select(.plugin_id == $id and .adapter == $file and .sha256 == $digest
            and .protocol == 1 and .observation_format == "agenturm-observations/1")]
        | length == 1
    ' "$catalog" > /dev/null
done

span_file="$scratch/trace export.jsonl"
cat > "$span_file" <<'JSONL'
{"trace_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","span_id":"bbbbbbbbbbbbbbbb","start_unix_nano":"100","end_unix_nano":"200","status":"OK","attributes":{"session.id":"installer-session"}}
JSONL
check_history() {
    "${program[@]}" "$launcher" query --db "$1" --harness-id otlp.installer --session-id installer-session |
        jq -e '[.observations[] | select(.observation.kind == "trace_span") |
            [.harness_id, .identity.session_id, .observation.trace_id, .observation.span_id]] ==
            [["otlp.installer", "installer-session", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"]]' > /dev/null
}

database="$state/agenturm-rust/history.sqlite"
extra_database="$installation/operator history.sqlite"
for file in "$database" "$extra_database"; do
    "${program[@]}" "$launcher" import-jsonl --file "$span_file" --db "$file" --source-id otlp.installer
    [[ -s "$file" ]]
    check_history "$file"
done
database_hashes=$(sha256sum -- "$database" "$extra_database")
printf '%s\n' 'Existing user configuration' > "$config/operator settings"

[[ $("${installer[@]}" update "${locations[@]}" "${release_options[@]}") == "$launcher" ]]
[[ $(readlink -- "$launcher") == "$installation/bin/agenturm" ]]
[[ $("${program[@]}" "$launcher" --version) == "agenturm $version" ]]
check_history "$database"
check_history "$extra_database"
[[ $(sha256sum -- "$database" "$extra_database") == "$database_hashes" ]]

"${installer[@]}" uninstall "${locations[@]}"
[[ ! -e "$launcher" && ! -L "$launcher" ]]
for binary in "${binaries[@]}"; do
    file="$installation/bin/$binary"
    [[ ! -e "$file" && ! -L "$file" ]]
done
[[ ! -e "$catalog" && ! -L "$catalog" && ! -e "$installation/.install-receipt" ]]
[[ $(sha256sum -- "$database" "$extra_database") == "$database_hashes" ]]
[[ $(cat -- "$config/operator settings") == 'Existing user configuration' ]]
[[ $(sha256sum < "$existing") == "$existing_hash" && $("$existing") == 'Existing Agenturm command' ]]
[[ ! -e "$scratch/forbidden commands" ]]
printf 'Verified offline agenturm-rust %s installer on Linux %s, including replay and retained history\n' "$version" "$architecture"
