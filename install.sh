#!/bin/sh
# Installs the latest release into /Applications and opens it.
# The app is not notarized; curl does not quarantine its downloads, so Gatekeeper does not block the first launch.
set -eu
[ "$(uname -sm)" = "Darwin arm64" ] && [ "$(sw_vers -productVersion | cut -d. -f1)" -ge 14 ] || {
    echo "CC Sessions Viewer needs an Apple Silicon Mac on macOS 14 or later" >&2; exit 1; }
work=$(mktemp -d)
trap 'hdiutil detach "$work/volume" >/dev/null 2>&1; rm -rf "$work"' EXIT
curl -fsSL -o "$work/app.dmg" https://github.com/Castruu/cc-sessions-viewer/releases/latest/download/CC-Sessions-Viewer-arm64.dmg
mkdir "$work/volume"
hdiutil attach -readonly -nobrowse -mountpoint "$work/volume" "$work/app.dmg" >/dev/null
pkill -x notch 2>/dev/null || true # a running copy would keep the old version on screen
rm -rf "/Applications/CC Sessions Viewer.app"
ditto "$work/volume/CC Sessions Viewer.app" "/Applications/CC Sessions Viewer.app"
open "/Applications/CC Sessions Viewer.app"
echo "Installed CC Sessions Viewer. Hover the notch."
