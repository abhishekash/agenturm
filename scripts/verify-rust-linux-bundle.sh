#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: $0 <bundle.tar.gz> [--previous <bundle.tar.gz>] [--systemd-user]" >&2
    exit 2
}

[[ $# -gt 0 && $(uname -s) == Linux ]] || usage
archive=$1
shift
previous=
managed=0
while (($#)); do
    case "$1" in
        --previous)
            [[ $# -ge 2 && -z $previous ]] || usage
            previous=$2
            shift 2
            ;;
        --systemd-user)
            ((managed == 0)) || usage
            managed=1
            shift
            ;;
        *) usage ;;
    esac
done

scratch=$(mktemp -d)
unit=agenturm-collector.service
installed=0
watch_pid=
stop_watcher() {
    local status=0
    kill -TERM "$watch_pid" 2>/dev/null || true
    wait "$watch_pid" || status=$?
    watch_pid=
    return "$status"
}
cleanup() {
    if [[ -n $watch_pid ]]; then
        stop_watcher || true
    fi
    if ((installed)); then
        if ! "$agenturm" service uninstall; then
            echo "Service cleanup failed. Verification state remains at $scratch" >&2
            return 1
        fi
    fi
    rm -rf -- "$scratch"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

test_home="$scratch/home"
state="$scratch/state"
empty_path="$scratch/empty-path"
codex="$test_home/.codex/sessions/2026/11/20"
pi="$test_home/.pi/agent/sessions/project"
mkdir -p -- "$codex" "$pi" "$state" "$empty_path"

extract() {
    local archive=$1 destination=$2 name expected actual version
    name=$(basename -- "$archive" .tar.gz)
    [[ $name == agenturm-*-linux-"$(uname -m)" && -f $archive ]] || {
        echo "Unexpected bundle name for this Linux architecture: $archive" >&2
        return 1
    }
    expected=$(cat -- "$archive.sha256") || return 1
    actual=$(sha256sum < "$archive") || return 1
    if [[ $expected != "${actual%% *}  $name.tar.gz" ]]; then
        echo "Bundle checksum mismatch: $archive" >&2
        return 1
    fi
    mkdir -p -- "$destination" || return 1
    tar -xzf "$archive" -C "$destination" || return 1
    local executable="$destination/$name/bin/agenturm"
    version=$(env -i HOME="$test_home" PATH="$empty_path" "$executable" --version) || return 1
    if [[ $name != "agenturm-${version#agenturm }-linux-$(uname -m)" ]]; then
        echo "Archive and executable version disagree: $archive, $version" >&2
        return 1
    fi
    printf '%s\n' "$executable"
}
agenturm=$(extract "$archive" "$scratch/current")
isolated=(env -i HOME="$test_home" PATH="$empty_path" "$agenturm")

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

expect_usage() {
    jq -e '
        [.observations[] | select(.observation.kind == "usage") |
          [.identity.session_id, .observation.input_tokens, .observation.output_tokens]]
        | sort == [["bundle-codex-session", 7, 3], ["bundle-pi-session", 8, 3]]
    ' > /dev/null
}

expect_replay() {
    local log=$1
    grep -qE '^harness\.codex\..*: committed 0 observation\(s\).*ACK sent=true$' "$log"
    grep -qE '^harness\.pi\..*: committed 0 observation\(s\).*ACK sent=true$' "$log"
}

publish_fixture_files() {
    local target_codex=$1 target_pi=$2
    cp -- "$codex/session.jsonl" "$target_codex/new.tmp"
    cp -- "$pi/session.jsonl" "$target_pi/new.tmp"
    mv -- "$target_codex/new.tmp" "$target_codex/new.jsonl"
    mv -- "$target_pi/new.tmp" "$target_pi/new.jsonl"
}

inspect_history() {
    local database=$1
    local transcript="$state/inspector.txt"
    # Run on a PTY with the same empty PATH as the foreground collector.
    (
        sleep 1
        printf g
        sleep 0.3
        printf t
        sleep 0.3
        printf '\033[B'
        sleep 0.3
        printf '\r'
        sleep 0.3
        printf '\033'
        sleep 0.3
        printf q
    ) |
        AGENTURM_TEST_HOME="$test_home" AGENTURM_TEST_PATH="$empty_path" \
        AGENTURM_TEST_BIN="$agenturm" AGENTURM_TEST_DB="$database" \
        timeout 15s script -eq -c '
            stty rows 28 cols 120
            exec env -i HOME="$AGENTURM_TEST_HOME" PATH="$AGENTURM_TEST_PATH" TERM=xterm-256color "$AGENTURM_TEST_BIN" inspect --db "$AGENTURM_TEST_DB"
        ' "$transcript" > /dev/null
    # Strip terminal control sequences when matching the rendered value.
    local screen="$state/inspector-screen.txt"
    sed -E $'s/\033\\[[0-?]*[ -\\/]*[@-~]//g' "$transcript" > "$screen"
    grep -Eq 'Sessions observed[[:space:]]*2([[:space:]│]|$)' "$screen"
    grep -q 'select · Enter open · Esc back' "$screen"
    grep -q 'Turn activity' "$screen"
    grep -Eq 'Lifecycle[[:space:]]*completed · codex_task_complete' "$screen"
}

config="$state/sources.json"
db="$state/history.sqlite"
"${isolated[@]}" discover --sessions-per-harness 1 > "$config"
jq -e '[.sources[].plugin_id] | sort == ["example.rust-codex-harness", "example.rust-pi-harness"]' "$config" > /dev/null
"${isolated[@]}" collect --config "$config" --db "$db"
"${isolated[@]}" query --db "$db" > "$state/query.json"
expect_usage < "$state/query.json"

"${isolated[@]}" collect --config "$config" --db "$db" > "$state/restart.log"
expect_replay "$state/restart.log"
"${isolated[@]}" query --db "$db" > "$state/restarted-query.json"
cmp -- "$state/query.json" "$state/restarted-query.json"
inspect_history "$db"
"${isolated[@]}" backup --db "$db" --output "$state/backup.sqlite"
"${isolated[@]}" query --db "$state/backup.sqlite" | expect_usage
echo "Verified extracted bundle, foreground restart, both sessions in the inspector, and backup"

wait_for_live_check() {
    local attempt
    for ((attempt=0; attempt<100; attempt++)); do
        if "$@"; then
            return
        fi
        kill -0 "$watch_pid" 2>/dev/null || return 1
        sleep 0.2
    done
    return 1
}

query_fixture_usage() {
    "${isolated[@]}" query --db "$1" 2>/dev/null | expect_usage
}

query_empty_history() {
    "${isolated[@]}" query --db "$1" 2>/dev/null | jq -e '.observations == []' > /dev/null
}

verify_default_cockpit() {
    local cockpit_home="$scratch/cockpit-home"
    local cockpit_codex="$cockpit_home/.codex/sessions/2026/11/20"
    local cockpit_pi="$cockpit_home/.pi/agent/sessions/project"
    local cockpit_db="$cockpit_home/.local/state/agenturm-rust/history.sqlite"
    local cockpit_config="$state/cockpit-sources.json"
    local transcript="$state/cockpit.txt"
    local keyboard="$state/cockpit-keys" keyboard_fd status=0
    local -a cockpit_isolated=(env -i HOME="$cockpit_home" PATH="$empty_path" "$agenturm")
    mkdir -p -- "$cockpit_codex" "$cockpit_pi"
    mkfifo -- "$keyboard"
    : > "$transcript"
    exec {keyboard_fd}<>"$keyboard"
    AGENTURM_TEST_HOME="$cockpit_home" AGENTURM_TEST_PATH="$empty_path" AGENTURM_TEST_BIN="$agenturm" \
        timeout --foreground --kill-after=5s 70s script -efq -c '
            stty rows 18 cols 60
            exec env -i HOME="$AGENTURM_TEST_HOME" PATH="$AGENTURM_TEST_PATH" TERM=xterm-256color "$AGENTURM_TEST_BIN"
        ' "$transcript" < "$keyboard" > /dev/null &
    watch_pid=$!
    if ! wait_for_live_check grep -aFq 'Agenturm  /  Agents' "$transcript" ||
        ! wait_for_live_check query_empty_history "$cockpit_db"; then
        echo "Bare cockpit did not open the empty Agents workspace" >&2
        return 1
    fi

    publish_fixture_files "$cockpit_codex" "$cockpit_pi"
    if ! wait_for_live_check query_fixture_usage "$cockpit_db"; then
        echo "Bare cockpit did not collect both newly arrived sessions" >&2
        return 1
    fi
    printf r >&"$keyboard_fd"
    wait_for_live_check grep -aFq 'bundle-codex-session' "$transcript"
    wait_for_live_check grep -aFq 'bundle-pi-session' "$transcript"
    printf q >&"$keyboard_fd"
    wait "$watch_pid" || status=$?
    watch_pid=
    exec {keyboard_fd}>&-
    if ((status != 0)); then
        echo "Bare cockpit did not quit cleanly" >&2
        return 1
    fi

    "${cockpit_isolated[@]}" discover --sessions-per-harness 1 > "$cockpit_config"
    "${cockpit_isolated[@]}" collect --config "$cockpit_config" --db "$cockpit_db" > "$state/cockpit-replay.log"
    expect_replay "$state/cockpit-replay.log"
    query_fixture_usage "$cockpit_db"
    echo "Verified bare cockpit opened empty, collected both sessions, and released the writer on quit"
}
verify_default_cockpit

verify_follow() {
    local follow_home="$scratch/follow-home"
    local follow_codex="$follow_home/.codex/sessions/2026/11/20"
    local follow_pi="$follow_home/.pi/agent/sessions/project"
    local follow_config="$state/follow-sources.json"
    local follow_db="$state/follow.sqlite"
    local watch_log="$state/follow.log"
    local -a follow_isolated=(env -i HOME="$follow_home" PATH="$empty_path" "$agenturm")
    local attempt
    mkdir -p -- "$follow_codex" "$follow_pi"
    "${follow_isolated[@]}" discover --follow --sessions-per-harness 1 > "$follow_config"

    timeout --foreground --kill-after=5s 50s "${follow_isolated[@]}" collect \
        --config "$follow_config" --db "$follow_db" --watch --interval-ms 100 > "$watch_log" 2>&1 &
    watch_pid=$!
    for ((attempt=0; attempt<100; attempt++)); do
        if grep -qx '0 sources checked; 0 source failures; 0 global failures' "$watch_log"; then
            break
        fi
        if ! kill -0 "$watch_pid" 2>/dev/null; then
            break
        fi
        sleep 0.2
    done
    if ! grep -qx '0 sources checked; 0 source failures; 0 global failures' "$watch_log"; then
        echo "Follow collector did not start with empty roots" >&2
        cat -- "$watch_log" >&2
        return 1
    fi
    "${follow_isolated[@]}" query --db "$follow_db" | jq -e '.observations == []' > /dev/null

    publish_fixture_files "$follow_codex" "$follow_pi"
    for ((attempt=0; attempt<100; attempt++)); do
        if "${follow_isolated[@]}" query --db "$follow_db" | expect_usage; then
            break
        fi
        if ! kill -0 "$watch_pid" 2>/dev/null; then
            attempt=100
            break
        fi
        sleep 0.2
    done
    if ((attempt == 100)) || ! stop_watcher; then
        echo "Follow collector did not admit new sessions or stop cleanly" >&2
        cat -- "$watch_log" >&2
        return 1
    fi
    grep -qE '^Stopped after [0-9]+ collection cycle\(s\)\.$' "$watch_log"
    "${follow_isolated[@]}" query --db "$follow_db" | expect_usage
    echo "Verified follow discovery collected both new sessions without a restart"
}
verify_follow

if [[ -n $previous ]]; then
    prior=$(extract "$previous" "$scratch/prior")
    prior_isolated=(env -i HOME="$test_home" PATH="$empty_path" "$prior")
    prior_db="$state/upgrade.sqlite"
    prior_config="$state/prior-sources.json"
    "${prior_isolated[@]}" discover --sessions-per-harness 1 > "$prior_config"
    "${prior_isolated[@]}" collect --config "$prior_config" --db "$prior_db"
    "${prior_isolated[@]}" query --db "$prior_db" | expect_usage
    # Rediscover for the replacement catalog, then open the old DB as a writer.
    "${isolated[@]}" collect --config "$config" --db "$prior_db" > "$state/upgrade.log"
    expect_replay "$state/upgrade.log"
    "${isolated[@]}" query --db "$prior_db" | expect_usage
    inspect_history "$prior_db"
    echo "Verified prior bundle history and cursor after opening with the current bundle"
fi

if ((managed)); then
    installed_unit="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$unit"
    if [[ -e $installed_unit || -L $installed_unit ||
        $(systemctl --user show "$unit" --property=LoadState --value) != not-found ]]; then
        echo "Managed verification requires a working user manager and an unused $unit" >&2
        exit 1
    fi

    managed_db="$state/managed.sqlite"
    wait_for_collection() {
        local attempt
        for ((attempt=0; attempt<30; attempt++)); do
            if "${isolated[@]}" query --db "$managed_db" 2>/dev/null | expect_usage; then
                return
            fi
            sleep 1
        done
        systemctl --user status --no-pager "$unit" >&2 || true
        return 1
    }

    installed=1
    "$agenturm" service install --config "$config" --db "$managed_db" --interval-ms 500
    wait_for_collection
    first_pid=$(systemctl --user show "$unit" --property=MainPID --value)
    [[ $first_pid != 0 ]]
    "$agenturm" service status
    systemctl --user restart "$unit"
    systemctl --user is-active --quiet "$unit"
    second_pid=$(systemctl --user show "$unit" --property=MainPID --value)
    [[ $second_pid != 0 && $first_pid != "$second_pid" ]]
    wait_for_collection
    inspect_history "$managed_db"
    "$agenturm" service uninstall
    installed=0
    [[ $(systemctl --user show "$unit" --property=LoadState --value) == not-found ]]
    "${isolated[@]}" query --db "$managed_db" | expect_usage
    echo "Verified managed install, restart, inspector, and retained history after uninstall"
fi
