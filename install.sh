#!/bin/sh
# Install from source: builds, puts `ccs` on your PATH and starts the overlay at login.
# (For the packaged app, download the DMG from GitHub Releases instead.)
# Arguments are passed on to notch, e.g. ./install.sh --beside-notch --quiet
set -e
cd "$(dirname "$0")"
dir=$(pwd)
./build.sh

mkdir -p "$HOME/.local/bin" "$HOME/Library/LaunchAgents"
ln -sf "$dir/build/ccs" "$HOME/.local/bin/ccs"

label=com.castruu.cc-sessions-notch
plist="$HOME/Library/LaunchAgents/$label.plist"
args=""
for a in "$@"; do args="$args
		<string>$a</string>"; done
cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>$label</string>
	<key>ProgramArguments</key>
	<array>
		<string>$dir/build/notch</string>$args
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
</dict>
</plist>
EOF
# stop the old one (if any), then start the new one
launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$plist"

cat <<EOF

Installed. The overlay is running and starts at login.
ccs is at ~/.local/bin/ccs (add ~/.local/bin to your PATH if it is not there).

Optional:

  One-line summaries for Claude Code (needs jq), in ~/.claude/settings.json:
    "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "$dir/hooks/summarize.sh" }] }] }

  The same for Codex, in ~/.codex/hooks.json:
    { "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "$dir/hooks/summarize.sh codex" }] }] } }

  Keep Claude limits fresh while you work, in ~/.claude/settings.json:
    "statusLine": { "type": "command", "command": "$dir/hooks/statusline.sh" }

Setup check:
EOF
build/ccs --doctor || true
