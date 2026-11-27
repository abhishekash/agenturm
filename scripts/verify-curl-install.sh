#!/usr/bin/env bash
set -euo pipefail
work=$(mktemp -d "${TMPDIR:-/tmp}/agenturm-curl-check.XXXXXXXX")
trap 'rm -r -- "$work"' EXIT
script_dir=$(cd "$(dirname "$0")" && pwd)
export AGENTURM_INSTALL_DIR="$work/program files"
export AGENTURM_BIN_DIR="$work/user commands"
mkdir -p "$AGENTURM_BIN_DIR"
# A normal shared user bin may have group write permission.
chmod 775 "$AGENTURM_BIN_DIR"
printf '#!/bin/sh\nprintf "previous launcher\\n"\n' > "$AGENTURM_BIN_DIR/agenturm"
chmod 755 "$AGENTURM_BIN_DIR/agenturm"
bash "$script_dir/install-rust-bootstrap.sh"
[[ $("$AGENTURM_BIN_DIR/agenturm" --version) == 'agenturm 0.3.10' ]]
backup=("$AGENTURM_BIN_DIR"/agenturm.before-rust.*)
[[ ${#backup[@]} -eq 1 && $("${backup[0]}") == 'previous launcher' ]]
selected=$(readlink "$AGENTURM_BIN_DIR/agenturm")
bundle=${selected%/bin/agenturm}
[[ -f $bundle/agenturm-plugins.json ]]
for binary in agenturm-plugin-host agenturm-codex-harness agenturm-pi-harness agenturm-claude-harness agenturm-kiro-harness agenturm-opencode-harness; do
  [[ -x $bundle/bin/$binary ]]
done
bash "$script_dir/install-rust-bootstrap.sh"
[[ $(readlink "$AGENTURM_BIN_DIR/agenturm") == "$selected" ]]
backup=("$AGENTURM_BIN_DIR"/agenturm.before-rust.*)
[[ ${#backup[@]} -eq 1 ]]
printf 'Verified Agenturm curl install, previous launcher backup, repeat install, and complete bundle\n'
