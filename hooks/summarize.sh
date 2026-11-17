#!/bin/sh
# Claude Code Stop hook: writes a one-line summary to ~/.cache/ccs/<session_id>.txt in the background.
[ -n "$CCS_SUMMARIZING" ] && exit 0 # the headless claude below must not re-trigger us

input=$(cat)
id=$(printf '%s' "$input" | jq -r '.session_id // empty')
transcript=$(printf '%s' "$input" | jq -r '.transcript_path // empty')
[ -z "$id" ] || [ ! -f "$transcript" ] && exit 0

dir="$HOME/.cache/ccs"
out="$dir/$id.txt"
mkdir -p "$dir"
# throttle: at most one summary per session every 3 minutes
[ -f "$out" ] && [ $(( $(date +%s) - $(stat -f %m "$out") )) -lt 180 ] && exit 0
touch "$out"

convo=$(jq -r '
  if .type == "user" and (.message.content | type) == "string" then "USER: " + .message.content
  elif .type == "assistant" then (.message.content[]? | select(.type == "text") | "ASSISTANT: " + .text)
  else empty end' "$transcript" 2>/dev/null | tail -n 40 | cut -c1-500)

(
  printf '%s' "$convo" | CCS_SUMMARIZING=1 claude -p --model haiku --no-session-persistence \
    "In one line of at most 15 words, say what this coding session is working on right now and whether it is waiting on the user. No preamble." \
    > "$out.tmp" 2>/dev/null && [ -s "$out.tmp" ] && mv "$out.tmp" "$out"
  rm -f "$out.tmp"
) </dev/null >/dev/null 2>&1 &
exit 0
