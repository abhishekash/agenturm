#!/usr/bin/env bash
set -euo pipefail
umask 077

fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
usage() {
  cat <<'HELP'
Usage:
  install-rust-macos.sh install --version X.Y.Z
  install-rust-macos.sh update  --version X.Y.Z
  install-rust-macos.sh install --version X.Y.Z --archive FILE --checksum FILE
  install-rust-macos.sh uninstall

Installs an Apple Silicon release in ~/Library/Application Support/Agenturm/Rust
and selects it through ~/.local/bin/agenturm-rust. macOS 13 or newer required.
Install/update download the exact archive and SHA-256 sidecar from GitHub Releases
unless local --archive and --checksum files are supplied.
HELP
}
[[ $(uname -s) == Darwin && $(uname -m) == arm64 ]] || fail "Apple Silicon macOS is required; Intel and Rosetta are unsupported"
[[ $(sw_vers -productVersion | cut -d. -f1) -ge 13 ]] || fail "macOS 13 Ventura or newer is required"
action=${1:-}; shift || true
case "$action" in install|update|uninstall) ;; -h|--help|help) usage; exit 0;; *) usage >&2; exit 2;; esac
home=${HOME:?HOME is required}
root="$home/Library/Application Support/Agenturm/Rust"
bin_dir="$home/.local/bin"
launcher="$bin_dir/agenturm-rust"
marker="$root/.managed-by-agenturm-rust-macos"
app_support="$home/Library/Application Support/Agenturm"
mkdir -p "$app_support" "$bin_dir"
[[ -d $app_support && ! -L $app_support && -d $bin_dir && ! -L $bin_dir ]] || fail "installation parents must be real directories"
[[ $(stat -f %u "$app_support") == $(id -u) && $(stat -f %u "$bin_dir") == $(id -u) ]] || fail "installation parents must be owned by the current user"

if [[ $action == uninstall ]]; then
  [[ ! -e $root ]] && exit 0
  [[ -d $root && ! -L $root && -f $marker && ! -L $marker ]] || fail "refusing to remove unrecognized path: $root"
  [[ $(cat "$marker") == agenturm-rust-macos-v1 ]] || fail "invalid installer ownership marker"
  [[ ! -L $root/releases ]] || fail "managed releases path must not be a symlink"
  if [[ -L $launcher ]]; then
    target=$(readlink "$launcher")
    [[ $target == "$root/releases/"*/bin/agenturm ]] || fail "launcher points outside the managed installation"
    rm -f "$launcher"
  elif [[ -e $launcher ]]; then fail "launcher exists and is not a managed symlink"; fi
  rm -rf "$root/releases"
  rm -f "$marker"
  exit 0
fi

version= archive= checksum= download_dir= stage=
while (($#)); do
  case $1 in
    --version) (($# >= 2)) || fail "--version needs a value"; version=$2; shift 2;;
    --archive) (($# >= 2)) || fail "--archive needs a value"; archive=$2; shift 2;;
    --checksum) (($# >= 2)) || fail "--checksum needs a value"; checksum=$2; shift 2;;
    *) fail "unknown option: $1";;
  esac
done
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "--version requires an exact X.Y.Z release"
[[ -z $archive && -z $checksum || -n $archive && -n $checksum ]] || fail "--archive and --checksum must be supplied together"
archive_name="agenturm-$version-macos-aarch64.tar.gz"
if [[ -z $archive ]]; then
  command -v curl >/dev/null 2>&1 || fail "curl is required to download the release"
  download_dir=$(mktemp -d "${TMPDIR:-/tmp}/agenturm-macos-download.XXXXXXXX")
  release_url="https://github.com/abhishekash/agenturm/releases/download/rust-v$version"
  curl --fail --location --silent --show-error --retry 3 --proto '=https' --tlsv1.2 \
    "$release_url/$archive_name" -o "$download_dir/$archive_name" || fail "could not download $archive_name"
  curl --fail --location --silent --show-error --retry 3 --proto '=https' --tlsv1.2 \
    "$release_url/$archive_name.sha256" -o "$download_dir/$archive_name.sha256" || fail "could not download the SHA-256 sidecar"
  archive="$download_dir/$archive_name"
  checksum="$archive.sha256"
fi
[[ -f $archive && -f $checksum ]] || fail "archive and checksum must be regular files"
[[ $(basename "$archive") == "$archive_name" ]] || fail "archive name does not match the requested version/target"
cleanup() {
  [[ -z $stage ]] || rm -rf "$stage"
  [[ -z $download_dir ]] || rm -rf "$download_dir"
}
trap cleanup EXIT
expected=$(awk 'NR == 1 { print $1 }' "$checksum")
[[ $expected =~ ^[0-9a-f]{64}$ ]] || fail "invalid SHA-256 sidecar"
actual=$(shasum -a 256 "$archive" | awk '{print $1}')
[[ $actual == "$expected" ]] || fail "archive checksum mismatch"

if [[ -e $root && ! -f $marker ]]; then fail "refusing to install over an unrecognized directory: $root"; fi
if [[ -f $marker && $(cat "$marker") != agenturm-rust-macos-v1 ]]; then fail "invalid installer ownership marker"; fi
[[ ! -L $root && (! -e $root || -d $root) ]] || fail "managed install root must be a real directory"
if [[ -e $launcher && ! -L $launcher ]]; then fail "launcher exists and is not a symlink"; fi
if [[ -L $launcher ]]; then
  current=$(readlink "$launcher")
  [[ $current == "$root/releases/agenturm-"*-macos-aarch64/bin/agenturm ]] || fail "launcher points outside this managed installation"
fi
[[ ! -L $root/releases ]] || fail "managed releases path must not be a symlink"
name="agenturm-$version-macos-aarch64"
release="$root/releases/$name"
[[ ! -e $release ]] || fail "version is already installed: $version"
mkdir -p "$root"
chmod 700 "$root"
stage=$(mktemp -d "$root/.install.XXXXXXXX")
trap 'rm -rf "$stage"' EXIT
tar -xzf "$archive" -C "$stage" -- "$name"
bundle="$stage/$name"
[[ -f $bundle/release-metadata.json && -f $bundle/bin/agenturm && -f $bundle/bin/agenturm-plugin-host ]] || fail "archive is missing required bundle files"
[[ $("$bundle/bin/agenturm" --version) == "agenturm $version" ]] || fail "archive version does not match --version"
grep -q '"target": "aarch64-apple-darwin"' "$bundle/release-metadata.json" || fail "archive target metadata is not Apple Silicon macOS"
grep -q '"minimum_macos": "13.0"' "$bundle/release-metadata.json" || fail "unsupported minimum macOS metadata"
for executable in "$bundle"/bin/*; do
  [[ -f $executable && ! -L $executable ]] || fail "bundle contains a non-regular executable"
  [[ $(lipo -archs "$executable") == arm64 ]] || fail "bundle contains a non-Apple-Silicon executable"
  vtool -show-build "$executable" | grep -q 'minos 13.0' || fail "binary does not declare the macOS 13 minimum"
done
mkdir -p "$root/releases"
chmod 700 "$root/releases"
mv "$bundle" "$release"
printf 'agenturm-rust-macos-v1\n' > "$marker"
chmod 600 "$marker"
link_stage="$bin_dir/.agenturm-rust-link.$$"
ln -s "$release/bin/agenturm" "$link_stage"
mv -f "$link_stage" "$launcher"
printf 'Installed Agenturm Rust %s at %s\n' "$version" "$launcher"
printf 'Add %s to PATH if needed; open a new terminal, then run agenturm-rust.\n' "$bin_dir"
printf 'This direct-download build has no Apple Developer ID signature or notarization. If macOS blocks first launch, approve only Agenturm in Privacy & Security; keep Gatekeeper enabled.\n'
