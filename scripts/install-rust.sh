#!/usr/bin/env bash
# Explicitly opt-in installer for qualified Rust release bundles.
set -eo pipefail
umask 077

usage() {
    cat <<'HELP'
Install the standalone Rust bundle alongside the default Agenturm command.

  install-rust.sh install --version 0.3.6
  install-rust.sh update --version VERSION [--sha256 HEX]
  install-rust.sh install --version VERSION --archive FILE --checksum FILE
  install-rust.sh uninstall

Options:
  --version VERSION  Required for install/update; exact X.Y.Z, no latest alias.
  --sha256 HEX       Independently pin the expected archive SHA-256.
  --archive FILE     Use a local archive instead of downloading.
  --checksum FILE    Local SHA-256 sidecar; required with --archive.
  --prefix DIR       Managed release directory (absolute path).
  --bin-dir DIR      Launcher directory (absolute path).
  --dry-run          Show the plan without downloading or changing files.
  --quiet           Suppress progress and next-step messages.
  --help            Show this help.

Default launcher: ~/.local/bin/agenturm-rust
Default releases: $XDG_DATA_HOME/agenturm-rust/dist
                  (or ~/.local/share/agenturm-rust/dist)

No sudo, shell profile edits, service changes, or database migration.
Uninstall removes managed program files and retains history/configuration.
Stop managed collection with "agenturm-rust service uninstall" first.
HELP
}

fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
invalid() { printf 'error: %s\nRun with --help for usage.\n' "$*" >&2; exit 2; }
say() { if [[ "$quiet" != yes ]]; then printf '%s\n' "$*" >&2; fi; }
need_value() { [[ $# -ge 2 && -n "$2" && "$2" != --* ]] || invalid "$1 requires a value"; }
require() { command -v "$1" >/dev/null 2>&1 || fail "$1 is required; install it with your OS package manager"; }

action=install
version=
expected_pin=
archive_input=
checksum_input=
data_base=$XDG_DATA_HOME
[[ -n "$data_base" ]] || data_base="$HOME/.local/share"
prefix="$data_base/agenturm-rust/dist"
bin_dir="$HOME/.local/bin"
dry_run=no
quiet=no
if [[ $# -gt 0 && "$1" != --* ]]; then action=$1; shift; fi
case "$action" in install|update|uninstall) ;; *) invalid "unknown action: $action";; esac
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version) need_value "$@"; version=$2; shift 2;;
        --sha256) need_value "$@"; expected_pin=$2; shift 2;;
        --archive) need_value "$@"; archive_input=$2; shift 2;;
        --checksum) need_value "$@"; checksum_input=$2; shift 2;;
        --prefix) need_value "$@"; prefix=$2; shift 2;;
        --bin-dir) need_value "$@"; bin_dir=$2; shift 2;;
        --dry-run) dry_run=yes; shift;;
        --quiet) quiet=yes; shift;;
        --help) usage; exit 0;;
        *) invalid "unknown option: $1";;
    esac
done
for path in "$prefix" "$bin_dir"; do
    [[ "$path" == /* && "$path" != *[[:cntrl:]]* ]] || invalid "installation directories must be absolute paths without control characters"
done
require realpath
prefix=$(realpath -m -- "$prefix")
bin_dir=$(realpath -m -- "$bin_dir")
[[ "$prefix" != / && "$prefix" != "$HOME" && "$prefix" != "$bin_dir" ]] || invalid "choose a dedicated release directory"
launcher="$bin_dir/agenturm-rust"
marker="$prefix/.agenturm-rust-managed"
marker_text=agenturm-rust-installer-v1
receipt_text=agenturm-rust-install-v1
release_pattern='^agenturm-([0-9]+\.[0-9]+\.[0-9]+)-linux-(x86_64|aarch64)$'
releases="$prefix/releases"
owner=$(id -u)
resources='LICENSE README.md README.html agenturm-plugins.json'
binaries='agenturm agenturm-plugin-host agenturm-connector agenturm-storage-service agenturm-codex-harness agenturm-pi-harness agenturm-claude-harness agenturm-kiro-harness agenturm-opencode-harness'

present() { [[ -e "$1" || -L "$1" ]]; }
owner_mode() {
    local mode
    [[ "$(stat -c %u -- "$1")" == "$owner" ]] || return 1
    mode=$(stat -c %a -- "$1") || return 1
    (( (8#$mode & 0022) == 0 ))
}
owned_directory() { [[ -d "$1" && ! -L "$1" ]] && owner_mode "$1"; }
writable_directory() { owned_directory "$1" && [[ -w "$1" && -x "$1" ]]; }
owned_file() { [[ -f "$1" && ! -L "$1" ]] && owner_mode "$1"; }

managed_root() {
    owned_directory "$prefix" && owned_file "$marker" &&
        [[ "$(cat -- "$marker")" == "$marker_text" ]]
}
valid_release_receipt() {
    local directory=$1 name header version_line digest_line trailing release_version
    name=${directory##*/}
    [[ "$name" =~ $release_pattern ]] || return 1
    release_version=${BASH_REMATCH[1]}
    owned_file "$directory/.install-receipt" || return 1
    {
        IFS= read -r header && IFS= read -r version_line && IFS= read -r digest_line &&
            ! IFS= read -r trailing
    } < "$directory/.install-receipt" || return 1
    [[ "$header" == "$receipt_text" && "$version_line" == "version $release_version" &&
        "$digest_line" =~ ^archive_sha256\ [0-9a-f]{64}$ && -z "$trailing" ]]
}
check_launcher() {
    if present "$launcher"; then
        [[ -L "$launcher" ]] || fail "$launcher already exists and is not managed by this installer"
        local target relative name directory
        target=$(readlink -- "$launcher")
        relative=${target#"$releases"/}
        name=${relative%/bin/agenturm}
        [[ "$relative" != "$target" && "$relative" == "$name/bin/agenturm" && "$name" =~ $release_pattern ]] ||
            fail "$launcher points outside this installation; keep it or choose another --bin-dir"
        directory="$releases/$name"
        managed_root || fail "$launcher has no matching installer ownership marker"
        owned_directory "$releases" || fail "managed releases directory must be owned by the current user and writable only by its owner"
        owned_directory "$directory" && owned_directory "$directory/bin" && valid_release_receipt "$directory" ||
            fail "$launcher does not select a managed release with a matching installer receipt"
        if present "$target"; then
            owned_file "$target" || fail "$launcher has an unexpected target"
        fi
    fi
}
check_collector() {
    local config_base=${XDG_CONFIG_HOME:-"$HOME/.config"}
    [[ "$config_base" == /* ]] || invalid "XDG_CONFIG_HOME must be an absolute path"
    if present "$config_base/systemd/user/agenturm-collector.service"; then
        fail "a managed collector is configured; run agenturm-rust service uninstall first"
    fi
}

if [[ "$action" == uninstall ]]; then
    [[ -z "$version$expected_pin$archive_input$checksum_input" ]] || invalid "uninstall does not accept release options"
    [[ -e "$prefix" ]] || { say "No managed Rust installation at $prefix"; exit 0; }
    managed_root || fail "refusing to remove an unrecognized installation at $prefix"
    check_launcher
    check_collector
    if [[ "$dry_run" == yes ]]; then say "Would remove the managed launcher and program files in $prefix; retain history and configuration"; exit 0; fi
else
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || invalid "--version requires an exact X.Y.Z Rust release"
    [[ -z "$expected_pin" || "$expected_pin" =~ ^[0-9a-f]{64}$ ]] || invalid "--sha256 requires 64 lowercase hexadecimal characters"
    [[ -z "$archive_input" && -z "$checksum_input" || -n "$archive_input" && -n "$checksum_input" ]] ||
        invalid "--archive and --checksum must be supplied together"
    [[ "$(uname -s)" == Linux ]] || fail "Rust bundles are qualified for Linux; this installer does not support this OS"
    architecture=$(uname -m)
    case "$architecture" in x86_64|aarch64) ;; *) fail "no qualified Rust bundle for architecture $architecture";; esac
    require getconf
    libc=$(getconf GNU_LIBC_VERSION 2>/dev/null) || fail "a glibc Linux runtime is required; musl is not qualified"
    [[ "$libc" =~ ^glibc\ ([0-9]+)\.([0-9]+)$ ]] || fail "cannot identify the glibc runtime"
    (( BASH_REMATCH[1] > 2 || (BASH_REMATCH[1] == 2 && BASH_REMATCH[2] >= 39) )) ||
        fail "the qualified Ubuntu 24.04 bundles require glibc 2.39 or newer"
    bundle="agenturm-$version-linux-$architecture"
    archive_name="$bundle.tar.gz"
    release_url="https://github.com/abhishekash/agenturm/releases/download/rust-v$version"
    check_launcher
    if [[ "$dry_run" == yes ]]; then
        say "Would verify $archive_name and install the complete bundle in $prefix/releases/$bundle"
        say "Would atomically select $launcher; leave agenturm, services and history unchanged"
        printf '%s\n' "$launcher"
        exit 0
    fi
fi

work=
stage=
prefix_lock=
launcher_lock=
launcher_stage=
cleanup() {
    if [[ -n "$launcher_stage" ]]; then
        rm -f -- "$launcher_stage/launcher" || true
        rmdir -- "$launcher_stage" || true
    fi
    [[ -z "$stage" || ! -d "$stage" ]] || rm -r -- "$stage" || true
    [[ -z "$work" || ! -d "$work" ]] || rm -r -- "$work" || true
    [[ -z "$launcher_lock" ]] || rmdir -- "$launcher_lock" 2>/dev/null || true
    [[ -z "$prefix_lock" ]] || rmdir -- "$prefix_lock" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ "$action" != uninstall ]]; then
    require tar
    require sha256sum
    require cmp
    work=$(mktemp -d)
    if [[ -n "$archive_input" ]]; then
        [[ -f "$archive_input" && -f "$checksum_input" ]] || fail "local archive or checksum file does not exist"
        [[ "$(stat -Lc %s -- "$archive_input")" -le 104857600 && "$(stat -Lc %s -- "$checksum_input")" -le 256 ]] ||
            fail "local archive or checksum exceeds its size limit"
        cp -- "$archive_input" "$work/$archive_name"
        cp -- "$checksum_input" "$work/checksum"
    else
        require curl
        say "Fetching Rust $version for Linux $architecture"
        curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error --location \
            --connect-timeout 15 --max-time 180 --retry 2 --max-filesize 104857600 \
            --output "$work/$archive_name" "$release_url/$archive_name"
        curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error --location \
            --connect-timeout 15 --max-time 60 --retry 2 --max-filesize 256 \
            --output "$work/checksum" "$release_url/$archive_name.sha256"
    fi
    [[ "$(wc -c < "$work/checksum")" -le 256 && "$(wc -l < "$work/checksum")" == 1 ]] ||
        fail "checksum sidecar must contain exactly one bounded line"
    read -r digest checksum_name extra < "$work/checksum"
    [[ "$digest" =~ ^[0-9a-f]{64}$ && "$checksum_name" == "$archive_name" && -z "$extra" ]] ||
        fail "checksum sidecar does not name the requested archive"
    [[ -z "$expected_pin" || "$digest" == "$expected_pin" ]] || fail "release checksum differs from --sha256"
    actual=$(sha256sum -- "$work/$archive_name")
    [[ "$actual" == "$digest  "* ]] || fail "archive SHA-256 mismatch; nothing was installed"
    [[ "$(wc -c < "$work/$archive_name")" -le 104857600 ]] || fail "archive exceeds the 100 MiB limit"
    # A closed inventory also rejects traversal, duplicate members and extra files.
    {
        printf '%s/\n%s/bin/\n' "$bundle" "$bundle"
        for name in $resources; do printf '%s/%s\n' "$bundle" "$name"; done
        for name in $binaries; do printf '%s/bin/%s\n' "$bundle" "$name"; done
    } | LC_ALL=C sort > "$work/expected"
    tar --list --gzip --file "$work/$archive_name" --quoting-style=escape | LC_ALL=C sort > "$work/members"
    cmp -s -- "$work/expected" "$work/members" || fail "archive does not contain the expected closed bundle inventory"
    tar --list --verbose --gzip --file "$work/$archive_name" --numeric-owner --quoting-style=escape > "$work/types"
    awk 'substr($0,1,1)!="-" && substr($0,1,1)!="d" {bad=1} {bytes+=$3} END {exit (bad || bytes>536870912)}' "$work/types" ||
        fail "archive contains links, special files, or more than 512 MiB of unpacked data"
fi

if [[ ! -e "$prefix" ]]; then mkdir -p -- "$prefix"; fi
writable_directory "$prefix" || fail "installation directory must be owned by the current user and writable only by its owner"
if ! present "$marker"; then
    [[ -z "$(find "$prefix" -mindepth 1 -maxdepth 1 -print -quit)" ]] || fail "installation directory is nonempty and unmanaged"
    printf '%s\n' "$marker_text" > "$marker"
fi
managed_root || fail "installation ownership marker is invalid"
mkdir -- "$prefix/.install-lock" 2>/dev/null || fail "another installer is active; if it was interrupted, remove $prefix/.install-lock after checking"
prefix_lock="$prefix/.install-lock"

if [[ "$action" != uninstall ]]; then
    mkdir -p -- "$bin_dir"
fi
if present "$bin_dir"; then
    writable_directory "$bin_dir" || fail "launcher directory must be owned by the current user and writable only by its owner"
    mkdir -- "$bin_dir/.agenturm-rust.install-lock" 2>/dev/null ||
        fail "another installer is using the launcher directory; if interrupted, remove $bin_dir/.agenturm-rust.install-lock after checking"
    launcher_lock="$bin_dir/.agenturm-rust.install-lock"
    check_launcher
fi
if present "$releases"; then
    writable_directory "$releases" || fail "managed releases directory must be owned by the current user and writable only by its owner"
fi

if [[ "$action" == uninstall ]]; then
    check_collector
    # Build the removal list only after every managed release passes preflight.
    removal=()
    for directory in "$releases"/agenturm-*; do
        [[ -d "$directory" && ! -L "$directory" ]] || continue
        present "$directory/.install-receipt" || continue
        writable_directory "$directory" && writable_directory "$directory/bin" ||
            fail "refusing to remove an altered managed release at $directory"
        owned_file "$directory/.install-receipt" ||
            fail "refusing to remove an altered managed release at $directory"
        valid_release_receipt "$directory" ||
            fail "refusing to remove a release with an invalid receipt at $directory"
        for name in $resources; do
            ! present "$directory/$name" || owned_file "$directory/$name" ||
                fail "refusing to remove an altered managed file at $directory/$name"
        done
        for name in $binaries; do
            ! present "$directory/bin/$name" || owned_file "$directory/bin/$name" ||
                fail "refusing to remove an altered managed file at $directory/bin/$name"
        done
        removal+=("$directory")
    done
    if [[ -n "$launcher_lock" ]]; then
        check_launcher
        [[ ! -L "$launcher" ]] || rm -- "$launcher"
    fi
    for directory in "${removal[@]}"; do
        for name in $binaries; do rm -f -- "$directory/bin/$name"; done
        for name in $resources .install-receipt; do rm -f -- "$directory/$name"; done
        rmdir -- "$directory/bin" "$directory" 2>/dev/null || say "Retained extra files in $directory"
    done
    rmdir -- "$releases" 2>/dev/null || true
    say "Removed the managed Rust launcher and program files. History and configuration were retained."
    exit 0
fi

mkdir -p -- "$releases"
writable_directory "$releases" || fail "managed releases directory must be owned by the current user and writable only by its owner"
stage=$(mktemp -d "$prefix/.install.XXXXXXXX")
tar --extract --gzip --file "$work/$archive_name" --directory "$stage" \
    --no-same-owner --no-same-permissions --keep-old-files --delay-directory-restore
chmod 755 -- "$stage/$bundle" "$stage/$bundle/bin"
for name in $binaries; do chmod 755 -- "$stage/$bundle/bin/$name"; done
[[ "$("$stage/$bundle/bin/agenturm" --version)" == "agenturm $version" ]] ||
    fail "bundle cannot run on this host or reports the wrong version"
printf '%s\nversion %s\narchive_sha256 %s\n' "$receipt_text" "$version" "$digest" > "$stage/$bundle/.install-receipt"
destination="$releases/$bundle"
if present "$destination"; then
    owned_directory "$destination" && owned_directory "$destination/bin" && valid_release_receipt "$destination" ||
        fail "release directory already exists without a valid installer receipt or has unsafe permissions"
    cmp -s -- "$destination/.install-receipt" "$stage/$bundle/.install-receipt" ||
        fail "installed version has different archive bytes; refusing to replace it"
    for name in $binaries; do
        owned_file "$destination/bin/$name" && [[ -x "$destination/bin/$name" ]] &&
            cmp -s -- "$destination/bin/$name" "$stage/$bundle/bin/$name" || fail "installed binary was modified or is not executable: $name"
    done
    for name in $resources; do
        owned_file "$destination/$name" &&
            cmp -s -- "$destination/$name" "$stage/$bundle/$name" || fail "installed bundle file was modified: $name"
    done
    [[ "$("$destination/bin/agenturm" --version)" == "agenturm $version" ]] ||
        fail "installed release cannot run on this host or reports the wrong version"
else
    mv -T -- "$stage/$bundle" "$destination"
fi
check_launcher
launcher_stage=$(mktemp -d "$bin_dir/.agenturm-rust.XXXXXXXX")
ln -s -- "$destination/bin/agenturm" "$launcher_stage/launcher"
mv -Tf -- "$launcher_stage/launcher" "$launcher"
say "Installed Rust $version. Run: $launcher"
say "Existing services keep their selected version. Back up history before upgrading a managed collector."
case ":$PATH:" in *":$bin_dir:"*) ;; *) say "Add $bin_dir to PATH to run agenturm-rust by name.";; esac
printf '%s\n' "$launcher"
