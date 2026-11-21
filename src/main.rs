//! ccs: list active Claude Code and Codex sessions.
//! Usage: ccs [--json] [--watch] | ccs --limits [--json] [--refresh]
mod cache;
mod limits;
mod pr;

use serde_json::{json, Value};
use std::{
    collections::HashMap,
    env, fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Session {
    tool: &'static str,
    profile: Option<String>, // config dir the session runs under: "default" for ~/.claude or ~/.codex, else the dir's name
    id: String,
    title: String,
    cwd: String,
    repo: String,
    tty: Option<String>,
    app: Option<String>, // bundle of the terminal or editor the session runs in
    status: String,
    idle_secs: u64,
    last_prompt: String,
    summary: Option<String>,
    waiting_for: Option<String>, // Claude Code's reason for waiting, including ordinary dialogs
    needs: bool,                 // waiting on the user
    stale: bool,                 // idle for over a day
    branch: Option<String>,
    dirty: bool,
    pr: Value, // {number, state, draft, url, ci} or null
}

const STALE_SECS: u64 = 86400;
const GIT_TTL: u64 = 30; // seconds between `git status` runs per folder
const PR_TTL: u64 = 300; // seconds between GitHub lookups per folder

/// Live pid -> (ps terminal field, parent pid, command).
type Procs = HashMap<u64, (Option<String>, u64, String)>;

fn main() {
    if let Err(error) = cache::prepare(&home()) {
        eprintln!("Could not secure the session cache: {error}");
        std::process::exit(1);
    }
    let args: Vec<String> = env::args().collect();
    if let [_, flag, cwd, out, url @ ..] = args.as_slice() {
        if flag == "--fetch-pr" {
            return fetch_pr(cwd, Path::new(out), url.first().map(String::as_str));
        }
    }
    let as_json = args.iter().any(|a| a == "--json");
    let watch = args.iter().any(|a| a == "--watch" || a == "-w");
    if args.iter().any(|a| a == "--limits") {
        if args.iter().any(|a| a == "--refresh") {
            if let Err(error) = limits::refresh(&home()) { eprintln!("{error}; showing cached limits"); }
        }
        let l = limits::read(&home());
        println!("{}", if as_json { limits::to_json(&l).to_string() } else { limits::line(&l) });
        return;
    }
    loop {
        let (procs, codex_running) = processes();
        // lsof is only worth running when a codex process exists
        let open = if codex_running { open_codex_rollouts(&codex_profiles(&home())) } else { vec![] };
        let sessions = collect(&home(), &open, &procs);
        if as_json {
            println!("{}", to_json(&sessions));
        } else {
            if watch {
                print!("\x1b[2J\x1b[H");
            }
            print_table(&sessions);
        }
        if !watch {
            break;
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn home() -> PathBuf {
    PathBuf::from(env::var("HOME").expect("HOME not set"))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

// macOS ps spends most of its time resolving terminal names for unrelated processes.
#[cfg(target_os = "macos")]
const PS_FIELDS: &str = "pid=,ppid=,tdev=,comm=";
#[cfg(not(target_os = "macos"))]
const PS_FIELDS: &str = "pid=,ppid=,tty=,comm=";

#[cfg(target_os = "macos")]
fn tty_path(field: &str) -> Option<String> {
    use std::ffi::{CStr, c_char};
    unsafe extern "C" {
        fn devname_r(dev: i32, mode: u16, buf: *mut c_char, len: i32) -> *mut c_char;
    }
    let (major, minor) = field.split_once('/')?;
    let dev = (u32::from(major.parse::<u8>().ok()?) << 24) | minor.parse::<u32>().ok()?;
    let mut buf = [0; 1024];
    // Darwin's devname_r writes a NUL-terminated character-device name into the supplied buffer.
    let name = unsafe { devname_r(dev as i32, 0o020000, buf.as_mut_ptr(), buf.len() as i32) };
    if name.is_null() { return None; }
    Some(format!("/dev/{}", unsafe { CStr::from_ptr(name) }.to_str().ok()?))
}

#[cfg(not(target_os = "macos"))]
fn tty_path(field: &str) -> Option<String> { Some(format!("/dev/{field}")) }

fn processes() -> (Procs, bool) {
    let out = Command::new("ps").args(["-axo", PS_FIELDS]).output().map(|o| o.stdout).unwrap_or_default();
    let (mut procs, mut codex) = (Procs::new(), false);
    for line in String::from_utf8_lossy(&out).lines() {
        // three padded columns, then the command, which may contain spaces
        let mut rest = line;
        let mut next = || {
            let t = rest.trim_start();
            let (field, tail) = t.split_at(t.find(char::is_whitespace).unwrap_or(t.len()));
            rest = tail;
            field
        };
        let (Ok(pid), Ok(ppid), tty) = (next().parse::<u64>(), next().parse::<u64>(), next()) else { continue };
        let comm = rest.trim();
        codex |= project(comm).starts_with("codex");
        procs.insert(pid, ((!matches!(tty, "??" | "?")).then(|| tty.to_owned()), ppid, comm.to_string()));
    }
    (procs, codex)
}

/// The app a process runs inside: the first ancestor that lives in an .app bundle, e.g.
/// "/Applications/Ghostty.app". None for tmux and the like, whose server is detached from any app.
fn app_of(mut pid: u64, procs: &Procs) -> Option<String> {
    for _ in 0..16 {
        let (_, ppid, comm) = procs.get(&pid)?;
        // Claude Code ships its own launcher as an .app; that is not where the session is shown
        if let Some(app) = comm.find(".app/").map(|i| &comm[..i + 4]).filter(|a| !a.contains("/claude/")) {
            return Some(app.to_string());
        }
        pid = *ppid;
    }
    None
}

/// Reads what changed in a Claude or Codex transcript into its cache entry.
/// Transcripts are append-only, so only the bytes added since the last run are parsed.
fn scan_transcript(path: &Path, entry: &mut Value) {
    let Ok(mut f) = fs::File::open(path) else { return };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut from = entry["len"].as_u64().unwrap_or(0);
    if len == from {
        return;
    }
    if len < from {
        // file was rewritten: start over
        *entry = json!({"path": entry["path"]});
        from = 0;
    }
    if f.seek(SeekFrom::Start(from)).is_err() {
        return;
    }
    let mut reader = BufReader::new(f.take(len - from));
    let mut line = Vec::new();
    loop {
        line.clear();
        let Ok(n) = reader.read_until(b'\n', &mut line) else { break };
        // Leave a half-written line at its starting offset so the next refresh retries it.
        if n == 0 || !line.ends_with(b"\n") {
            break;
        }
        from += n as u64;
        let Ok(l) = serde_json::from_slice::<Value>(&line) else { continue };
        if l["type"] == "event_msg" {
            let payload = &l["payload"];
            match payload["type"].as_str() {
                Some("task_started") => {
                    entry["status"] = "busy".into();
                    entry["active_turn"] = payload["turn_id"].clone();
                    entry["started_at"] = payload["started_at"].clone();
                }
                Some("task_complete" | "turn_aborted")
                    if entry["active_turn"].is_null() || payload["turn_id"] == entry["active_turn"] => {
                    entry["status"] = "idle".into();
                    entry["active_turn"] = Value::Null;
                }
                _ => {}
            }
        }
        if l["type"] == "session_meta" && entry["meta"].is_null() {
            let m = &l["payload"];
            entry["meta"] = json!({"id": m["id"], "cwd": m["cwd"], "source": m["source"]});
        }
        if l["type"] == "turn_context" {
            if let Some(c) = l["payload"]["cwd"].as_str() {
                entry["cwd"] = c.into();
            }
        }
        if l["payload"]["type"] == "message" && l["payload"]["role"] == "user" {
            if let Some(t) = l["payload"]["content"][0]["text"].as_str()
                .filter(|t| !t.starts_with('<') && !t.starts_with("# AGENTS.md")) {
                entry["last_prompt"] = t.chars().take(300).collect::<String>().into();
            }
        }
        if let Some(t) = l["aiTitle"].as_str() {
            entry["title"] = t.into();
        }
        if let Some(t) = l["customTitle"].as_str() {
            entry["custom_title"] = t.into(); // set by /rename; wins over the generated title
        }
        if let Some(c) = l["cwd"].as_str() {
            entry["cwd"] = c.into(); // where the session is now, e.g. a worktree it moved into
        }
        if l["type"] == "pr-link" {
            // Claude Code records the PR a session works on; no GitHub call needed for number and link
            entry["pr_link"] = json!({"number": l["prNumber"], "url": l["prUrl"]});
        }
        if l["type"] == "user" {
            if let Some(t) = l["message"]["content"].as_str().filter(|t| !t.starts_with('<')) {
                entry["last_prompt"] = t.chars().take(300).collect::<String>().into();
            }
        }
    }
    entry["len"] = from.into();
}

fn collect(home: &Path, codex_open: &[(PathBuf, u64)], procs: &Procs) -> Vec<Session> {
    // ~/.cache/ccs/cache.json keeps transcript and git results between runs, so an unchanged session costs nothing
    let cache_path = home.join(".cache/ccs/cache.json");
    let old: Value = fs::read_to_string(&cache_path).ok().and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &Value| v["v"] == 5 && v["t"].is_object() && v["git"].is_object()) // v5: replay Codex turn start times
        .unwrap_or_else(|| json!({"v": 5, "t": {}, "git": {}}));
    let mut cache = old.clone();
    let mut out = claude_sessions(home, procs, &mut cache);
    out.extend(codex_sessions(home, codex_open, procs, &mut cache));
    for s in &mut out {
        let g = &mut cache["git"][s.cwd.as_str()];
        if now().saturating_sub(g["at"].as_u64().unwrap_or(0)) > GIT_TTL {
            let (branch, dirty) = git_state(&s.cwd);
            *g = json!({"repo": repo_of(&s.cwd), "branch": branch, "dirty": dirty, "at": now()});
        }
        s.repo = g["repo"].as_str().unwrap_or_default().to_string();
        s.branch = g["branch"].as_str().map(str::to_string);
        s.dirty = g["dirty"] == true;
        s.stale = s.status != "busy" && s.idle_secs > STALE_SECS;
        // Claude sessions record their own PR (pr-link); gh only adds state and CI, looked up by URL.
        // Otherwise use the PR of the branch checked out where the session is now (its worktree, if it
        // moved into one). Default branches rarely have a PR, and stale sessions are not worth a GitHub call.
        validate_pr(home, &mut s.pr);
        if let Some(url) = s.pr["url"].as_str().map(str::to_string) {
            if !s.stale {
                let live = pr_state(home, &s.cwd, Some(&url));
                if !live.is_null() { s.pr = live; }
            }
        } else if s.pr.is_null() && !s.stale && s.branch.as_deref().is_some_and(|b| !["main", "master", "(detached)"].contains(&b)) {
            s.pr = pr_state(home, &s.cwd, None);
        }
        // shortcut: ambiguous dialogs stay quiet, add explicit request reasons when Claude exposes them.
        s.needs = !s.stale && s.status == "waiting" && matches!(s.waiting_for.as_deref(),
            Some("permission" | "permission prompt" | "input needed" | "sandbox request" | "worker request" | "goal proposal"));
        let path = home.join(".cache/ccs").join(format!("{}.txt", s.id));
        s.summary = fs::read_to_string(&path)
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        // Strip verdicts from older summaries for display; summaries never determine attention.
        if let Some(t) = s.summary.take() {
            s.summary = Some(match t.split_once(": ") {
                Some(("WAIT" | "DONE", rest)) => rest.to_string(),
                _ => t,
            });
        }
    }
    out.sort_by(|a, b| (&a.repo, !a.needs, a.idle_secs).cmp(&(&b.repo, !b.needs, b.idle_secs)));
    // forget closed sessions, then save only if something changed
    cache["t"].as_object_mut().unwrap().retain(|k, _|
        out.iter().any(|s| &s.id == k) || codex_open.iter().any(|(p, _)| p.to_str() == Some(k)));
    cache["git"].as_object_mut().unwrap().retain(|k, _| out.iter().any(|s| &s.cwd == k));
    if cache != old {
        let tmp = cache_path.with_extension(format!("{}.tmp", std::process::id()));
        if cache::prepare(home).and_then(|_| cache::write_new(&tmp, &cache.to_string())).is_ok() {
            fs::rename(&tmp, &cache_path).ok();
        }
    }
    out
}

fn read_jsonl(path: &Path, keep: impl Fn(&str) -> bool) -> impl Iterator<Item = Value> {
    fs::File::open(path).into_iter()
        .flat_map(|f| BufReader::new(f).lines().map_while(Result::ok))
        .filter(move |l| keep(l))
        .filter_map(|l| serde_json::from_str(&l).ok())
}

/// Profiles as (name, dir). "default" is the tool's usual dir and comes first; the rest are the dirs listed
/// one per line in ~/.config/ccs/<list>, named after their folder. Claude and Codex dirs with the same folder
/// name are one profile.
fn profiles(home: &Path, list: &str, default: &str) -> Vec<(String, PathBuf)> {
    let listed = fs::read_to_string(home.join(".config/ccs").join(list)).unwrap_or_default();
    let listed = listed.lines().map(str::trim).filter(|l| !l.is_empty())
        .map(|l| l.strip_prefix("~/").map_or_else(|| PathBuf::from(l), |rest| home.join(rest)))
        .map(|dir| (dir.file_name().map_or("?".into(), |n| n.to_string_lossy().into_owned()), dir));
    std::iter::once(("default".to_string(), home.join(default))).chain(listed).collect()
}

/// `~/.claude` and the CLAUDE_CONFIG_DIR dirs in ~/.config/ccs/claude-dirs.
fn claude_profiles(home: &Path) -> Vec<(String, PathBuf)> { profiles(home, "claude-dirs", ".claude") }

/// `~/.codex` and the CODEX_HOME dirs in ~/.config/ccs/codex-dirs.
fn codex_profiles(home: &Path) -> Vec<(String, PathBuf)> { profiles(home, "codex-dirs", ".codex") }

fn claude_sessions(home: &Path, procs: &Procs, cache: &mut Value) -> Vec<Session> {
    let profiles = claude_profiles(home);
    let files = profiles.iter().flat_map(|(profile, config)|
        fs::read_dir(config.join("sessions")).into_iter().flatten().flatten().map(move |e| (profile, config, e.path())));
    let mut out = vec![];
    for (profile, config, path) in files {
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(meta) = serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap_or_default()) else { continue };
        // sdk-cli = headless `claude -p` runs (e.g. our own summary hook), not sessions you work in
        let Some(pid) = meta["pid"].as_u64().filter(|p| procs.contains_key(p)) else { continue };
        if meta["entrypoint"] == "sdk-cli" {
            continue;
        }
        let id = meta["sessionId"].as_str().unwrap_or_default().to_string();
        let entry = &mut cache["t"][id.as_str()];
        if entry["path"].as_str().is_none_or(|p| !Path::new(p).exists()) {
            if let Some(p) = find_claude_transcript(config, &id) {
                *entry = json!({"path": p.to_string_lossy()});
            }
        }
        if let Some(p) = entry["path"].as_str().map(PathBuf::from) {
            scan_transcript(&p, entry);
        }
        let title = entry["custom_title"].as_str().or(entry["title"].as_str()).or(meta["name"].as_str()).unwrap_or("?").to_string();
        let last_prompt = entry["last_prompt"].as_str().unwrap_or("").to_string();
        let pr = entry["pr_link"].clone();
        // the registry keeps the folder the session started in; the transcript knows where it is now
        let cwd = entry["cwd"].as_str().or(meta["cwd"].as_str()).unwrap_or_default().to_string();
        let updated_ms = meta["statusUpdatedAt"].as_u64().or(meta["updatedAt"].as_u64()).unwrap_or(0);
        out.push(Session {
            tool: "claude",
            profile: Some(profile.clone()),
            id,
            title,
            cwd,
            repo: String::new(),
            tty: procs[&pid].0.as_deref().and_then(tty_path),
            app: app_of(pid, procs),
            status: meta["status"].as_str().unwrap_or("?").to_string(),
            idle_secs: now().saturating_sub(updated_ms / 1000),
            last_prompt,
            summary: None,
            waiting_for: meta["waitingFor"].as_str().map(str::to_string),
            needs: false,
            stale: false,
            branch: None,
            dirty: false,
            pr,
        });
    }
    out
}

/// Main repo name, so git worktrees group under their repo. Falls back to the folder name.
fn repo_of(cwd: &str) -> String {
    Command::new("git").args(["-C", cwd, "rev-parse", "--path-format=absolute", "--git-common-dir"]).output().ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_end_matches("/.git").to_string())
        .map_or_else(|| project(cwd).to_string(), |root| project(&root).to_string())
}

/// Branch name and whether the tree has uncommitted changes.
/// Cached per folder for GIT_TTL seconds by `collect`.
fn git_state(cwd: &str) -> (Option<String>, bool) {
    if cwd.is_empty() {
        return (None, false);
    }
    let out = Command::new("git").args(["--no-optional-locks", "-C", cwd, "status", "--porcelain=v2", "--branch"]).output();
    let Some(out) = out.ok().filter(|o| o.status.success()) else { return (None, false) };
    let text = String::from_utf8_lossy(&out.stdout);
    let branch = text.lines().find_map(|l| l.strip_prefix("# branch.head ")).map(str::to_string);
    (branch, text.lines().any(|l| !l.starts_with('#')))
}

fn validate_pr(home: &Path, pr: &mut Value) {
    if !pr.is_object() { return }
    let number = pr["url"].as_str().and_then(|url| pr::number(url, home));
    if number.is_none() || number != pr["number"].as_u64() {
        pr["url"] = Value::Null;
    }
}

/// Cached PR for `url` if given, else for the branch checked out in `cwd`, or null. GitHub is slow, so when the cache is older than
/// PR_TTL this starts `ccs --fetch-pr` in the background and returns what it has; the next run sees the result.
fn pr_state(home: &Path, cwd: &str, url: Option<&str>) -> Value {
    let key = url.unwrap_or(cwd).replace(['/', ':'], "-");
    let path = home.join(".cache/ccs").join(format!("pr{key}.json"));
    let age = fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    if age.is_none_or(|a| a.as_secs() > PR_TTL) {
        // mark the attempt first, so a slow or failing lookup is not started again on the next refresh
        let marked = if path.exists() {
            fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(SystemTime::now()))
        } else {
            cache::prepare(home).and_then(|_| cache::write_new(&path, "null"))
        };
        if let (Ok(()), Ok(exe)) = (marked, env::current_exe()) {
            Command::new(exe).arg("--fetch-pr").arg(cwd).arg(&path).args(url)
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
        }
    }
    let mut pr = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    validate_pr(home, &mut pr);
    pr
}

/// `ccs --fetch-pr <cwd> <out> [url]`: asks `gh` for that PR (or the current branch's) and writes it (or null) to <out>.
fn fetch_pr(cwd: &str, out: &Path, url: Option<&str>) {
    if url.is_some_and(|u| pr::number(u, &home()).is_none()) { return; }
    // launchd starts the overlay without Homebrew on PATH
    let pr = ["gh", "/opt/homebrew/bin/gh", "/usr/local/bin/gh"].iter()
        .find_map(|gh| Command::new(gh).current_dir(cwd)
            .args(["pr", "view"]).args(url).args(["--json", "number,state,isDraft,url,statusCheckRollup"]).output().ok())
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
        .map_or(Value::Null, |v| json!({
            "number": v["number"], "state": v["state"], "draft": v["isDraft"], "url": v["url"],
            "ci": ci_of(&v["statusCheckRollup"]),
        }));
    let tmp = out.with_extension(format!("{}.tmp", std::process::id()));
    if cache::write_new(&tmp, &pr.to_string()).is_ok() {
        fs::rename(&tmp, out).ok();
    }
}

/// "fail" if any check failed, else "pending" if any is still running, else "pass"; None without checks.
fn ci_of(rollup: &Value) -> Option<&'static str> {
    let checks = rollup.as_array().filter(|a| !a.is_empty())?;
    // check runs carry status + conclusion, commit statuses carry state
    let has = |keys: [&str; 2], values: &[&str]| checks.iter().any(|c| keys.iter().any(|k| c[k].as_str().is_some_and(|v| values.contains(&v))));
    Some(if has(["conclusion", "state"], &["FAILURE", "ERROR", "TIMED_OUT", "CANCELLED", "ACTION_REQUIRED", "STARTUP_FAILURE"]) {
        "fail"
    } else if has(["status", "state"], &["QUEUED", "IN_PROGRESS", "PENDING", "WAITING", "REQUESTED", "EXPECTED"]) {
        "pending"
    } else {
        "pass"
    })
}

fn find_claude_transcript(config: &Path, id: &str) -> Option<PathBuf> {
    fs::read_dir(config.join("projects")).ok()?
        .flatten()
        .map(|d| d.path().join(format!("{id}.jsonl")))
        .find(|p| p.exists())
}

/// Codex has no live-session registry, but a running `codex` keeps its rollout file open,
/// so open rollouts == live sessions. Closed sessions disappear immediately.
/// Open rollout files with the pid holding each; the pid gives the session's terminal (tty).
fn open_codex_rollouts(profiles: &[(String, PathBuf)]) -> Vec<(PathBuf, u64)> {
    let sessions: Vec<PathBuf> = profiles.iter().map(|(_, dir)| dir.join("sessions")).collect();
    let Ok(out) = Command::new("lsof").args(["-nP", "-c", "codex", "-Fpn"]).output() else { return vec![] };
    let mut pid = 0;
    let mut open = vec![];
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(p) = l.strip_prefix('p') {
            pid = p.parse().unwrap_or(0); // "p<pid>" starts each process's files
        } else if let Some(p) = l.strip_prefix('n').filter(|p| p.ends_with(".jsonl") && sessions.iter().any(|s| Path::new(p).starts_with(s))) {
            open.push((PathBuf::from(p), pid));
        }
    }
    open.sort();
    open.dedup();
    open
}

fn codex_sessions(home: &Path, open: &[(PathBuf, u64)], procs: &Procs, cache: &mut Value) -> Vec<Session> {
    if open.is_empty() { return vec![]; }
    let profiles = codex_profiles(home);
    let titles: HashMap<String, String> = profiles.iter().flat_map(|(_, dir)| read_jsonl(&dir.join("session_index.jsonl"), |_| true).collect::<Vec<_>>())
        .filter_map(|t| Some((t["id"].as_str()?.to_string(), t["thread_name"].as_str()?.to_string())))
        .collect(); // later entries replace earlier titles
    let mut out = vec![];
    let mut paths: Vec<&PathBuf> = open.iter().map(|(p, _)| p).collect();
    paths.dedup(); // `open` is sorted, so one entry per file even if several processes hold it
    for path in paths {
        let mtime = fs::metadata(path).and_then(|m| m.modified()).ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let entry = &mut cache["t"][path.to_string_lossy().as_ref()];
        if entry.is_null() { *entry = json!({"path": path.to_string_lossy()}); }
        scan_transcript(path, entry);
        let since = if entry["status"] == "busy" {
            // shortcut: older Codex events lack started_at; use file activity until those sessions end.
            entry["started_at"].as_u64().unwrap_or(mtime)
        } else { mtime };
        let meta = &entry["meta"];
        if !meta.is_object() { continue; }
        // subagent = internal reviewer threads; exec = headless `codex exec` runs
        if meta["source"]["subagent"].is_object() || meta["source"] == "exec" {
            continue;
        }
        // Prefer the CLI with a terminal when multiple processes hold the same rollout.
        let holder = open.iter().filter(|(p, _)| p == path).find_map(|(_, pid)| {
            Some((*pid, tty_path(procs.get(pid)?.0.as_deref()?)?))
        });
        let app = holder.as_ref().and_then(|(pid, _)| app_of(*pid, procs));
        let tty = holder.map(|(_, tty)| tty);
        let id = meta["id"].as_str().unwrap_or_default().to_string();
        let title = titles.get(&id).cloned().unwrap_or_else(|| "?".to_string());
        out.push(Session {
            tool: "codex",
            profile: Some(profiles.iter().skip(1).find(|(_, dir)| path.starts_with(dir)).map_or("default", |(name, _)| name).to_string()),
            id,
            title,
            cwd: entry["cwd"].as_str().or(meta["cwd"].as_str()).unwrap_or_default().to_string(),
            repo: String::new(),
            tty,
            app,
            status: entry["status"].as_str().unwrap_or("idle").to_string(),
            idle_secs: now().saturating_sub(since),
            last_prompt: entry["last_prompt"].as_str().unwrap_or_default().to_string(),
            summary: None,
            waiting_for: None,
            needs: false,
            stale: false,
            branch: None,
            dirty: false,
            pr: Value::Null,
        });
    }
    out
}

fn to_json(sessions: &[Session]) -> Value {
    let arr: Vec<Value> = sessions.iter().map(|s| json!({
        "tool": s.tool, "profile": s.profile, "id": s.id, "title": s.title, "cwd": s.cwd,
        "project": project(&s.cwd), "repo": s.repo, "tty": s.tty, "app": s.app, "status": s.status, "idle_secs": s.idle_secs,
        "last_prompt": s.last_prompt, "summary": s.summary,
        "waiting_for": s.waiting_for, "needs": s.needs, "stale": s.stale, "branch": s.branch, "dirty": s.dirty, "pr": s.pr,
        "pr_open_url": s.pr["url"],
    })).collect();
    Value::Array(arr)
}

fn project(cwd: &str) -> &str {
    cwd.rsplit('/').next().unwrap_or(cwd)
}

fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

fn terminal_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().filter(|c| !c.is_control()).collect()
}

fn one_line(s: &str, max: usize) -> String {
    let flat = terminal_text(s);
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max - 1).collect::<String>())
    } else {
        flat
    }
}

fn print_table(sessions: &[Session]) {
    if sessions.is_empty() {
        println!("no active sessions");
    }
    let mut repo = "";
    for s in sessions {
        if s.repo != repo {
            repo = &s.repo;
            println!("\x1b[1;36m{}\x1b[0m", terminal_text(repo));
        }
        let dot = if s.needs { "\x1b[35m●\x1b[0m" } else if s.status == "busy" { "\x1b[33m●\x1b[0m" } else { "\x1b[32m●\x1b[0m" };
        let mut branch = s.branch.as_ref().map(|b| format!(" · {}{}", terminal_text(b), if s.dirty { "*" } else { "" })).unwrap_or_default();
        if let Some(n) = s.pr["number"].as_u64() {
            let mark = match (s.pr["state"].as_str(), s.pr["ci"].as_str()) {
                (Some("MERGED"), _) => " merged",
                (Some("CLOSED"), _) => " closed",
                (_, Some("fail")) => " ✗",
                (_, Some("pending")) => " …",
                (_, Some("pass")) => " ✓",
                _ => "",
            };
            branch += &format!(" · #{n}{mark}");
        }
        let status = if s.needs { "needs you" } else if s.stale { "stale" } else { &s.status };
        println!("  {dot} \x1b[{}m{}\x1b[0m  \x1b[2m{} · {}{branch} · {} {}\x1b[0m",
            if s.stale { 2 } else { 1 }, one_line(&s.title, 60), s.tool, terminal_text(project(&s.cwd)), terminal_text(status), ago(s.idle_secs));
        let detail = s.summary.as_deref().unwrap_or(&s.last_prompt);
        if !detail.is_empty() {
            println!("      {}", one_line(detail, 100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn terminal_fields_cannot_emit_control_characters() {
        let input = "fix\x1b[2J\u{009b}red\x07\n café";
        assert_eq!(one_line(input, 100), "fix[2Jred café");
    }

    #[test]
    fn streams_spaced_json_and_retries_partial_transcript_lines() {
        let home = env::temp_dir().join(format!("ccs-stream-{}", std::process::id()));
        fs::create_dir_all(home.join(".codex")).unwrap();
        let claude = home.join("claude.jsonl");
        let body = [
            r#"{"customTitle": "My title"}"#,
            &json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "x".repeat(100_000)}]}}).to_string(),
            r#"{"type" : "user", "message": {"content" : "spaced prompt"}}"#,
            r#"{"type": "user", "message": {"content": "<command-name>/clear</command-name>"}}"#,
            r#"{"type": "assistant", "message": {"content": [{"text": "Ship it?"}]}}"#,
            r#"{"type": "pr-link", "prNumber": 7, "prUrl": "https://github.com/o/r/pull/7"}"#,
            r#"{"aiTitle": "Later AI title"}"#,
            r#"{"type": "user", "message": {"content": "next"#,
        ].join("\n");
        fs::write(&claude, &body).unwrap();
        let mut entry = json!({"path": claude.to_string_lossy()});
        scan_transcript(&claude, &mut entry);
        assert_eq!(entry["last_prompt"], "spaced prompt");
        assert_eq!(entry["custom_title"], "My title");
        assert_eq!(entry["title"], "Later AI title");
        assert_eq!(entry["pr_link"]["number"], 7);
        assert_eq!(entry["len"], body.rfind('\n').unwrap() + 1);
        fs::OpenOptions::new().append(true).open(&claude).unwrap().write_all(b" prompt\"}}\n").unwrap();
        scan_transcript(&claude, &mut entry);
        assert_eq!(entry["last_prompt"], "next prompt");
        assert_eq!(entry["len"], fs::metadata(&claude).unwrap().len());

        let codex = home.join("rollout.jsonl");
        fs::write(&codex, concat!(
            r#"{"type": "session_meta", "payload": {"id": "cx1", "cwd": "/x", "source": "cli"}}"#, "\n",
            r#"{"payload":{"type":"message","role":"user","content":[{"text":"old prompt"}]}}"#, "\n",
            "{\"payload\": {\"type\": \"message\", \"role\"\t:\t\"user\", \"content\": [{\"text\": \"spaced codex prompt\"}]}}\n",
            r##"{"payload": {"type": "message", "role": "user", "content": [{"text": "# AGENTS.md instructions"}]}}"##, "\n",
            r#"{"payload": {"type": "message", "role": "user", "content": [{"text": "half"#,
        )).unwrap();
        fs::write(home.join(".codex/session_index.jsonl"), concat!(
            r#"{"id":"cx1","thread_name":"Old title"}"#, "\n",
            r#"{"id": "cx1", "thread_name": "Newest title"}"#, "\n",
        )).unwrap();
        let open = [(codex.clone(), 0)];
        let mut cache = json!({"t": {}});
        let sessions = codex_sessions(&home, &open, &Procs::new(), &mut cache);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].last_prompt, "spaced codex prompt");
        assert_eq!(sessions[0].title, "Newest title");
        let key = codex.to_string_lossy();
        let offset = cache["t"][key.as_ref()]["len"].as_u64().unwrap();
        assert!(offset < fs::metadata(&codex).unwrap().len(), "retry the partial Codex line");
        codex_sessions(&home, &open, &Procs::new(), &mut cache);
        assert_eq!(cache["t"][key.as_ref()]["len"], offset);
        let prompt = "é".repeat(500);
        let tail = format!("\"}}]}}}}\n{}\n{}\n",
            json!({"payload": {"type": "message", "role": "user", "content": [{"text": prompt}]}}),
            json!({"type": "turn_context", "payload": {"cwd": "/x/worktree"}}));
        fs::OpenOptions::new().append(true).open(&codex).unwrap().write_all(tail.as_bytes()).unwrap();
        fs::OpenOptions::new().append(true).open(home.join(".codex/session_index.jsonl")).unwrap()
            .write_all(b"{\"id\":\"cx1\",\"thread_name\":\"Renamed\"}\n").unwrap();
        let sessions = codex_sessions(&home, &open, &Procs::new(), &mut cache);
        assert_eq!(sessions[0].last_prompt, "é".repeat(300));
        assert_eq!(sessions[0].cwd, "/x/worktree");
        assert_eq!(sessions[0].title, "Renamed");
        assert_eq!(cache["t"][key.as_ref()]["len"], fs::metadata(&codex).unwrap().len());
        // Rewrites must discard the previous prompt and turn's folder.
        fs::write(&codex, "{\"type\":\"session_meta\",\"payload\":{\"id\":\"cx1\",\"cwd\":\"/new\",\"source\":\"cli\"}}\n").unwrap();
        let sessions = codex_sessions(&home, &open, &Procs::new(), &mut cache);
        assert_eq!(sessions[0].cwd, "/new");
        assert!(sessions[0].last_prompt.is_empty());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn claude_needs_only_explicit_pending_requests() {
        let home = env::temp_dir().join(format!("ccs-needs-{}", std::process::id()));
        let registry = home.join(".claude/sessions/1.json");
        let transcript = home.join(".claude/projects/test/pending.jsonl");
        fs::create_dir_all(registry.parent().unwrap()).unwrap();
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::create_dir_all(home.join(".cache/ccs")).unwrap();
        fs::write(&transcript, concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Ship it?"}]}}"#, "\n",
            r#"{"type":"user","message":{"content":"<command-name>/usage</command-name>"}}"#, "\n",
        )).unwrap();
        let procs = Procs::from([(1, (None, 0, String::new()))]);
        let cases = [
            ("waiting", Some("permission"), false, true),
            ("waiting", Some("permission prompt"), false, true),
            ("waiting", Some("input needed"), false, true),
            ("waiting", Some("sandbox request"), false, true),
            ("waiting", Some("worker request"), false, true),
            ("waiting", Some("goal proposal"), false, true),
            ("waiting", Some("dialog open"), false, false),
            ("waiting", Some("unknown"), false, false),
            ("waiting", None, false, false),
            ("idle", None, false, false),
            ("idle", Some("input needed"), false, false),
            ("busy", Some("permission prompt"), false, false),
            ("waiting", Some("permission prompt"), true, false),
        ];
        for (status, reason, stale, needs) in cases {
            fs::write(&registry, json!({
                "pid": 1, "sessionId": "pending", "cwd": home, "status": status,
                "waitingFor": reason, "statusUpdatedAt": if stale { 0 } else { now() * 1000 },
            }).to_string()).unwrap();
            fs::write(home.join(".cache/ccs/pending.txt"),
                if needs { "DONE: Old summary" } else { "WAIT: Old question" }).unwrap();
            let sessions = collect(&home, &[], &procs);
            assert_eq!(sessions[0].needs, needs, "{status}, {reason:?}, stale={stale}");
            assert_eq!(sessions[0].summary.as_deref(), Some(if needs { "Old summary" } else { "Old question" }));
            // A cached question flag from an older build must never restore the heuristic.
            let path = home.join(".cache/ccs/cache.json");
            let mut cache: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            cache["t"]["pending"]["asks"] = true.into();
            fs::write(path, cache.to_string()).unwrap();
        }
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn codex_status_follows_turn_events_not_file_activity() {
        let home = env::temp_dir().join(format!("ccs-lifecycle-{}", std::process::id()));
        fs::create_dir_all(&home).unwrap();
        let path = home.join("rollout.jsonl");
        let open = [(path.clone(), 0)];
        let append = |event: Value| {
            writeln!(fs::OpenOptions::new().append(true).open(&path).unwrap(), "{event}").unwrap();
        };
        let status = || collect(&home, &open, &Procs::new()).remove(0).status;
        fs::write(&path, concat!(
            r#"{"type":"session_meta","payload":{"id":"cx","source":"cli"}}"#, "\n",
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"one"}}"#, "\n",
        )).unwrap();
        let cache_path = home.join(".cache/ccs/cache.json");
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let mut legacy = json!({"v": 3, "t": {}, "git": {}});
        legacy["t"][path.to_string_lossy().as_ref()] = json!({
            "path": path.to_string_lossy(), "len": fs::metadata(&path).unwrap().len(),
            "meta": {"id": "cx", "source": "cli"}
        });
        fs::write(&cache_path, legacy.to_string()).unwrap();
        let old = UNIX_EPOCH + Duration::from_secs(now() - 120);
        fs::File::options().write(true).open(&path).unwrap()
            .set_times(fs::FileTimes::new().set_modified(old)).unwrap();
        assert_eq!(status(), "busy", "a silent running turn must not trigger completion");
        let cache: Value = serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
        assert_eq!(cache["v"], 5, "replay old cache entries to recover the active turn");
        assert_eq!(status(), "busy", "keep the active turn across cached refreshes");
        append(json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"one"}}));
        assert_eq!(status(), "idle");
        append(json!({"type":"event_msg","payload":{"type":"token_count"}}));
        assert_eq!(status(), "idle", "post-completion writes must not restart the turn");
        append(json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"two"}}));
        assert_eq!(status(), "busy");
        append(json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"one"}}));
        assert_eq!(status(), "busy", "an earlier turn cannot end the active turn");
        append(json!({"type":"event_msg","payload":{"type":"turn_aborted","turn_id":"two"}}));
        assert_eq!(status(), "idle");
        append(json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"three"}}));
        fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(
            br#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"three""#
        ).unwrap();
        assert_eq!(status(), "busy", "retry partial completion events on the next refresh");
        fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"}}\n").unwrap();
        assert_eq!(status(), "idle");
        fs::write(&path, "{\"type\":\"session_meta\",\"payload\":{\"id\":\"cx\",\"source\":\"cli\"}}\n").unwrap();
        assert_eq!(status(), "idle", "rewriting the transcript discards cached turn state");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn codex_working_time_survives_tool_writes_and_cached_refreshes() {
        let home = env::temp_dir().join(format!("ccs-timing-{}", std::process::id()));
        fs::create_dir_all(&home).unwrap();
        let path = home.join("rollout.jsonl");
        let open = [(path.clone(), 0)];
        fs::write(&path, "{\"type\":\"session_meta\",\"payload\":{\"id\":\"cx\",\"source\":\"cli\"}}\n").unwrap();
        let append = |payload: Value| {
            writeln!(fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}",
                json!({"type": "event_msg", "payload": payload})).unwrap();
        };
        let age = || collect(&home, &open, &Procs::new()).remove(0).idle_secs;
        let started = now() - 120;
        append(json!({"type":"task_started","turn_id":"one","started_at":started}));
        // An already-cached running turn must recover its start without another task_started event.
        let cache_path = home.join(".cache/ccs/cache.json");
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let mut legacy = json!({"v": 4, "t": {}, "git": {}});
        legacy["t"][path.to_string_lossy().as_ref()] = json!({
            "path": path.to_string_lossy(), "len": fs::metadata(&path).unwrap().len(),
            "meta": {"id": "cx", "source": "cli"}, "status": "busy", "active_turn": "one"
        });
        fs::write(&cache_path, legacy.to_string()).unwrap();
        assert!((120..125).contains(&age()), "recover the turn start from a v4 cache");
        append(json!({"type":"token_count"}));
        writeln!(fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}",
            json!({"type":"response_item","payload":{"type":"function_call_output","output":"done"}})).unwrap();
        assert!((120..125).contains(&age()), "tool writes must not reset working time");
        assert!((120..125).contains(&age()), "keep the start across cached refreshes");
        append(json!({"type":"task_started","turn_id":"two","started_at":now() - 30}));
        append(json!({"type":"task_complete","turn_id":"one"}));
        assert!((30..35).contains(&age()), "a new turn resets its own timer");
        append(json!({"type":"turn_aborted","turn_id":"two"}));
        assert!(age() < 5, "idle sessions keep measuring time since the last write");
        append(json!({"type":"task_started","turn_id":"three"}));
        assert!(age() < 5, "older events without started_at fall back to file activity");
        append(json!({"type":"task_complete","turn_id":"three"}));
        assert!(age() < 5);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn reads_claude_and_codex_fixtures() {
        let home = env::temp_dir().join(format!("ccs-test-{}", std::process::id()));
        let w = |p: &str, c: &str| {
            let f = home.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, c).unwrap();
        };
        let me = std::process::id();
        w(".claude/sessions/1.json", &format!(r#"{{"pid":{me},"sessionId":"abc","cwd":"/x/proj","status":"idle","updatedAt":0}}"#));
        w(".claude/sessions/2.json", r#"{"pid":999999999,"sessionId":"dead"}"#);
        w(".claude/sessions/3.json", &format!(r#"{{"pid":{me},"sessionId":"headless","entrypoint":"sdk-cli"}}"#));
        w(".claude/projects/-x-proj/abc.jsonl", concat!(
            r#"{"type":"user","message":{"content":"first ask"}}"#, "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done.\n\n**Ship it?**"}]}}"#, "\n",
            r#"{"type":"pr-link","prNumber":7,"prUrl":"https://github.com/o/r/pull/7","prRepository":"o/r"}"#, "\n",
            r#"{"type":"ai-title","aiTitle":"Fix login"}"#, "\n"));
        w(".claude/sessions/4.json", &format!(r#"{{"pid":{me},"sessionId":"fresh","cwd":"/x/proj","status":"waiting","waitingFor":"permission","updatedAt":{}}}"#, now() * 1000));
        // started in /x/proj, then moved into a worktree; the registry still says /x/proj
        w(".claude/projects/-x-proj/fresh.jsonl", concat!(
            r#"{"type":"system","cwd":"/x/proj"}"#, "\n",
            r#"{"type":"system","cwd":"/x/worktrees/proj"}"#, "\n"));

        // a session started with CLAUDE_CONFIG_DIR lives in its own config dir
        w(".config/ccs/claude-dirs", "~/other-home\n");
        w("other-home/sessions/5.json", &format!(r#"{{"pid":{me},"sessionId":"other","cwd":"/x/proj","status":"idle","updatedAt":0}}"#));
        w("other-home/projects/-x-proj/other.jsonl", concat!(r#"{"type":"ai-title","aiTitle":"Elsewhere"}"#, "\n"));

        w(".codex/sessions/2026/01/01/rollout-1.jsonl", concat!(
            r#"{"type":"session_meta","payload":{"id":"cx1","cwd":"/y/api","source":"cli"}}"#, "\n",
            r#"{"type":"turn_context","payload":{"cwd":"/y/worktrees/api"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"add tests"}]}}"#, "\n"));
        w(".config/ccs/codex-dirs", "~/other-home\n");
        w("other-home/sessions/2026/01/01/rollout-2.jsonl", concat!(r#"{"type":"session_meta","payload":{"id":"cx2","cwd":"/y/api","source":"cli"}}"#, "\n"));
        w("other-home/session_index.jsonl", r#"{"id":"cx2","thread_name":"Other Codex"}"#);
        w(".codex/sessions/2026/01/01/rollout-closed.jsonl", r#"{"type":"session_meta","payload":{"id":"gone","cwd":"/z","source":"cli"}}"#);
        w(".codex/sessions/2026/01/01/rollout-exec.jsonl", r#"{"type":"session_meta","payload":{"id":"ex","cwd":"/z","source":"exec"}}"#);
        w(".codex/session_index.jsonl", r#"{"id":"cx1","thread_name":"API tests"}"#);
        w(".cache/ccs/abc.txt", "Fixing the login bug\n");
        w(".cache/ccs/fresh.txt", "DONE: Tests pass\n");
        w(".cache/ccs/cx1.txt", "Adding API tests\n");

        let rollouts = home.join(".codex/sessions/2026/01/01");
        // rollout-closed is not open; rollout-1 is held by a desktop-app server (no tty) and the CLI (tty)
        let open = [(rollouts.join("rollout-1.jsonl"), 1), (rollouts.join("rollout-1.jsonl"), 2), (rollouts.join("rollout-exec.jsonl"), 1),
            (home.join("other-home/sessions/2026/01/01/rollout-2.jsonl"), 1)];
        let p = |tty: Option<&str>| (tty.map(String::from), 0, String::new());
        #[cfg(target_os = "macos")]
        let terminal = {
            use std::os::unix::fs::MetadataExt;
            let dev = fs::metadata("/dev/null").unwrap().rdev();
            format!("{}/{}", dev >> 24, dev & 0xffffff)
        };
        #[cfg(not(target_os = "macos"))]
        let terminal = "null".to_string();
        let procs = Procs::from([(me as u64, p(None)), (1, p(None)), (2, p(Some(&terminal)))]);
        let s = collect(&home, &open, &procs);
        // second run parses only the appended line, and keeps what the cache already knew
        let t = home.join(".claude/projects/-x-proj/abc.jsonl");
        fs::write(&t, fs::read_to_string(&t).unwrap() + r#"{"type":"ai-title","aiTitle":"Fix login v2"}"# + "\n").unwrap();
        let again = collect(&home, &open, &procs);
        let a = again.iter().find(|s| s.id == "abc").unwrap();
        assert_eq!((a.title.as_str(), a.last_prompt.as_str()), ("Fix login v2", "first ask"));
        let cache: Value = serde_json::from_str(&fs::read_to_string(home.join(".cache/ccs/cache.json")).unwrap()).unwrap();
        let codex_path = rollouts.join("rollout-1.jsonl");
        assert_eq!(cache["t"][codex_path.to_string_lossy().as_ref()]["len"], fs::metadata(&codex_path).unwrap().len(),
            "keep the Codex transcript offset between refresh processes");
        fs::remove_dir_all(&home).ok();
        assert_eq!(s.len(), 5, "dead pids, closed codex sessions and headless runs must be skipped");
        let other = s.iter().find(|s| s.id == "other").unwrap();
        assert_eq!(other.title, "Elsewhere", "listed config dirs are scanned with their transcripts");
        let profile = |id: &str| s.iter().find(|s| s.id == id).unwrap().profile.as_deref();
        assert_eq!((profile("abc"), profile("other"), profile("cx1"), profile("cx2")),
            (Some("default"), Some("other-home"), Some("default"), Some("other-home")), "same folder name, same profile");
        assert_eq!(s.iter().find(|s| s.id == "cx2").unwrap().title, "Other Codex", "each Codex home has its own title index");
        let c = s.iter().find(|s| s.id == "abc").unwrap();
        assert!(c.stale && !c.needs, "an old question is stale, not needs-you");
        assert_eq!((c.pr["number"].as_u64(), c.pr["url"].as_str()), (Some(7), Some("https://github.com/o/r/pull/7")),
            "Claude's pr-link gives the PR without asking GitHub (stale, so no gh lookup)");
        let f = s.iter().find(|s| s.id == "fresh").unwrap();
        assert!(f.needs, "an explicit permission request needs the user");
        assert_eq!(f.summary.as_deref(), Some("Tests pass"), "verdict prefix is stripped");
        let pos = |id: &str| s.iter().position(|s| s.id == id).unwrap();
        assert!(pos("fresh") < pos("abc"), "needs-you sorts first within a repo");
        assert_eq!(ci_of(&json!([{"status": "COMPLETED", "conclusion": "SUCCESS"}, {"state": "SUCCESS"}])), Some("pass"));
        assert_eq!(ci_of(&json!([{"status": "IN_PROGRESS", "conclusion": ""}, {"state": "SUCCESS"}])), Some("pending"));
        assert_eq!(ci_of(&json!([{"status": "IN_PROGRESS"}, {"status": "COMPLETED", "conclusion": "FAILURE"}])), Some("fail"));
        assert_eq!(ci_of(&json!([])), None);
        assert_eq!((c.title.as_str(), c.last_prompt.as_str()), ("Fix login", "first ask"));
        assert_eq!(c.summary.as_deref(), Some("Fixing the login bug"));
        let x = s.iter().find(|s| s.tool == "codex").unwrap();
        assert_eq!(x.summary.as_deref(), Some("Adding API tests"));
        assert_eq!((x.title.as_str(), x.last_prompt.as_str(), x.repo.as_str()), ("API tests", "add tests", "api"));
        assert_eq!(c.repo, "proj", "non-git folder falls back to its name");
        assert_eq!(s.iter().find(|s| s.id == "fresh").unwrap().cwd, "/x/worktrees/proj", "Claude: folder from the transcript, not the registry");
        assert_eq!(x.cwd, "/y/worktrees/api", "Codex: folder from the latest turn");
        assert_eq!(x.tty.as_deref(), Some("/dev/null"), "Codex: tty of the process holding its rollout");
    }
}
