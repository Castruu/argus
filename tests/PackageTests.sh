#!/bin/sh
# Smoke-test the actual disk image, including the installed layout on a path with spaces.
set -eu
image=${1:?Pass the path to a built DMG}
work=$(mktemp -d)
mount="$work/volume"
mkdir "$mount" "$work/Applications" "$work/snapshots" "$work/user"
hdiutil attach -readonly -nobrowse -mountpoint "$mount" "$image" >/dev/null
trap 'hdiutil detach "$mount" >/dev/null && rm -rf "$work"' EXIT
test "$(readlink "$mount/Applications")" = /Applications
app="$work/Applications/CC Sessions Viewer.app"
ditto "$mount/CC Sessions Viewer.app" "$app"
codesign --verify --deep --strict "$app"
resources="$app/Contents/Resources"
for script in focus focus-terminal; do test -s "$resources/$script.applescript"; done
for hook in summarize statusline; do test -x "$resources/hooks/$hook.sh"; done
test "$(env HOME="$work/user" CODEX_HOME="$work/user/.codex" "$app/Contents/MacOS/ccs" --limits --json)" = '[]'
"$app/Contents/MacOS/notch" --snapshot "$work/snapshots"
test -s "$work/snapshots/notch-open.png"
test -s "$work/snapshots/notch-finished.png"
echo "Packaged app and helper passed smoke tests"
