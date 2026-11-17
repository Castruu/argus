//! ccs: list active Claude Code and Codex sessions.
//! Usage: ccs [--json] [--watch] | ccs --limits [--json]
mod limits;

use serde_json::{json, Value};
use std::{
    env, fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::Command,
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
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let as_json = args.iter().any(|a| a == "--json");
    let watch = args.iter().any(|a| a == "--watch" || a == "-w");
    if args.iter().any(|a| a == "--limits") {
        let l = limits::read(&home());
        println!("{}", if as_json { limits::to_json(&l).to_string() } else { limits::line(&l) });
        return;
    }
    loop {
        let sessions = collect(&home(), &open_codex_rollouts());
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

fn collect(home: &Path, codex_open: &[PathBuf]) -> Vec<Session> {
    let mut out = claude_sessions(home);
    out.extend(codex_sessions(home, codex_open));
    for s in &mut out {
        s.repo = repo_of(&s.cwd);
        s.summary = fs::read_to_string(home.join(".cache/ccs").join(format!("{}.txt", s.id)))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
    }
    out.sort_by(|a, b| (&a.repo, a.idle_secs).cmp(&(&b.repo, b.idle_secs)));
    out
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Streams a JSONL file one line at a time and parses only the lines `keep` lets through.
/// Transcripts reach hundreds of MB, mostly tool output: loading them whole cost ~4x the file
/// size in memory on every refresh. `keep` is a cheap substring check that must never drop a
/// line the caller needs; the parsed value is still checked by the caller.
fn scan_jsonl(path: &Path, keep: impl Fn(&str) -> bool, mut each: impl FnMut(Value)) {
    let Ok(f) = fs::File::open(path) else { return };
    let mut reader = BufReader::new(f);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(text) = std::str::from_utf8(&line) else { continue };
        if keep(text) {
            if let Ok(v) = serde_json::from_str(text) {
                each(v);
            }
        }
    }
}

fn pid_alive(pid: u64) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).output().map(|o| o.status.success()).unwrap_or(false)
}

fn claude_sessions(home: &Path) -> Vec<Session> {
    let Ok(dir) = fs::read_dir(home.join(".claude/sessions")) else { return vec![] };
    let mut out = vec![];
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(meta) = serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap_or_default()) else { continue };
        // sdk-cli = headless `claude -p` runs (e.g. our own summary hook), not sessions you work in
        if meta["entrypoint"] == "sdk-cli" || meta["pid"].as_u64().is_none_or(|p| !pid_alive(p)) {
            continue;
        }
        let id = meta["sessionId"].as_str().unwrap_or_default().to_string();
        let cwd = meta["cwd"].as_str().unwrap_or_default().to_string();
        let (mut custom_title, mut ai_title, mut last_prompt) = (None, None, String::new());
        if let Some(p) = find_claude_transcript(home, &id) {
            // titles, and user prompts typed as plain text (tool results are arrays, skip them unparsed)
            let keep = |l: &str| l.contains("Title\"") || (l.contains("\"type\":\"user\"") && l.contains("\"content\":\""));
            scan_jsonl(&p, keep, |l| {
                if let Some(t) = l["customTitle"].as_str() {
                    custom_title = Some(t.to_string());
                }
                if let Some(t) = l["aiTitle"].as_str() {
                    ai_title = Some(t.to_string());
                }
                if l["type"] == "user" {
                    if let Some(t) = l["message"]["content"].as_str().filter(|t| !t.starts_with('<')) {
                        last_prompt = t.to_string();
                    }
                }
            });
        }
        let title = custom_title.or(ai_title).or(meta["name"].as_str().map(String::from)).unwrap_or("?".into());
        let updated_ms = meta["statusUpdatedAt"].as_u64().or(meta["updatedAt"].as_u64()).unwrap_or(0);
        out.push(Session {
            tool: "claude",
            id,
            title,
            cwd,
            repo: String::new(),
            tty: meta["pid"].as_u64().and_then(tty_of),
            status: meta["status"].as_str().unwrap_or("?").to_string(),
            idle_secs: now().saturating_sub(updated_ms / 1000),
            last_prompt,
            summary: None,
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

fn tty_of(pid: u64) -> Option<String> {
    let out = Command::new("ps").args(["-o", "tty=", "-p", &pid.to_string()]).output().ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!t.is_empty() && t != "??").then(|| format!("/dev/{t}"))
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
    let index = home.join(".codex/session_index.jsonl");
    let mut out = vec![];
    for path in open {
        let mtime = fs::metadata(path).and_then(|m| m.modified()).ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let idle = now().saturating_sub(mtime);
        let (mut meta, mut last_prompt) = (None::<Value>, String::new());
        let keep = |l: &str| l.contains("\"session_meta\"") || l.contains("\"role\":\"user\"");
        scan_jsonl(path, keep, |mut l| {
            if meta.is_none() && l["type"] == "session_meta" {
                meta = Some(l["payload"].take());
            } else if l["payload"]["type"] == "message" && l["payload"]["role"] == "user" {
                if let Some(t) = l["payload"]["content"][0]["text"].as_str()
                    .filter(|t| !t.starts_with('<') && !t.starts_with("# AGENTS.md")) {
                    last_prompt = t.to_string();
                }
            }
        });
        let Some(meta) = meta else { continue };
        let meta = &meta;
        // subagent = internal reviewer threads; exec = headless `codex exec` runs
        if meta["source"]["subagent"].is_object() || meta["source"] == "exec" {
            continue;
        }
        let id = meta["id"].as_str().unwrap_or_default().to_string();
        let mut title = "?".to_string();
        scan_jsonl(&index, |l| l.contains(id.as_str()), |t| {
            if t["id"] == id.as_str() {
                if let Some(n) = t["thread_name"].as_str() {
                    title = n.to_string();
                }
            }
        });
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
        });
    }
    out
}

fn to_json(sessions: &[Session]) -> String {
    let arr: Vec<Value> = sessions.iter().map(|s| json!({
        "tool": s.tool, "id": s.id, "title": s.title, "cwd": s.cwd,
        "project": project(&s.cwd), "repo": s.repo, "tty": s.tty, "status": s.status, "idle_secs": s.idle_secs,
        "last_prompt": s.last_prompt, "summary": s.summary,
    })).collect();
    Value::Array(arr).to_string()
}

fn project(cwd: &str) -> &str {
    cwd.rsplit('/').next().unwrap_or(cwd)
}

fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h", s / 3600),
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
        let dot = if s.status == "busy" { "\x1b[33m●\x1b[0m" } else { "\x1b[32m●\x1b[0m" };
        println!("  {dot} \x1b[1m{}\x1b[0m  \x1b[2m{} · {} · {} {}\x1b[0m",
            one_line(&s.title, 60), s.tool, project(&s.cwd), s.status, ago(s.idle_secs));
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
            r#"{"type":"ai-title","aiTitle":"Fix login"}"#, "\n"));
        w(".codex/sessions/2026/01/01/rollout-1.jsonl", concat!(
            r#"{"type":"session_meta","payload":{"id":"cx1","cwd":"/y/api","source":"cli"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"add tests"}]}}"#, "\n"));
        w(".codex/sessions/2026/01/01/rollout-closed.jsonl", r#"{"type":"session_meta","payload":{"id":"gone","cwd":"/z","source":"cli"}}"#);
        w(".codex/sessions/2026/01/01/rollout-exec.jsonl", r#"{"type":"session_meta","payload":{"id":"ex","cwd":"/z","source":"exec"}}"#);
        w(".codex/session_index.jsonl", r#"{"id":"cx1","thread_name":"API tests"}"#);
        w(".cache/ccs/abc.txt", "Fixing the login bug\n");

        let rollouts = home.join(".codex/sessions/2026/01/01");
        let open = [rollouts.join("rollout-1.jsonl"), rollouts.join("rollout-exec.jsonl")]; // rollout-closed is not open
        let s = collect(&home, &open);
        fs::remove_dir_all(&home).ok();
        assert_eq!(s.len(), 2, "dead pids, closed codex sessions and headless runs must be skipped");
        let c = s.iter().find(|s| s.tool == "claude").unwrap();
        assert_eq!((c.title.as_str(), c.last_prompt.as_str()), ("Fix login", "first ask"));
        assert_eq!(c.summary.as_deref(), Some("Fixing the login bug"));
        let x = s.iter().find(|s| s.tool == "codex").unwrap();
        assert_eq!((x.title.as_str(), x.last_prompt.as_str(), x.repo.as_str()), ("API tests", "add tests", "api"));
        assert_eq!(c.repo, "proj", "non-git folder falls back to its name");
    }

    #[test]
    fn streams_transcripts_keeping_newest_prompt_and_custom_title() {
        let home = env::temp_dir().join(format!("ccs-stream-{}", std::process::id()));
        let w = |p: &str, c: &str| {
            let f = home.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, c).unwrap();
        };
        let me = std::process::id();
        w(".claude/sessions/1.json", &format!(r#"{{"pid":{me},"sessionId":"s1","cwd":"/x/proj","status":"busy"}}"#));
        let big = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"BIG"}]}}"#.replace("BIG", &"x".repeat(100_000));
        w(".claude/projects/-x-proj/s1.jsonl", &[
            r#"{"type":"user","message":{"content":"old ask"}}"#,
            r#"{"type":"ai-title","aiTitle":"AI name"}"#,
            r#"{"type":"custom-title","customTitle":"My name"}"#,
            &big,
            r#"{"type":"user","message":{"content":"new ask"}}"#,
            r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"ai-title","aiTitle":"Later AI name"}"#,
            r#"{"type":"user","message":{"content":"half-writ"#, // still being written
        ].join("\n"));
        let s = claude_sessions(&home);
        fs::remove_dir_all(&home).ok();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].title.as_str(), s[0].last_prompt.as_str()), ("My name", "new ask"));
    }
}
