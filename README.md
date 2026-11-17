# cc-sessions-viewer

See your active Claude Code and Codex sessions at a glance.

Hover the MacBook notch to see every session, grouped by repo. Click one to jump to its Ghostty tab.

![Sessions grouped by repo](docs/notch-open.png)

When a session finishes its turn you get a soft chime and this:

![Turn finished toast](docs/notch-finished.png)

Or in the terminal:

```
$ ccs
api
  ● Add rate limiting to /login  claude · api · busy 8s
      Writing middleware tests; no input needed yet.
web-app
  ● Fix flaky checkout test  codex · web-app · busy 3s
      Re-running checkout suite after fixing a race.
```

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
  "Stop": [{ "hooks": [{ "type": "command", "command": "/ABSOLUTE/PATH/cc-sessions-viewer/hooks/summarize.sh" }] }]
}
```

Without it, the last prompt is shown instead.

## Ghostty quick terminal

```
keybind = global:cmd+backquote=toggle_quick_terminal
```

then run `build/ccs --watch` in it.

## Screenshots

Regenerate them from sample data with `build/notch --snapshot docs`.
