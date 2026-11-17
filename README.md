# claude-sessions-viewer

See your active Claude Code and Codex sessions at a glance.

- `ccs`: list sessions (`--watch` to refresh, `--json` for tools)
- `notch`: macOS overlay; hover the notch to see sessions
- `hooks/summarize.sh`: Claude Code Stop hook that writes a one-line AI summary per session (Haiku, at most once per 3 min per session)

## Build

```sh
./build.sh          # -> build/ccs, build/notch
build/notch &       # start the overlay
```

## Enable summaries

Add to `~/.claude/settings.json`:

```json
"hooks": {
  "Stop": [{ "hooks": [{ "type": "command", "command": "/ABSOLUTE/PATH/claude-sessions-viewer/hooks/summarize.sh" }] }]
}
```

Without it, the last prompt is shown instead.

## Ghostty quick terminal

```
keybind = global:cmd+backquote=toggle_quick_terminal
```

then run `build/ccs --watch` in it.
