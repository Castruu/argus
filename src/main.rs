//! ccs: list active Claude Code and Codex sessions.
//! Usage: ccs [--json] [--watch] | ccs --limits [--json]
mod limits;

use serde_json::{json, Value};
use std::{
    collections::HashMap,
    env, fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Session {
    tool: &'static str,
    id: String,
    title: String,
    cwd: String,
    repo: String,
    tty: Option<String>,
    status: String,
    idle_secs: u64,
    last_prompt: String,
    summary: Option<String>,
    waiting_for: Option<String>, // set by Claude Code while blocked on a permission prompt or question
    asks: bool,                  // last assistant message ends with a question
    needs: bool,                 // waiting on the user
    stale: bool,                 // idle for over a day
    branch: Option<String>,
    dirty: bool,
    pr: Value, // {number, state, draft, url, ci} or null
}

const STALE_SECS: u64 = 86400;
const GIT_TTL: u64 = 30; // seconds between `git status` runs per folder
const PR_TTL: u64 = 300; // seconds between GitHub lookups per folder

/// Live pid -> its tty.
type Procs = HashMap<u64, Option<String>>;

fn main() {
    let args: Vec<String> = env::args().collect();
    if let [_, flag, cwd, out] = args.as_slice() {
        if flag == "--fetch-pr" {
            return fetch_pr(cwd, Path::new(out));
        }
    }
    let as_json = args.iter().any(|a| a == "--json");
    let watch = args.iter().any(|a| a == "--watch" || a == "-w");
    if args.iter().any(|a| a == "--limits") {
        let l = limits::read(&home());
        println!("{}", if as_json { limits::to_json(&l).to_string() } else { limits::line(&l) });
        return;
    }
    loop {
        let (procs, codex_running) = processes();
        // lsof is only worth running when a codex process exists
        let open = if codex_running { open_codex_rollouts() } else { vec![] };
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

/// One `ps` call for everything: live pids with their tty, and whether Codex runs at all.
fn processes() -> (Procs, bool) {
    let out = Command::new("ps").args(["-axo", "pid=,tty=,comm="]).output().map(|o| o.stdout).unwrap_or_default();
    let (mut procs, mut codex) = (Procs::new(), false);
    for line in String::from_utf8_lossy(&out).lines() {
        let mut f = line.split_whitespace();
        let (Some(pid), Some(tty)) = (f.next().and_then(|p| p.parse().ok()), f.next()) else { continue };
        procs.insert(pid, (tty != "??").then(|| format!("/dev/{tty}")));
        codex |= f.next().is_some_and(|c| project(c).starts_with("codex"));
    }
    (procs, codex)
}

/// Reads what changed in a transcript into its cache entry (title, last_prompt, asks).
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
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(from)).is_err() || f.read_to_end(&mut buf).is_err() {
        return;
    }
    // stop at the last newline so a half-written line is read next time
    let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return };
    for line in String::from_utf8_lossy(&buf[..end]).lines() {
        let Ok(l) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(t) = l["aiTitle"].as_str() {
            entry["title"] = t.into();
        }
        if let Some(t) = l["customTitle"].as_str() {
            entry["custom_title"] = t.into(); // set by /rename; wins over the generated title
        }
        if l["type"] == "user" {
            if let Some(t) = l["message"]["content"].as_str().filter(|t| !t.starts_with('<')) {
                entry["last_prompt"] = t.chars().take(300).collect::<String>().into();
            }
        } else if l["type"] == "assistant" {
            if let Some(t) = l["message"]["content"].as_array().and_then(|a| a.iter().rev().find_map(|c| c["text"].as_str())) {
                entry["asks"] = ends_with_question(t).into();
            }
        }
    }
    entry["len"] = (from + end as u64 + 1).into();
}

fn collect(home: &Path, codex_open: &[PathBuf], procs: &Procs) -> Vec<Session> {
    // ~/.cache/ccs/cache.json keeps transcript and git results between runs, so an unchanged session costs nothing
    let cache_path = home.join(".cache/ccs/cache.json");
    let old: Value = fs::read_to_string(&cache_path).ok().and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &Value| v["t"].is_object() && v["git"].is_object())
        .unwrap_or_else(|| json!({"t": {}, "git": {}}));
    let mut cache = old.clone();
    let mut out = claude_sessions(home, procs, &mut cache);
    out.extend(codex_sessions(home, codex_open));
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
        // default branches rarely have a PR and stale sessions are not worth a GitHub call
        if !s.stale && s.branch.as_deref().is_some_and(|b| !["main", "master", "(detached)"].contains(&b)) {
            s.pr = pr_state(home, &s.cwd);
        }
        s.needs = !s.stale && (s.status == "waiting" || (s.status == "idle" && s.asks));
        let path = home.join(".cache/ccs").join(format!("{}.txt", s.id));
        s.summary = fs::read_to_string(&path)
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        // The summary hook prefixes its verdict ("WAIT: " / "DONE: "). Trust it over the question-mark
        // heuristic, but only if it was written after the session's last status change.
        let fresh = fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() <= s.idle_secs + 5);
        if let Some(t) = s.summary.take() {
            s.summary = Some(match t.split_once(": ") {
                Some((v @ ("WAIT" | "DONE"), rest)) => {
                    if fresh { s.asks = v == "WAIT"; }
                    rest.to_string()
                }
                _ => t,
            });
        }
    }
    out.sort_by(|a, b| (&a.repo, !a.needs, a.idle_secs).cmp(&(&b.repo, !b.needs, b.idle_secs)));
    // forget closed sessions, then save only if something changed
    cache["t"].as_object_mut().unwrap().retain(|k, _| out.iter().any(|s| &s.id == k));
    cache["git"].as_object_mut().unwrap().retain(|k, _| out.iter().any(|s| &s.cwd == k));
    if cache != old {
        let tmp = cache_path.with_extension(format!("{}.tmp", std::process::id()));
        fs::create_dir_all(home.join(".cache/ccs")).ok();
        if fs::write(&tmp, cache.to_string()).is_ok() {
            fs::rename(&tmp, &cache_path).ok();
        }
    }
    out
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn claude_sessions(home: &Path, procs: &Procs, cache: &mut Value) -> Vec<Session> {
    let Ok(dir) = fs::read_dir(home.join(".claude/sessions")) else { return vec![] };
    let mut out = vec![];
    for entry in dir.flatten() {
        let path = entry.path();
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
        let cwd = meta["cwd"].as_str().unwrap_or_default().to_string();
        let entry = &mut cache["t"][id.as_str()];
        if entry["path"].as_str().is_none_or(|p| !Path::new(p).exists()) {
            if let Some(p) = find_claude_transcript(home, &id) {
                *entry = json!({"path": p.to_string_lossy()});
            }
        }
        if let Some(p) = entry["path"].as_str().map(PathBuf::from) {
            scan_transcript(&p, entry);
        }
        let title = entry["custom_title"].as_str().or(entry["title"].as_str()).or(meta["name"].as_str()).unwrap_or("?").to_string();
        let last_prompt = entry["last_prompt"].as_str().unwrap_or("").to_string();
        let asks = entry["asks"] == true;
        let updated_ms = meta["statusUpdatedAt"].as_u64().or(meta["updatedAt"].as_u64()).unwrap_or(0);
        out.push(Session {
            tool: "claude",
            id,
            title,
            cwd,
            repo: String::new(),
            tty: procs[&pid].clone(),
            status: meta["status"].as_str().unwrap_or("?").to_string(),
            idle_secs: now().saturating_sub(updated_ms / 1000),
            last_prompt,
            summary: None,
            waiting_for: meta["waitingFor"].as_str().map(str::to_string),
            asks,
            needs: false,
            stale: false,
            branch: None,
            dirty: false,
            pr: Value::Null,
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

/// True when the last non-empty line ends with a question mark (ignoring closing markdown/quotes).
fn ends_with_question(text: &str) -> bool {
    text.lines().rev().find(|l| !l.trim().is_empty())
        .is_some_and(|l| l.trim().trim_end_matches(['*', '_', ')', '"', '`']).ends_with('?'))
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

/// Cached PR for the branch checked out in `cwd`, or null. GitHub is slow, so when the cache is older than
/// PR_TTL this starts `ccs --fetch-pr` in the background and returns what it has; the next run sees the result.
fn pr_state(home: &Path, cwd: &str) -> Value {
    let path = home.join(".cache/ccs").join(format!("pr{}.json", cwd.replace('/', "-")));
    let age = fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    if age.is_none_or(|a| a.as_secs() > PR_TTL) {
        // mark the attempt first, so a slow or failing lookup is not started again on the next refresh
        let marked = if path.exists() {
            fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(SystemTime::now()))
        } else {
            fs::create_dir_all(home.join(".cache/ccs")).and_then(|_| fs::write(&path, "null"))
        };
        if let (Ok(()), Ok(exe)) = (marked, env::current_exe()) {
            Command::new(exe).arg("--fetch-pr").arg(cwd).arg(&path)
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
        }
    }
    fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

/// `ccs --fetch-pr <cwd> <out>`: asks `gh` for the current branch's PR and writes it (or null) to <out>.
fn fetch_pr(cwd: &str, out: &Path) {
    // launchd starts the overlay without Homebrew on PATH
    let pr = ["gh", "/opt/homebrew/bin/gh", "/usr/local/bin/gh"].iter()
        .find_map(|gh| Command::new(gh).current_dir(cwd)
            .args(["pr", "view", "--json", "number,state,isDraft,url,statusCheckRollup"]).output().ok())
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
        .map_or(Value::Null, |v| json!({
            "number": v["number"], "state": v["state"], "draft": v["isDraft"], "url": v["url"],
            "ci": ci_of(&v["statusCheckRollup"]),
        }));
    let tmp = out.with_extension(format!("{}.tmp", std::process::id()));
    if fs::write(&tmp, pr.to_string()).is_ok() {
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

fn find_claude_transcript(home: &Path, id: &str) -> Option<PathBuf> {
    fs::read_dir(home.join(".claude/projects")).ok()?
        .flatten()
        .map(|d| d.path().join(format!("{id}.jsonl")))
        .find(|p| p.exists())
}

/// Codex has no live-session registry, but a running `codex` keeps its rollout file open,
/// so open rollouts == live sessions. Closed sessions disappear immediately.
fn open_codex_rollouts() -> Vec<PathBuf> {
    let Ok(out) = Command::new("lsof").args(["-c", "codex", "-Fn"]).output() else { return vec![] };
    let mut paths: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout).lines()
        .filter_map(|l| l.strip_prefix('n'))
        .filter(|p| p.contains("/.codex/sessions/") && p.ends_with(".jsonl"))
        .map(PathBuf::from)
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

fn codex_sessions(home: &Path, open: &[PathBuf]) -> Vec<Session> {
    let titles = read_jsonl(&home.join(".codex/session_index.jsonl"));
    let mut out = vec![];
    for path in open {
        let mtime = fs::metadata(path).and_then(|m| m.modified()).ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let idle = now().saturating_sub(mtime);
        let lines = read_jsonl(path);
        let Some(meta) = lines.iter().find(|l| l["type"] == "session_meta") else { continue };
        let meta = &meta["payload"];
        // subagent = internal reviewer threads; exec = headless `codex exec` runs
        if meta["source"]["subagent"].is_object() || meta["source"] == "exec" {
            continue;
        }
        let id = meta["id"].as_str().unwrap_or_default().to_string();
        let title = titles.iter().rev()
            .find(|t| t["id"] == id.as_str())
            .and_then(|t| t["thread_name"].as_str())
            .unwrap_or("?").to_string();
        let last_prompt = lines.iter().rev()
            .filter(|l| l["payload"]["type"] == "message" && l["payload"]["role"] == "user")
            .filter_map(|l| l["payload"]["content"][0]["text"].as_str())
            .find(|t| !t.starts_with('<') && !t.starts_with("# AGENTS.md"))
            .unwrap_or("").to_string();
        out.push(Session {
            tool: "codex",
            id,
            title,
            cwd: meta["cwd"].as_str().unwrap_or_default().to_string(),
            repo: String::new(),
            tty: None,
            status: if idle < 10 { "busy" } else { "idle" }.to_string(),
            idle_secs: idle,
            last_prompt,
            summary: None,
            waiting_for: None,
            asks: false,
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
        "tool": s.tool, "id": s.id, "title": s.title, "cwd": s.cwd,
        "project": project(&s.cwd), "repo": s.repo, "tty": s.tty, "status": s.status, "idle_secs": s.idle_secs,
        "last_prompt": s.last_prompt, "summary": s.summary,
        "waiting_for": s.waiting_for, "needs": s.needs, "stale": s.stale, "branch": s.branch, "dirty": s.dirty, "pr": s.pr,
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

fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
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
            println!("\x1b[1;36m{repo}\x1b[0m");
        }
        let dot = if s.needs { "\x1b[35m●\x1b[0m" } else if s.status == "busy" { "\x1b[33m●\x1b[0m" } else { "\x1b[32m●\x1b[0m" };
        let mut branch = s.branch.as_ref().map(|b| format!(" · {b}{}", if s.dirty { "*" } else { "" })).unwrap_or_default();
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
        println!("  {dot} \x1b[{}m{}\x1b[0m  \x1b[2m{} · {}{branch} · {status} {}\x1b[0m",
            if s.stale { 2 } else { 1 }, one_line(&s.title, 60), s.tool, project(&s.cwd), ago(s.idle_secs));
        let detail = s.summary.as_deref().unwrap_or(&s.last_prompt);
        if !detail.is_empty() {
            println!("      {}", one_line(detail, 100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            r#"{"type":"ai-title","aiTitle":"Fix login"}"#, "\n"));
        w(".claude/sessions/4.json", &format!(r#"{{"pid":{me},"sessionId":"fresh","cwd":"/x/proj","status":"waiting","waitingFor":"permission","updatedAt":{}}}"#, now() * 1000));

        w(".codex/sessions/2026/01/01/rollout-1.jsonl", concat!(
            r#"{"type":"session_meta","payload":{"id":"cx1","cwd":"/y/api","source":"cli"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"add tests"}]}}"#, "\n"));
        w(".codex/sessions/2026/01/01/rollout-closed.jsonl", r#"{"type":"session_meta","payload":{"id":"gone","cwd":"/z","source":"cli"}}"#);
        w(".codex/sessions/2026/01/01/rollout-exec.jsonl", r#"{"type":"session_meta","payload":{"id":"ex","cwd":"/z","source":"exec"}}"#);
        w(".codex/session_index.jsonl", r#"{"id":"cx1","thread_name":"API tests"}"#);
        w(".cache/ccs/abc.txt", "Fixing the login bug\n");
        w(".cache/ccs/fresh.txt", "DONE: Tests pass\n");

        let rollouts = home.join(".codex/sessions/2026/01/01");
        let open = [rollouts.join("rollout-1.jsonl"), rollouts.join("rollout-exec.jsonl")]; // rollout-closed is not open
        let procs = Procs::from([(me as u64, None)]);
        let s = collect(&home, &open, &procs);
        // second run parses only the appended line, and keeps what the cache already knew
        let t = home.join(".claude/projects/-x-proj/abc.jsonl");
        fs::write(&t, fs::read_to_string(&t).unwrap() + r#"{"type":"ai-title","aiTitle":"Fix login v2"}"# + "\n").unwrap();
        let again = collect(&home, &open, &procs);
        let a = again.iter().find(|s| s.id == "abc").unwrap();
        assert_eq!((a.title.as_str(), a.last_prompt.as_str(), a.asks), ("Fix login v2", "first ask", true));
        fs::remove_dir_all(&home).ok();
        assert_eq!(s.len(), 3, "dead pids, closed codex sessions and headless runs must be skipped");
        let c = s.iter().find(|s| s.id == "abc").unwrap();
        assert!(c.asks && c.stale && !c.needs, "an old question is stale, not needs-you");
        let f = s.iter().find(|s| s.id == "fresh").unwrap();
        assert!(f.needs, "waiting status needs the user");
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
        assert_eq!((x.title.as_str(), x.last_prompt.as_str(), x.repo.as_str()), ("API tests", "add tests", "api"));
        assert_eq!(c.repo, "proj", "non-git folder falls back to its name");
    }
}
