#!/bin/sh
# Claude Code statusLine: saves the account rate limits to ~/.cache/argus/claude-limits.json
# (claude-limits-<folder>.json for another CLAUDE_CONFIG_DIR, which is another account) for argus/notch,
# and prints a short usage line.
input=$(cat)
dir="$HOME/.cache/argus"
umask 077
[ ! -L "$dir" ] && mkdir -p "$dir" && chmod 700 "$dir" || exit 0
[ -n "$ARGUS_STATUSLINE_DEBUG" ] && printf '%s' "$input" > "$dir/statusline-debug.json"
case "${CLAUDE_CONFIG_DIR:-$HOME/.claude}" in
    "$HOME/.claude") file=claude-limits.json ;;
    *) file="claude-limits-$(basename "$CLAUDE_CONFIG_DIR").json" ;;
esac
tmp="$dir/claude-limits.$$.tmp"
printf '%s' "$input" | jq -c '.rate_limits // empty' > "$tmp" 2>/dev/null
# The modification time records freshness even when the percentages have not changed.
if [ -s "$tmp" ]; then mv "$tmp" "$dir/$file"; else rm -f "$tmp"; fi
printf '%s' "$input" | jq -r '
  [ .model.display_name // empty,
    (.rate_limits.five_hour.used_percentage | select(. != null) | "5h \(round)%"),
    (.rate_limits.seven_day.used_percentage | select(. != null) | "wk \(round)%"),
    (.context_window.used_percentage | select(. != null) | "ctx \(round)%")
  ] | join(" · ") | gsub("[\u0000-\u001f\u007f-\u009f]"; "")' 2>/dev/null
