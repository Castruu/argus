#!/bin/sh
# Prints one short line for the notch: Claude 5h limit (% left, hours to reset), Codex weekly (% left, days to reset).
t=$(security find-generic-password -s "Claude Code-credentials" -w 2>/dev/null | jq -r .claudeAiOauth.accessToken)
claude=$(curl -sf -m 10 https://api.anthropic.com/api/oauth/usage -H "Authorization: Bearer $t" -H "anthropic-beta: oauth-2025-04-20" | jq -r '
  .five_hour | select(.) | "Claude \(100 - .utilization | floor)% ↻\((.resets_at | sub("\\.[0-9]+.*"; "Z") | fromdateiso8601) - now | . / 3600 | ceil)h"')
codex=$(curl -sf -m 10 https://chatgpt.com/backend-api/wham/usage -H "Authorization: Bearer $(jq -r .tokens.access_token ~/.codex/auth.json)" -H "ChatGPT-Account-Id: $(jq -r .tokens.account_id ~/.codex/auth.json)" | jq -r '
  .rate_limit.primary_window | "Codex \(100 - .used_percent)% ↻\(.reset_at - now | . / 86400 | ceil)d"')
echo "$claude   $codex"
