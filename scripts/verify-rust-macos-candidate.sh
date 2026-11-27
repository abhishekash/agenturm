#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    echo "Usage on an Apple Silicon Mac: $0 <agenturm-version-macos-aarch64.tar.gz>" >&2
    exit 2
fi
command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 1; }
archive=$1
name=${archive##*/}
[[ $name =~ ^agenturm-[0-9]+\.[0-9]+\.[0-9]+-macos-aarch64\.tar\.gz$ ]] || { echo "Unexpected macOS candidate archive" >&2; exit 2; }
archive_dir=$(cd "$(dirname "$archive")" && pwd -P)
archive="$archive_dir/$name"
(cd "$archive_dir" && shasum -a 256 --check "$name.sha256")

work=$(mktemp -d "${TMPDIR:-/tmp}/agenturm-macos-candidate.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
bundle_name=${name%.tar.gz}
tar -xzf "$archive" -C "$work" -- "$bundle_name"
bundle="$work/$bundle_name"
app="$bundle/bin/agenturm"
home="$work/home"
empty_path="$work/empty-path"
state="$work/state"
codex="$home/.codex/sessions/2026/11/20"
pi="$home/.pi/agent/sessions/project"
mkdir -p "$codex" "$pi" "$empty_path" "$state"

# Exercise the exact packaged executables with no Python, Rust, or inherited PATH.
isolated=(env -i HOME="$home" PATH="$empty_path" "$app")
version=$("${isolated[@]}" --version)
[[ $bundle_name == "agenturm-${version#agenturm }-macos-aarch64" ]] || { echo "Archive/application version mismatch" >&2; exit 1; }

cat > "$codex/session.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"bundle-codex-session"}}
{"type":"event_msg","timestamp":"2026-11-20T12:00:00Z","payload":{"type":"task_started","turn_id":"turn-1"}}
{"type":"token_usage_record","timestamp":"2026-11-20T12:00:02Z","payload":{"type":"token_usage_record","turn_id":"turn-1","response_id":"response-1","usage":{"input_tokens":7,"output_tokens":3,"total_tokens":10}}}
{"type":"event_msg","timestamp":"2026-11-20T12:00:03Z","payload":{"type":"task_complete","turn_id":"turn-1"}}
{"type":"event_msg","timestamp":"2026-11-20T12:00:04Z","payload":{"type":"task_started","turn_id":"turn-2"}}
JSONL
cat > "$pi/session.jsonl" <<'JSONL'
{"type":"session","id":"bundle-pi-session"}
{"type":"message","id":"assistant-1","timestamp":"2026-11-20T12:00:01Z","message":{"role":"assistant","stopReason":"stop","usage":{"input":8,"output":3},"content":[]}}
JSONL

config="$state/sources.json"
db="$home/Library/Application Support/Agenturm/Rust/history.sqlite"
mkdir -p "$(dirname "$db")"
"${isolated[@]}" discover --sessions-per-harness 1 > "$config"
"${isolated[@]}" collect --config "$config" --db "$db"
"${isolated[@]}" query --db "$db" > "$state/query.json"
jq -e '
  [.observations[] | select(.observation.kind == "usage") |
    [.identity.session_id, .observation.input_tokens, .observation.output_tokens]]
  | sort == [["bundle-codex-session", 7, 3], ["bundle-pi-session", 8, 3]]
' "$state/query.json" > /dev/null

"${isolated[@]}" collect --config "$config" --db "$db" > "$state/reopen.log"
"${isolated[@]}" query --db "$db" > "$state/reopened-query.json"
cmp "$state/query.json" "$state/reopened-query.json"
"${isolated[@]}" backup --db "$db" --output "$state/backup.sqlite"
"${isolated[@]}" query --db "$state/backup.sqlite" > "$state/backup-query.json"
cmp "$state/query.json" "$state/backup-query.json"

printf 'Verified native Apple Silicon candidate %s: isolated CLI, collection, query, reopen, and database backup\n' "$version"
