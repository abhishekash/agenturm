#!/usr/bin/env bash
# Install the complete prebuilt Agenturm bundle. No Python or Rust required.
set -euo pipefail
umask 077

fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
version=0.3.16
case "$(uname -s):$(uname -m)" in
  Darwin:arm64)
    target=macos-aarch64
    digest=a4f7ea0b73654057f5978775c2c04a2158ba8f739c3d053abf810bc0e9718091
    [[ $(sw_vers -productVersion | cut -d. -f1) -ge 13 ]] || fail 'macOS 13 or newer is required'
    ;;
  Linux:x86_64|Linux:amd64)
    target=linux-x86_64
    digest=7892d72645e817ac00b64fcb086d0037093046c22aee844d61234ce3da2961f6
    ;;
  Linux:aarch64|Linux:arm64)
    target=linux-aarch64
    digest=04746b9150960b2921acc54158d2d192ec640782923fe4e735e25e9239febf68
    ;;
  Darwin:*) fail 'Apple Silicon is required; Intel Macs and Rosetta are unsupported';;
  *) fail "unsupported platform: $(uname -s) $(uname -m)";;
esac
for tool in curl tar mktemp diff find; do command -v "$tool" >/dev/null || fail "$tool is required"; done
if command -v sha256sum >/dev/null; then
  checksum() { sha256sum "$1" | awk '{print $1}'; }
else
  command -v shasum >/dev/null || fail 'shasum or sha256sum is required'
  checksum() { shasum -a 256 "$1" | awk '{print $1}'; }
fi

root=${AGENTURM_INSTALL_DIR:-"${XDG_DATA_HOME:-$HOME/.local/share}/agenturm"}
bin_dir=${AGENTURM_BIN_DIR:-"$HOME/.local/bin"}
[[ $root == /* && $bin_dir == /* && $root != / && $bin_dir != / ]] || fail 'installation directories must be absolute paths below /'
[[ ! -L $root && ! -L $bin_dir ]] || fail 'installation directories must not be symbolic links'
mkdir -p "$root/releases" "$bin_dir"
[[ -O $root && -O $root/releases && -O $bin_dir && -w $bin_dir ]] || fail 'installation directories must be owned by you and writable'
[[ ! -L $root/releases ]] || fail 'release directory must not be a symbolic link'
work=$(mktemp -d "$root/.install.XXXXXXXX")
cleanup() { rm -r -- "$work"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
name="agenturm-$version-$target"
archive="$name.tar.gz"
url="https://github.com/abhishekash/agenturm/releases/download/rust-v$version/$archive"
printf 'Installing Agenturm %s (%s)\n' "$version" "$target"
curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL --retry 3 \
  --connect-timeout 15 --max-time 300 "$url" -o "$work/$archive"
[[ $(checksum "$work/$archive") == "$digest" ]] || fail 'downloaded archive failed SHA-256 verification'
tar -xzf "$work/$archive" -C "$work"
[[ $("$work/$name/bin/agenturm" --version) == "agenturm $version" ]] || fail 'this Agenturm binary cannot run on your system'
release="$root/releases/$name-$digest"
if [[ -e $release || -L $release ]]; then
  [[ -d $release && ! -L $release && -O $release ]] || fail 'existing release directory is not owned by you'
  [[ -d $release/bin && ! -L $release/bin && -O $release/bin ]] || fail 'existing release bin directory is not owned by you'
  [[ -z $(find "$release" -type l -print) ]] || fail 'existing release contains a symbolic link; remove it and retry'
  diff -qr "$work/$name" "$release" >/dev/null 2>&1 || fail 'existing release differs from the verified bundle; remove it and retry'
  for file in "$work/$name/bin/"*; do
    installed="$release/bin/${file##*/}"
    [[ ! -L $installed && -f $installed && -x $installed ]] || fail 'existing release executable is not a regular executable file'
  done
else
  mv "$work/$name" "$release"
fi
launcher="$bin_dir/agenturm"
# Keep a displaced launcher recoverable; all new launches use the Rust binary.
if [[ -e $launcher || -L $launcher ]]; then
  if [[ ! -L $launcher || $(readlink "$launcher") != "$release/bin/agenturm" ]]; then
    if [[ ! -L $launcher ]]; then
      [[ ! -d $launcher && -O $launcher ]] || fail 'existing agenturm command is not owned by you'
    fi
    backup="$bin_dir/agenturm.before-rust.$(date +%s).$$"
    mv "$launcher" "$backup"
    printf 'Saved previous launcher: %s\n' "$backup"
  fi
fi
ln -s "$release/bin/agenturm" "$work/launcher"
# rename into place without following an existing link to an executable.
if [[ $target == linux-* ]]; then
  mv -Tf "$work/launcher" "$launcher"
else
  mv -fh "$work/launcher" "$launcher"
fi
printf 'Installed Agenturm %s. Run: agenturm\n' "$version"
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) printf 'Add to your shell profile: export PATH="%s:$PATH"\n' "$bin_dir";;
esac
