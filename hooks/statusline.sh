#!/bin/sh
# Claude Code statusLine: saves the account rate limits to ~/.cache/ccs/claude-limits.json
# for ccs/notch, and prints a short usage line.
input=$(cat)
dir="$HOME/.cache/ccs"
mkdir -p "$dir"
[ -n "$CCS_STATUSLINE_DEBUG" ] && printf '%s' "$input" > "$dir/statusline-debug.json"
tmp="$dir/claude-limits.$$.tmp"
printf '%s' "$input" | jq -c '.rate_limits // empty' > "$tmp" 2>/dev/null
# The modification time records freshness even when the percentages have not changed.
if [ -s "$tmp" ]; then mv "$tmp" "$dir/claude-limits.json"; else rm -f "$tmp"; fi
printf '%s' "$input" | jq -r '
  [ .model.display_name // empty,
    (.rate_limits.five_hour.used_percentage | select(. != null) | "5h \(round)%"),
    (.rate_limits.seven_day.used_percentage | select(. != null) | "wk \(round)%"),
    (.context_window.used_percentage | select(. != null) | "ctx \(round)%")
  ] | join(" · ")' 2>/dev/null
