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
- `hooks/statusline.sh`: Claude Code status line that records your usage limits for the usage bar

Each session shows its state: working, idle, needs you (blocked on a permission prompt, or its last message asks you something) or stale (idle for over a day). You also get the git branch, a dot for uncommitted changes, and your Claude and Codex usage with the time until reset. A turn that ends plays Tink; a session that starts needing you plays Glass.

`ccs --json` prints `{"sessions": [...], "limits": {...}}`.

## Requirements

- macOS, Rust (`cargo`) and Xcode command line tools (`swiftc`) to build
- `jq` for the summary hook
- [Ghostty](https://ghostty.org) 1.4+ only for click-to-jump; without it everything else works and clicks do nothing

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

Without it, the last prompt is shown instead, and "needs you" falls back to checking whether the last message ends with a question mark.

## Enable the usage bar

Claude Code only exposes rate limits to a status line command, so add to `~/.claude/settings.json`:

```json
"statusLine": { "type": "command", "command": "/ABSOLUTE/PATH/cc-sessions-viewer/hooks/statusline.sh" }
```

It prints a short line (model, usage, context) and saves the limits to `~/.cache/ccs/claude-limits.json`. Codex limits need no setup; they are read from its newest session file.

## Running next to another notch app

`build/notch --beside-notch` puts the overlay in a small pill to the right of the notch instead of on it, one window level lower, so it does not fight apps like boringNotch for the hover area.

## Ghostty quick terminal

```
keybind = global:cmd+backquote=toggle_quick_terminal
```

then run `build/ccs --watch` in it.

## Screenshots

Regenerate them from sample data with `build/notch --snapshot docs`.
