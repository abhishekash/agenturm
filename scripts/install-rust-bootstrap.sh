#!/usr/bin/env bash
# Small, immutable-commit bootstrap for the explicitly opt-in Rust bundle.
set -euo pipefail
umask 077

fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
require() { command -v "$1" >/dev/null 2>&1 || fail "$1 is required"; }

version=0.3.10
installer_tag="rust-v$version"
linux_installer_sha256=14e852d4b435b00164193bdd30d23e9696b7bfcc6f4181f4bc31735e5a4b32f8
macos_installer_sha256=f4e29bcd1bce821f8df0541cd894e3854a35f2d8370ab918bac202837aa552fb
case "$(uname -s):$(uname -m)" in
    Linux:x86_64|Linux:amd64)
        installer_name=install-rust.sh
        checksum_tool=sha256sum
        platform_label="Linux $(uname -m)"
        ;;
    Linux:aarch64|Linux:arm64)
        installer_name=install-rust.sh
        checksum_tool=sha256sum
        platform_label="Linux $(uname -m)"
        ;;
    Darwin:arm64)
        installer_name=install-rust-macos.sh
        checksum_tool=shasum
        platform_label="Apple Silicon macOS"
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
require "$checksum_tool"
require mktemp

work=$(mktemp -d "${TMPDIR:-/tmp}/agenturm-rust-bootstrap.XXXXXXXX")
cleanup() { rm -rf -- "$work"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

installer_url="https://raw.githubusercontent.com/abhishekash/agenturm/$installer_tag/scripts/$installer_name"
curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error --location \
    --connect-timeout 15 --max-time 60 --retry 2 --max-filesize 1048576 \
    --output "$work/$installer_name" "$installer_url"
if [[ $checksum_tool == sha256sum ]]; then
    actual_installer_sha256=$(sha256sum -- "$work/$installer_name" | awk '{print $1}')
else
    actual_installer_sha256=$(shasum -a 256 "$work/$installer_name" | awk '{print $1}')
fi
expected_installer_sha256=$linux_installer_sha256
[[ $installer_name == install-rust-macos.sh ]] && expected_installer_sha256=$macos_installer_sha256
[[ "$actual_installer_sha256" == "$expected_installer_sha256" ]] ||
    fail "downloaded installer does not match the pinned Rust $version source"

printf 'Installing Agenturm Rust %s for %s\n' "$version" "$platform_label" >&2
bash "$work/$installer_name" install --version "$version"
