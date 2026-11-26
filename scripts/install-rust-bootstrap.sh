#!/usr/bin/env bash
# Small, immutable-commit bootstrap for the explicitly opt-in Rust bundle.
set -euo pipefail
umask 077

fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
require() { command -v "$1" >/dev/null 2>&1 || fail "$1 is required"; }

version=0.3.8
installer_tag="rust-v$version"
installer_sha256=14e852d4b435b00164193bdd30d23e9696b7bfcc6f4181f4bc31735e5a4b32f8
case "$(uname -s):$(uname -m)" in
    Linux:x86_64|Linux:amd64)
        archive_sha256=528d4c7057d280258788d44e684505b7c552bc5f4dc8f730c9a24d568fc0d657
        ;;
    Linux:aarch64|Linux:arm64)
        archive_sha256=95e37c0254f9bcd1650b1a7d9a630a40c180f61dc80f43035e934c732dd556cc
        ;;
    Darwin:arm64)
        fail "Apple Silicon macOS support is not published yet; the native macOS artifact and installer are still being qualified"
        ;;
    Darwin:x86_64)
        fail "Intel Macs are outside the supported platform scope"
        ;;
    Linux:*)
        fail "no qualified Rust bundle for Linux architecture $(uname -m)"
        ;;
    *)
        fail "no qualified Rust bundle for $(uname -s) $(uname -m)"
        ;;
esac

require curl
require sha256sum
require mktemp

work=$(mktemp -d "${TMPDIR:-/tmp}/agenturm-rust-bootstrap.XXXXXXXX")
cleanup() { rm -rf -- "$work"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

installer_url="https://raw.githubusercontent.com/abhishekash/agenturm/$installer_tag/scripts/install-rust.sh"
curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error --location \
    --connect-timeout 15 --max-time 60 --retry 2 --max-filesize 1048576 \
    --output "$work/install-rust.sh" "$installer_url"
actual_installer_sha256=$(sha256sum -- "$work/install-rust.sh")
[[ "$actual_installer_sha256" == "$installer_sha256  "* ]] ||
    fail "downloaded installer does not match the pinned Rust $version source"

printf 'Installing Agenturm Rust %s for Linux %s\n' "$version" "$(uname -m)" >&2
bash "$work/install-rust.sh" install --version "$version" --sha256 "$archive_sha256"
