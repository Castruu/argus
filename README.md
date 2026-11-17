# cc-sessions-viewer

See your active Claude Code and Codex sessions at a glance.

Hover the MacBook notch to see every session, grouped by repo. Click one to bring it forward in the app it runs in.

Each session's Claude or Codex icon is tinted by its status, with the state written below it. Hover the icon to see the provider name.

Hover a session's summary or last prompt to read the full text in a tooltip.

![Sessions grouped by repo](docs/notch-open.png)

When a session finishes its turn you get a soft chime and this:

![Turn finished toast](docs/notch-finished.png)

Or in the terminal:

```
$ ccs
api
  ● Migrate users table  claude · api · needs you 4m
      Migration ready; waiting on you to approve running it.
  ● Add rate limiting to /login  claude · api · rate-limit* · #412 ✓ · busy 8s
      Writing middleware tests; no input needed yet.
web-app
  ● Fix flaky checkout test  codex · web-app · busy 3s
      Re-running checkout suite after fixing a race.
```

- `ccs`: list sessions (`--watch` to refresh, `--json` for tools)
- `ccs --limits`: Claude 5h/weekly and Codex usage from local caches. Add `--refresh` to fetch current limits with your existing Claude Code and Codex logins. With the default Cool palette, a bar turns violet at 80% and coral at 95%.
- `notch`: macOS overlay; hover the notch to see sessions. Usage limits sit below the sessions with inline bars, percentages and reset times; narrow panels wrap them into rows. Palettes: Cool (default), Soft, Focus, Vivid. Click the palette icon in the panel header for the next one, right-click to pick; remembered. `--palette <id>` for one run.
- `hooks/summarize.sh`: Stop hook that writes a one-line AI summary per session (Claude: Haiku; Codex: Luna; at most once per 3 min per session)
- `hooks/statusline.sh`: optional Claude Code status line; keeps Claude limits fresh between `/usage` cache updates

Each session shows its state: working, idle, needs you (Claude Code explicitly reports a pending permission, input, sandbox, worker or goal request) or stale (idle for over a day). Informational dialogs such as `/usage`, ordinary text questions and AI summaries do not trigger "needs you". You also get the git branch, a dot for uncommitted changes, and the branch's pull request with its CI result (click it to open the PR). A turn that ends plays Tink; a session that starts needing you plays Glass. Click the speaker icon beside the palette icon to mute or unmute both chimes; the choice is remembered across launches, and visual notifications still appear.

Clicking a session opens it where it runs: the exact tab in Ghostty and Terminal, the right folder or multi-folder workspace window in VS Code, Cursor, Windsurf and VSCodium (an editor's terminal tab cannot be picked from outside), and the app itself for anything else.

The overlay refreshes local data when session files change. Opening it also fetches current Claude and Codex limits in the background; cached values stay visible if a request fails. It stops while the screen is locked or asleep. Each repo shows up to three sessions before scrolling vertically within its own column; the repo heading stays visible, with a count when there are more sessions. With more repos than fit on screen, the panel scrolls sideways.

A Claude session's pull request comes from its own transcript, so the number and link need no network. Its state and CI result need the [GitHub CLI](https://cli.github.com) (`gh`), logged in: they are looked up in the background at most once every 5 minutes and skipped for stale sessions. Codex sessions, and Claude sessions with no recorded PR, show the PR of the branch that is checked out, except on `main` and `master`.

## Requirements

- macOS, Rust (`cargo`) and Xcode command line tools (`swiftc`) to build
- `jq` for the summary hook
- [Ghostty](https://ghostty.org) 1.4+ only for jumping to the exact tab of a Ghostty, tmux or Codex session; everything else works without it

## Build

```sh
./build.sh          # -> build/ccs, build/notch
build/notch &       # start the overlay
```

To check editor window matching:

```sh
swiftc -parse-as-library notch/EditorWindow.swift tests/EditorWindowTests.swift -o /tmp/ccs-editor-tests
/tmp/ccs-editor-tests
```

To check notification sounds and muting without playing audio:

```sh
swiftc -parse-as-library -D NOTCH_TESTS notch/*.swift tests/NotificationSoundTests.swift -o /tmp/ccs-notification-tests
/tmp/ccs-notification-tests
```

To check that usage data does not widen the session panel:

```sh
swiftc -parse-as-library -D NOTCH_TESTS notch/*.swift tests/NotchLayoutTests.swift -o /tmp/ccs-layout-tests
/tmp/ccs-layout-tests
```

To check live usage refreshes only happen when opening the panel:

```sh
swiftc -parse-as-library -D NOTCH_TESTS notch/*.swift tests/UsageRefreshTests.swift -o /tmp/ccs-usage-tests
/tmp/ccs-usage-tests
```

## Enable summaries

Add to `~/.claude/settings.json`:

```json
"hooks": {
  "Stop": [{ "hooks": [{ "type": "command", "command": "/ABSOLUTE/PATH/cc-sessions-viewer/hooks/summarize.sh" }] }]
}
```

Without it, the last prompt is shown instead. Summaries are for display only; "needs you" always comes from explicit pending requests.

For Codex, add a Stop hook to `~/.codex/hooks.json` (keep any existing hooks):

```json
{
  "hooks": {
    "Stop": [{ "hooks": [{ "type": "command", "command": "/ABSOLUTE/PATH/cc-sessions-viewer/hooks/summarize.sh codex" }] }]
  }
}
```

Codex summaries use `codex exec --model gpt-6-luna` with your existing Codex login. The hook summarizes recent user and assistant text, skips tools and injected instructions, and runs in the background. Luna runs read-only with coding instructions, shell tools, plugins and hooks disabled, without persisting a session. Failed or empty responses keep the previous summary. Claude continues to use Haiku.

Check both summary hooks without calling a model with `python3 tests/SummaryHookTests.py`.

## Keeping limits current

Opening the notch fetches current Claude and Codex usage concurrently. Claude uses its existing login from macOS Keychain or `~/.claude/.credentials.json`; Codex uses the ChatGPT login in `~/.codex/auth.json` (or `$CODEX_HOME/auth.json`). API-key logins have no subscription quota to fetch. Each request has a five-second timeout and keeps credentials in memory. Existing logins are never renewed or rewritten.

`ccs --limits --refresh` does the same in the terminal. Ordinary refreshes only read local caches; newer Codex session usage events override older API samples, and reset windows remain visible at 0%. A failed fetch keeps the last available values for that provider.

The status-line hook is optional. To also update the cache as you use Claude Code, add to `~/.claude/settings.json`:

```json
"statusLine": { "type": "command", "command": "/ABSOLUTE/PATH/cc-sessions-viewer/hooks/statusline.sh" }
```

It prints a short line (model, usage, context) and saves the limits to `~/.cache/ccs/claude-limits.json`. `ccs --limits` uses the newest sample for each window.

## Running next to another notch app

`build/notch --beside-notch` puts the overlay in a small pill to the right of the notch instead of on it, one window level lower, so it does not fight apps like boringNotch for the hover area.

## Ghostty quick terminal

```
keybind = global:cmd+backquote=toggle_quick_terminal
```

then run `build/ccs --watch` in it.

## Screenshots

Regenerate them from sample data with `build/notch --snapshot docs`. Add `--palettes` to also render every palette on one sheet (`palettes.png`).
