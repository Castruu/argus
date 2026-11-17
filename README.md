# cc-sessions-viewer

See your active Claude Code and Codex sessions at a glance.

Hover the MacBook notch to see every session, grouped by repo. Click one to bring it forward in the app it runs in.

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
- `ccs --limits`: Claude 5h/weekly and Codex usage, read from what each tool caches locally (no tokens or network). In the notch it turns orange at 80% and red at 95%.
- `notch`: macOS overlay; hover the notch to see sessions
- `hooks/summarize.sh`: Claude Code Stop hook that writes a one-line AI summary per session (Haiku, at most once per 3 min per session)
- `hooks/statusline.sh`: optional Claude Code status line; a second source for the Claude limits when `~/.claude.json` has none

Each session shows its state: working, idle, needs you (blocked on a permission prompt, or its last message asks you something) or stale (idle for over a day). You also get the git branch, a dot for uncommitted changes, and the branch's pull request with its CI result (click it to open the PR). A turn that ends plays Tink; a session that starts needing you plays Glass.

Clicking a session opens it where it runs: the exact tab in Ghostty and Terminal, the right window in VS Code, Cursor, Windsurf and VSCodium (an editor's terminal tab cannot be picked from outside), and the app itself for anything else.

The overlay does not poll. It refreshes when Claude Code rewrites a session file or a hook writes a summary, so a finished turn shows up at once and an idle machine does no work. It stops while the screen is locked or asleep. With more repos than fit on screen, the panel scrolls sideways.

Pull request status needs the [GitHub CLI](https://cli.github.com) (`gh`), logged in. It is looked up in the background at most once every 5 minutes per folder, and skipped for `main`, `master` and stale sessions.

## Requirements

- macOS, Rust (`cargo`) and Xcode command line tools (`swiftc`) to build
- `jq` for the summary hook
- [Ghostty](https://ghostty.org) 1.4+ only for jumping to the exact tab of a Ghostty, tmux or Codex session; everything else works without it

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

## Limits when `~/.claude.json` has none

`ccs --limits` reads Claude's numbers from `cachedUsageUtilization` in `~/.claude.json`. Some installs never fill it. Claude Code also passes the limits to a status line command, so as a fallback add to `~/.claude/settings.json`:

```json
"statusLine": { "type": "command", "command": "/ABSOLUTE/PATH/cc-sessions-viewer/hooks/statusline.sh" }
```

It prints a short line (model, usage, context) and saves the limits to `~/.cache/ccs/claude-limits.json`, which `ccs --limits` uses when the cache is empty.

## Running next to another notch app

`build/notch --beside-notch` puts the overlay in a small pill to the right of the notch instead of on it, one window level lower, so it does not fight apps like boringNotch for the hover area.

## Ghostty quick terminal

```
keybind = global:cmd+backquote=toggle_quick_terminal
```

then run `build/ccs --watch` in it.

## Screenshots

Regenerate them from sample data with `build/notch --snapshot docs`.
