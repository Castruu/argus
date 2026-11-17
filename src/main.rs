//! ccs: list active Claude Code and Codex sessions.
//! Usage: ccs [--json] [--watch]
use serde_json::{json, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

// shortcut: Codex has no live-session registry, so "active" means the rollout was written recently; upgrade by matching running codex PIDs to cwd.
const CODEX_ACTIVE_SECS: u64 = 30 * 60;

struct Session {
    tool: &'static str,
    id: String,
    title: String,
    cwd: String,
    status: String,
    idle_secs: u64,
    last_prompt: String,
    summary: Option<String>,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let as_json = args.iter().any(|a| a == "--json");
    let watch = args.iter().any(|a| a == "--watch" || a == "-w");
    loop {
        let sessions = collect(&home());
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

fn collect(home: &Path) -> Vec<Session> {
    let mut out = claude_sessions(home);
    out.extend(codex_sessions(home));
    for s in &mut out {
        s.summary = fs::read_to_string(home.join(".cache/ccs").join(format!("{}.txt", s.id)))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
    }
    out.sort_by_key(|s| s.idle_secs);
    out
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
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
        if meta["pid"].as_u64().is_none_or(|p| !pid_alive(p)) {
            continue;
        }
        let id = meta["sessionId"].as_str().unwrap_or_default().to_string();
        let cwd = meta["cwd"].as_str().unwrap_or_default().to_string();
        let lines = find_claude_transcript(home, &id).map(|p| read_jsonl(&p)).unwrap_or_default();
        let title = lines.iter().rev().find_map(|l| l["aiTitle"].as_str())
            .or(meta["name"].as_str()).unwrap_or("?").to_string();
        let last_prompt = lines.iter().rev()
            .filter(|l| l["type"] == "user")
            .filter_map(|l| l["message"]["content"].as_str())
            .find(|t| !t.starts_with('<'))
            .unwrap_or("").to_string();
        let updated_ms = meta["statusUpdatedAt"].as_u64().or(meta["updatedAt"].as_u64()).unwrap_or(0);
        out.push(Session {
            tool: "claude",
            id,
            title,
            cwd,
            status: meta["status"].as_str().unwrap_or("?").to_string(),
            idle_secs: now().saturating_sub(updated_ms / 1000),
            last_prompt,
            summary: None,
        });
    }
    out
}

fn find_claude_transcript(home: &Path, id: &str) -> Option<PathBuf> {
    fs::read_dir(home.join(".claude/projects")).ok()?
        .flatten()
        .map(|d| d.path().join(format!("{id}.jsonl")))
        .find(|p| p.exists())
}

fn codex_sessions(home: &Path) -> Vec<Session> {
    let mut files = vec![];
    walk(&home.join(".codex/sessions"), &mut files);
    let titles = read_jsonl(&home.join(".codex/session_index.jsonl"));
    let mut out = vec![];
    for (path, mtime) in files {
        let idle = now().saturating_sub(mtime);
        if idle > CODEX_ACTIVE_SECS {
            continue;
        }
        let lines = read_jsonl(&path);
        let Some(meta) = lines.iter().find(|l| l["type"] == "session_meta") else { continue };
        let meta = &meta["payload"];
        if meta["source"]["subagent"].is_object() {
            continue; // internal reviewer threads, not user sessions
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
            status: if idle < 10 { "busy" } else { "idle" }.to_string(),
            idle_secs: idle,
            last_prompt,
            summary: None,
        });
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            let mtime = e.metadata().and_then(|m| m.modified()).ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            out.push((p, mtime));
        }
    }
}

fn to_json(sessions: &[Session]) -> String {
    let arr: Vec<Value> = sessions.iter().map(|s| json!({
        "tool": s.tool, "id": s.id, "title": s.title, "cwd": s.cwd,
        "project": project(&s.cwd), "status": s.status, "idle_secs": s.idle_secs,
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
    for s in sessions {
        let dot = if s.status == "busy" { "\x1b[33m●\x1b[0m" } else { "\x1b[32m●\x1b[0m" };
        println!("{dot} \x1b[1m{}\x1b[0m  \x1b[2m{} · {} · {} {}\x1b[0m",
            one_line(&s.title, 60), s.tool, project(&s.cwd), s.status, ago(s.idle_secs));
        let detail = s.summary.as_deref().unwrap_or(&s.last_prompt);
        if !detail.is_empty() {
            println!("    {}", one_line(detail, 100));
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
        w(".claude/projects/-x-proj/abc.jsonl", concat!(
            r#"{"type":"user","message":{"content":"first ask"}}"#, "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#, "\n",
            r#"{"type":"ai-title","aiTitle":"Fix login"}"#, "\n"));
        w(".codex/sessions/2026/01/01/rollout-1.jsonl", concat!(
            r#"{"type":"session_meta","payload":{"id":"cx1","cwd":"/y/api","source":"cli"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"add tests"}]}}"#, "\n"));
        w(".codex/session_index.jsonl", r#"{"id":"cx1","thread_name":"API tests"}"#);
        w(".cache/ccs/abc.txt", "Fixing the login bug\n");

        let s = collect(&home);
        fs::remove_dir_all(&home).ok();
        assert_eq!(s.len(), 2, "dead pid must be skipped");
        let c = s.iter().find(|s| s.tool == "claude").unwrap();
        assert_eq!((c.title.as_str(), c.last_prompt.as_str()), ("Fix login", "first ask"));
        assert_eq!(c.summary.as_deref(), Some("Fixing the login bug"));
        let x = s.iter().find(|s| s.tool == "codex").unwrap();
        assert_eq!((x.title.as_str(), x.last_prompt.as_str(), project(&x.cwd)), ("API tests", "add tests", "api"));
    }
}
