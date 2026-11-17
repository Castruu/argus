#!/bin/sh
# Build a drag-to-Applications disk image. Use --preview for local testing.
set -eu
cd "$(dirname "$0")"
case "${1:-}" in
    --preview) identity=-; timestamp=--timestamp=none; suffix=-preview ;;
    "") identity=${SIGN_IDENTITY:?Set SIGN_IDENTITY to your Developer ID Application identity, or use --preview}
        timestamp=--timestamp; suffix= ;;
    *) echo "usage: $0 [--preview]" >&2; exit 2 ;;
esac
[ "$#" -le 1 ] || exit 2
if [ -z "$suffix" ]; then
    case "$identity" in "Developer ID Application: "*) ;; *) echo "Expected a Developer ID Application identity" >&2; exit 1 ;; esac
fi

./build.sh
version=$(cargo metadata --locked --offline --no-deps --format-version 1 | /usr/bin/plutil -extract packages.0.version raw -o - -)
arch=$(lipo -archs build/notch)
[ "$arch" = "$(lipo -archs build/ccs)" ]
work=$(mktemp -d "$PWD/build/package.XXXXXX")
trap 'rm -rf "$work"' EXIT
name='CC Sessions Viewer'
app="$work/image/$name.app"
contents="$app/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources/hooks"
cp build/notch build/ccs "$contents/MacOS/"
cp notch/*.applescript LICENSE notch/ProviderIcons-LICENSE "$contents/Resources/"
cp hooks/*.sh "$contents/Resources/hooks/"
ln -s /Applications "$work/image/Applications"
cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>com.github.castruu.cc-sessions-viewer</string>
    <key>CFBundleName</key><string>$name</string>
    <key>CFBundleExecutable</key><string>notch</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><true/>
    <key>NSAppleEventsUsageDescription</key><string>Show the terminal tab for the session you select.</string>
</dict>
</plist>
PLIST
plutil -lint "$contents/Info.plist"
codesign --force --sign "$identity" "$timestamp" --options runtime "$contents/MacOS/ccs"
codesign --force --sign "$identity" "$timestamp" --options runtime --entitlements packaging/automation.entitlements "$app"
codesign --verify --deep --strict "$app"

file="CC-Sessions-Viewer-$version-$arch$suffix.dmg"
image="$work/$file"
hdiutil create -quiet -volname "$name" -format UDZO -fs HFS+ -srcfolder "$work/image" "$image"
if [ -z "$suffix" ]; then
    codesign --sign "$identity" --timestamp --identifier com.github.castruu.cc-sessions-viewer.dmg "$image"
    xcrun notarytool submit "$image" --keychain-profile "${NOTARY_PROFILE:-cc-sessions-viewer}" --wait --output-format json > "$work/notary.json" || {
        cat "$work/notary.json" >&2
        exit 1
    }
    cat "$work/notary.json"
    [ "$(plutil -extract status raw -o - "$work/notary.json")" = Accepted ]
    xcrun stapler staple "$image"
    xcrun stapler validate "$image"
    spctl --assess --type open --context context:primary-signature "$image"
fi
hdiutil verify -quiet "$image"
mv -f "$image" "build/$file"
echo "Built build/$file"
