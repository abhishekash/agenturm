#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    echo "Usage on an Apple Silicon Mac: $0 <agenturm-version-macos-aarch64.tar.gz>" >&2
    exit 2
fi
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
python3 - "$state/query.json" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))["observations"]
usage = sorted((row["identity"]["session_id"], row["observation"]["input_tokens"],
                row["observation"]["output_tokens"])
               for row in rows if row["observation"]["kind"] == "usage")
assert usage == [("bundle-codex-session", 7, 3), ("bundle-pi-session", 8, 3)], usage
PY
source_id=$(python3 - "$config" <<'PY'
import json, sys
config = json.load(open(sys.argv[1]))
print(next(source["source_id"] for source in config["sources"]
           if source["plugin_id"] == "example.rust-codex-harness"))
PY
)

"${isolated[@]}" collect --config "$config" --db "$db" > "$state/reopen.log"
"${isolated[@]}" query --db "$db" > "$state/reopened-query.json"
cmp "$state/query.json" "$state/reopened-query.json"
"${isolated[@]}" backup --db "$db" --output "$state/backup.sqlite"
"${isolated[@]}" query --db "$state/backup.sqlite" > "$state/backup-query.json"
cmp "$state/query.json" "$state/backup-query.json"

python3 "$(dirname "$0")/validate_rust_default_cockpit.py" \
    --agenturm "$app" --home "$home" --empty-path "$empty_path" --history "$db" \
    --transcript "$state/inspect-pty.txt" --inspect-source "$source_id"
printf 'Verified native Apple Silicon candidate %s: isolated CLI, collection, query, reopen, database backup, inspector navigation and terminal restoration\n' "$version"
