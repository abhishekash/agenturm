#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    echo "Usage on an Apple Silicon Mac: $0 <agenturm-version-macos-aarch64.tar.gz>" >&2
    exit 2
fi
archive=$1
[[ -f $archive && ! -L $archive ]] || { echo "Archive must be a regular file" >&2; exit 2; }
: "${APPLE_DEVELOPER_IDENTITY:?Set APPLE_DEVELOPER_IDENTITY to the Developer ID Application identity}"
: "${APPLE_NOTARY_PROFILE:?Set APPLE_NOTARY_PROFILE to a configured notarytool keychain profile}"
name=${archive##*/}
[[ $name =~ ^agenturm-[0-9]+\.[0-9]+\.[0-9]+-macos-aarch64\.tar\.gz$ ]] || { echo "Unexpected macOS archive name" >&2; exit 2; }
bundle_name=${name%.tar.gz}
work=$(mktemp -d "${TMPDIR:-/tmp}/agenturm-notarize.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
tar -xzf "$archive" -C "$work" -- "$bundle_name"
bundle="$work/$bundle_name"

for executable in "$bundle"/bin/*; do
    codesign --force --options runtime --timestamp --sign "$APPLE_DEVELOPER_IDENTITY" "$executable"
    codesign --verify --strict --verbose=2 "$executable"
done

python3 - "$bundle/agenturm-plugins.json" "$bundle/bin" <<'PY'
import hashlib, json, pathlib, sys
catalog, bindir = map(pathlib.Path, sys.argv[1:])
data = json.loads(catalog.read_text())
for plugin in data["plugins"]:
    path = pathlib.Path(plugin["adapter"])
    if path.is_absolute() or ".." in path.parts:
        raise SystemExit("refusing an unsafe adapter path in plugin catalog")
    plugin["sha256"] = hashlib.sha256((bindir.parent / path).read_bytes()).hexdigest()
catalog.write_text(json.dumps(data, indent=2) + "\n")
PY

ditto -c -k --keepParent "$bundle" "$work/notarize.zip"
xcrun notarytool submit "$work/notarize.zip" --keychain-profile "$APPLE_NOTARY_PROFILE" --wait
for executable in "$bundle"/bin/*; do
    spctl --assess --type execute "$executable"
done
(cd "$work" && tar -czf "$name" "$bundle_name" && shasum -a 256 "$name" > "$name.sha256")
mv -f "$work/$name" "$archive"
mv -f "$work/$name.sha256" "$archive.sha256"
printf 'Signed and notarized %s\n' "$archive"
