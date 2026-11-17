//! Usage limits for the signed-in Claude and Codex accounts, read from what each tool already
//! caches on disk. An explicit refresh fetches both providers with their existing logins.
use crate::now;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::UNIX_EPOCH,
};

pub struct Window {
    pub label: String,
    pub used: f64,
    pub resets_at: Option<u64>,
}

/// `[(tool, windows)]` for each tool that has cached limits.
pub fn read(home: &Path) -> Vec<(&'static str, Vec<Window>)> {
    [("Claude", claude(home)), ("Codex", codex(home))].into_iter().filter(|(_, w)| !w.is_empty()).collect()
}

/// Called on hover, never by the regular local refresh loop. Credentials stay in memory.
pub fn refresh(home: &Path) -> Result<(), String> {
    let (claude, codex) = std::thread::scope(|scope| {
        let claude = scope.spawn(|| refresh_claude(home));
        let codex = refresh_codex(home);
        (claude.join().unwrap(), codex)
    });
    let errors: Vec<_> = [("Claude", claude), ("Codex", codex)].into_iter()
        .filter_map(|(tool, result)| result.err().map(|error| format!("{tool}: {error}"))).collect();
    if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
}

fn refresh_claude(home: &Path) -> Result<(), String> {
    let token = claude_token(home).ok_or("Claude Code login unavailable")?;
    // shortcut: uses Claude Code's internal usage endpoint; update if Anthropic changes it.
    let usage = get_usage("https://api.anthropic.com/api/oauth/usage",
        &format!("Authorization: Bearer {token}\nanthropic-beta: oauth-2025-04-20\n"))?;
    save_claude_usage(home, &usage).map_err(|_| "Could not cache usage response".into())
}

fn refresh_codex(home: &Path) -> Result<(), String> {
    let auth: Value = serde_json::from_slice(&fs::read(codex_dir(home).join("auth.json"))
        .map_err(|_| "Codex login unavailable")?).map_err(|_| "Invalid Codex login")?;
    let token = auth["tokens"]["access_token"].as_str().filter(|s| header_value(s))
        .ok_or("Usage requires a Codex ChatGPT login")?;
    let mut headers = format!("Authorization: Bearer {token}\n");
    if let Some(account) = auth["tokens"]["account_id"].as_str() {
        if !header_value(account) { return Err("Invalid Codex account ID".into()); }
        headers.push_str(&format!("ChatGPT-Account-Id: {account}\n"));
    }
    // shortcut: uses the same internal usage endpoint as Codex; update if OpenAI changes it.
    let usage = get_usage("https://chatgpt.com/backend-api/wham/usage", &headers)?;
    save_codex_usage(home, &usage, &auth["tokens"]["account_id"])
        .map_err(|_| "Could not cache usage response".into())
}

fn get_usage(url: &str, headers: &str) -> Result<Value, String> {
    let mut child = Command::new("curl").args([
        "--disable", "--silent", "--fail", "--max-time", "5", "--connect-timeout", "2",
        "--header", "@-", url,
    ]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().map_err(|_| "Could not start usage request")?;
    // Passing the header on stdin keeps the token out of the process arguments.
    let sent = child.stdin.take().unwrap().write_all(headers.as_bytes());
    let response = child.wait_with_output().map_err(|_| "Usage request failed")?;
    if sent.is_err() || !response.status.success() { return Err("Usage request failed".into()); }
    serde_json::from_slice(&response.stdout).map_err(|_| "Invalid usage response".into())
}

fn header_value(value: &str) -> bool { !value.is_empty() && value.bytes().all(|b| b.is_ascii_graphic()) }

fn codex_dir(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex"))
}

fn claude_token(home: &Path) -> Option<String> {
    let parse = |data: &[u8]| -> Option<String> {
        let v: Value = serde_json::from_slice(data).ok()?;
        let token = v["claudeAiOauth"]["accessToken"].as_str()?;
        header_value(token).then(|| token.to_string())
    };
    if let Some(token) = fs::read(home.join(".claude/.credentials.json")).ok().and_then(|data| parse(&data)) {
        return Some(token);
    }
    if cfg!(target_os = "macos") {
        let output = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"]).output().ok()?;
        if output.status.success() { return parse(&output.stdout); }
    }
    None
}

fn save_claude_usage(home: &Path, usage: &Value) -> std::io::Result<()> {
    let mut cached = serde_json::Map::new();
    for key in ["five_hour", "seven_day"] {
        if let Some(used) = usage[key]["utilization"].as_f64().filter(|u| *u >= 0.0) {
            cached.insert(key.into(), json!({
                "used_percentage": used,
                "resets_at": usage[key]["resets_at"].as_str().and_then(parse_iso),
            }));
        }
    }
    if cached.is_empty() { return Err(std::io::Error::other("Missing usage windows")); }
    save_usage(home, "claude", Value::Object(cached))
}

fn save_codex_usage(home: &Path, usage: &Value, account: &Value) -> std::io::Result<()> {
    let mut limits = json!({});
    for (source, key) in [("primary_window", "primary"), ("secondary_window", "secondary")] {
        let w = &usage["rate_limit"][source];
        if let (Some(used), Some(seconds)) = (w["used_percent"].as_f64().filter(|u| *u >= 0.0), w["limit_window_seconds"].as_u64().filter(|s| *s > 0)) {
            limits[key] = json!({"used_percent": used, "window_seconds": seconds, "resets_at": w["reset_at"].as_u64()});
        }
    }
    if codex_windows(&limits).is_empty() { return Err(std::io::Error::other("Missing usage windows")); }
    save_usage(home, "codex", json!({"account_id": account, "fetched_at": now(), "rate_limits": limits}))
}

fn save_usage(home: &Path, tool: &str, usage: Value) -> std::io::Result<()> {
    let path = home.join(format!(".cache/ccs/{tool}-limits.json"));
    crate::cache::prepare(home)?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    crate::cache::write_new(&tmp, &usage.to_string())?;
    let result = fs::rename(&tmp, &path);
    if result.is_err() { let _ = fs::remove_file(tmp); }
    result
}

/// Prefer the latest /usage or status-line sample, falling back per window if one is missing.
fn claude(home: &Path) -> Vec<Window> {
    let v: Value = serde_json::from_str(&fs::read_to_string(home.join(".claude.json")).unwrap_or_default()).unwrap_or_default();
    let u = &v["cachedUsageUtilization"]["utilization"];
    let path = home.join(".cache/ccs/claude-limits.json");
    let status: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap_or_default()).unwrap_or_default();
    let status_at = fs::metadata(&path).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
    let cached_at = v["cachedUsageUtilization"]["fetchedAtMs"].as_u64().unwrap_or(0);
    [("5h", "five_hour"), ("weekly", "seven_day")].iter()
        .filter_map(|(label, key)| {
            let cached = || Some(window(label.to_string(), u[*key]["utilization"].as_f64()?,
                u[*key]["resets_at"].as_str().and_then(parse_iso)));
            let live = || Some(window(label.to_string(), status[*key]["used_percentage"].as_f64()?,
                status[*key]["resets_at"].as_u64()));
            if status_at > cached_at { live().or_else(cached) } else { cached().or_else(live) }
        })
        .collect()
}

/// Prefer the API sample until a newer usage event arrives in the session logs.
fn codex(home: &Path) -> Vec<Window> {
    let auth: Value = serde_json::from_slice(&fs::read(codex_dir(home).join("auth.json")).unwrap_or_default()).unwrap_or_default();
    let cache: Value = serde_json::from_slice(&fs::read(home.join(".cache/ccs/codex-limits.json")).unwrap_or_default()).unwrap_or_default();
    let cached = if auth["tokens"]["access_token"].is_string() && cache["account_id"] == auth["tokens"]["account_id"] {
        codex_windows(&cache["rate_limits"])
    } else { vec![] };
    let newest_first = |dir: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        v.sort_unstable_by(|a, b| b.cmp(a)); // YYYY/MM/DD/rollout-<timestamp> sorts by time
        v
    };
    let files = newest_first(&codex_dir(home).join("sessions")).into_iter()
        .flat_map(|y| newest_first(&y)).flat_map(|m| newest_first(&m)).flat_map(|d| newest_first(&d))
        .take(20); // bounded: don't read every old session if none has limits
    for f in files {
        let Some((rl, at)) = last_rate_limits(&f) else { continue };
        let at = at.or_else(|| fs::metadata(&f).and_then(|m| m.modified()).ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs())).unwrap_or(0);
        if !cached.is_empty() && cache["fetched_at"].as_u64().unwrap_or(0) >= at { return cached; }
        return codex_windows(&rl);
    }
    cached
}

fn codex_windows(limits: &Value) -> Vec<Window> {
    ["primary", "secondary"].iter().filter_map(|k| {
        let w = &limits[*k];
        let seconds = w["window_seconds"].as_u64().or_else(|| w["window_minutes"].as_u64()?.checked_mul(60))?;
        Some(window(label(seconds), w["used_percent"].as_f64()?, w["resets_at"].as_u64()))
    }).collect()
}

/// Search backwards in chunks, keeping a split line for the next read. Limits can be far from
/// the end during a long turn; memory depends on the longest line, not the whole rollout.
fn last_rate_limits(path: &Path) -> Option<(Value, Option<u64>)> {
    const CHUNK: u64 = 256 * 1024;
    let mut f = File::open(path).ok()?;
    let mut end = f.metadata().ok()?.len();
    let mut pending = Vec::new();
    while end > 0 {
        let start = end.saturating_sub(CHUNK);
        let mut buf = vec![0; (end - start) as usize];
        f.seek(SeekFrom::Start(start)).ok()?;
        f.read_exact(&mut buf).ok()?;
        buf.append(&mut pending);
        let first = if start == 0 { 0 } else {
            buf.iter().position(|&b| b == b'\n').map_or(buf.len(), |i| i + 1)
        };
        let found = String::from_utf8_lossy(&buf[first..]).lines().rev()
            .filter(|l| l.contains("\"rate_limits\""))
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .map(|l| (l["payload"]["rate_limits"].clone(), l["timestamp"].as_str().and_then(parse_iso)))
            .find(|(limits, _)| limits.is_object());
        if found.is_some() {
            return found;
        }
        buf.truncate(first);
        pending = buf;
        end = start;
    }
    None
}

/// A window whose reset time has passed is back to 0%, even if the cache hasn't caught up.
fn window(label: String, used: f64, resets_at: Option<u64>) -> Window {
    if resets_at.is_some_and(|r| r <= now()) {
        return Window { label, used: 0.0, resets_at: None }; // no "resets in 1m" for a time that has passed
    }
    Window { label, used, resets_at }
}

/// Time until `at`: "1d 10h", "3h", "12m".
fn until(at: u64) -> String {
    let s = at.saturating_sub(now());
    match (s / 86_400, s % 86_400 / 3600) {
        (0, 0) => format!("{}m", (s / 60).max(1)),
        (0, h) => format!("{h}h"),
        (d, 0) => format!("{d}d"),
        (d, h) => format!("{d}d {h}h"),
    }
}

fn label(secs: u64) -> String {
    match secs {
        604_800 => "weekly".into(),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s => format!("{}m", s / 60),
    }
}

/// "Claude 5h 9% ↻2h · weekly 93% ↻21h   Codex weekly 1% ↻1d"
pub fn line(limits: &[(&str, Vec<Window>)]) -> String {
    limits.iter().map(|(tool, ws)| {
        let parts: Vec<String> = ws.iter().map(|w| {
            let reset = w.resets_at.map(|r| format!(" ↻{}", until(r))).unwrap_or_default();
            format!("{} {:.0}%{reset}", w.label, w.used)
        }).collect();
        format!("{tool} {}", parts.join(" · "))
    }).collect::<Vec<_>>().join("   ")
}

/// `[{tool, windows: [{label, used, reset}]}]`; each window is colored by its own usage in the notch.
pub fn to_json(limits: &[(&str, Vec<Window>)]) -> Value {
    limits.iter().map(|(tool, ws)| json!({
        "tool": tool,
        "windows": ws.iter().map(|w| json!({
            "label": w.label,
            "used": w.used,
            "reset": w.resets_at.map(until),
        })).collect::<Vec<_>>(),
    })).collect()
}

/// Unix seconds from "2026-11-17T16:59:59.538613+00:00" or "...Z".
fn parse_iso(s: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    // offset: last '+' or '-' after the time part, e.g. "+02:00"
    let tail = &s[19..];
    let offset = tail.rfind(['+', '-']).map_or(0, |i| {
        let sign = if tail.as_bytes()[i] == b'-' { -1 } else { 1 };
        let hh = tail.get(i + 1..i + 3).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        let mm = tail.get(i + 4..i + 6).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        sign * (hh * 3600 + mm * 60)
    });
    // days since 1970-01-01 (Howard Hinnant's days_from_civil)
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + mi * 60 + se - offset).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_newest_limits_across_chunks_and_beyond_16_mb() {
        let path = std::env::temp_dir().join(format!("ccs-tail-{}.jsonl", std::process::id()));
        let limits = |p: f64| json!({"payload": {"rate_limits": {"primary": {"used_percent": p}}}}).to_string();
        let filler = format!("{}\n", json!({"payload": {"text": "x".repeat(1000)}}));
        for suffix in [String::new(), "x".repeat(256 * 1024 - 20), filler.repeat(17_000)] {
            fs::write(&path, format!("{}\n{}\n{}\n{{\"payload\":", limits(5.0), limits(42.0), suffix)).unwrap();
            assert_eq!(last_rate_limits(&path).unwrap().0["primary"]["used_percent"], 42.0);
        }
        fs::write(&path, filler.repeat(1000)).unwrap();
        assert!(last_rate_limits(&path).is_none());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn parses_iso_times() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2026-11-17T16:59:59.538613+00:00"), Some(1_794_934_799));
        assert_eq!(parse_iso("2026-11-17T18:59:59+02:00"), Some(1_794_934_799));
    }

    #[test]
    fn reads_both_tools_and_flags_the_tightest_window() {
        let home = std::env::temp_dir().join(format!("ccs-limits-{}", std::process::id()));
        let day = home.join(".codex/sessions/2026/11/17");
        fs::create_dir_all(&day).unwrap();
        let soon = now() + 3600 + 120; // buffer so the "1h" below never becomes "59m"
        fs::write(home.join(".claude.json"), json!({"cachedUsageUtilization": {"utilization": {
            "five_hour": {"utilization": 9, "resets_at": "1970-01-01T00:00:00Z"},  // already reset -> 0%
            "seven_day": {"utilization": 93, "resets_at": "2999-01-01T00:00:00Z"}}}}).to_string()).unwrap();
        fs::write(day.join("rollout-b.jsonl"), "{\"type\":\"session_meta\"}\n").unwrap(); // newest, no limits
        fs::write(day.join("rollout-a.jsonl"), json!({"payload": {"rate_limits": {
            "primary": {"used_percent": 1.0, "window_minutes": 10080, "resets_at": soon}, "secondary": null}}}).to_string()).unwrap();

        let l = read(&home);
        fs::remove_dir_all(&home).ok();
        let summary: Vec<_> = l.iter().map(|(t, ws)| (*t, ws.iter().map(|w| (w.label.clone(), w.used)).collect::<Vec<_>>())).collect();
        assert_eq!(summary, [
            ("Claude", vec![("5h".to_string(), 0.0), ("weekly".to_string(), 93.0)]),
            ("Codex", vec![("weekly".to_string(), 1.0)]),
        ]);
        assert_eq!(to_json(&l)[0]["windows"][1]["used"], 93.0);
        assert_eq!(to_json(&l)[1]["windows"][0]["reset"], "1h");
        assert_eq!(until(now() + 34 * 3600 + 60), "1d 10h");
    }

    #[test]
    fn falls_back_to_the_status_line_file() {
        let home = std::env::temp_dir().join(format!("ccs-limits-fallback-{}", std::process::id()));
        fs::create_dir_all(home.join(".cache/ccs")).unwrap();
        fs::write(home.join(".claude.json"), "{}").unwrap();
        fs::write(home.join(".cache/ccs/claude-limits.json"),
            json!({"seven_day": {"used_percentage": 20, "resets_at": now() + 7200}}).to_string()).unwrap();
        let l = read(&home);
        fs::remove_dir_all(&home).ok();
        assert_eq!((l[0].0, l[0].1[0].label.as_str(), l[0].1[0].used), ("Claude", "weekly", 20.0));
    }

    #[test]
    fn prefers_fresh_claude_samples_and_keeps_missing_windows() {
        let home = std::env::temp_dir().join(format!("ccs-limits-fresh-{}", std::process::id()));
        fs::create_dir_all(home.join(".cache/ccs")).unwrap();
        let cache = |at| json!({"cachedUsageUtilization": {"fetchedAtMs": at, "utilization": {
            "five_hour": {"utilization": 38, "resets_at": "1970-01-01T00:00:00Z"},
            "seven_day": {"utilization": 98, "resets_at": "2999-01-01T00:00:00Z"}
        }}}).to_string();
        fs::write(home.join(".claude.json"), cache(1)).unwrap();
        fs::write(home.join(".cache/ccs/claude-limits.json"),
            json!({"seven_day": {"used_percentage": 100, "resets_at": now() + 7200}}).to_string()).unwrap();
        let used = || claude(&home).iter().map(|w| (w.label.clone(), w.used)).collect::<Vec<_>>();
        assert_eq!(used(), [("5h".into(), 0.0), ("weekly".into(), 100.0)]);
        fs::write(home.join(".claude.json"), cache((now() + 60) * 1000)).unwrap();
        assert_eq!(used(), [("5h".into(), 0.0), ("weekly".into(), 98.0)]);
        fs::write(home.join(".cache/ccs/claude-limits.json"), "invalid").unwrap();
        assert_eq!(used(), [("5h".into(), 0.0), ("weekly".into(), 98.0)]);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn caches_live_usage_without_erasing_it_on_invalid_responses() {
        let home = std::env::temp_dir().join(format!("ccs-live-usage-{}", std::process::id()));
        let live = json!({"five_hour": {"utilization": 0, "resets_at": null},
            "seven_day": {"utilization": 100, "resets_at": "2999-01-01T00:00:00Z"}});
        save_claude_usage(&home, &live).unwrap();
        let used = || claude(&home).iter().map(|w| (w.label.clone(), w.used)).collect::<Vec<_>>();
        assert_eq!(used(), [("5h".into(), 0.0), ("weekly".into(), 100.0)]);
        for invalid in [json!({}), json!({"error": "unauthorized"}), json!({"five_hour": {"utilization": "unknown"}})] {
            assert!(save_claude_usage(&home, &invalid).is_err());
            assert_eq!(used(), [("5h".into(), 0.0), ("weekly".into(), 100.0)]);
        }
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn codex_api_cache_keeps_windows_and_yields_to_newer_events_or_account_switches() {
        let home = std::env::temp_dir().join(format!("ccs-codex-api-{}", std::process::id()));
        let dir = home.join(".codex");
        let day = dir.join("sessions/2026/11/17");
        fs::create_dir_all(&day).unwrap();
        let auth = |account| json!({"tokens": {"access_token": "fixture", "account_id": account}}).to_string();
        fs::write(dir.join("auth.json"), auth("a")).unwrap();
        let log = |at| json!({"timestamp": at, "payload": {"rate_limits": {
            "primary": {"window_minutes": 10080, "used_percent": 60, "resets_at": now() + 7200}
        }}}).to_string();
        let rollout = day.join("rollout-a.jsonl");
        fs::write(&rollout, log("1970-01-01T00:00:00Z")).unwrap();
        let used = || codex(&home).iter().map(|w| (w.label.clone(), w.used)).collect::<Vec<_>>();
        let api = json!({"rate_limit": {
            "primary_window": {"used_percent": 0, "limit_window_seconds": 18000, "reset_at": null},
            "secondary_window": {"used_percent": 3, "limit_window_seconds": 604800, "reset_at": now() + 7200}
        }});
        save_codex_usage(&home, &api, &json!("a")).unwrap();
        assert_eq!(used(), [("5h".into(), 0.0), ("weekly".into(), 3.0)]);
        for invalid in [json!({}), json!({"rate_limit": {"primary_window": {"used_percent": -1, "limit_window_seconds": 18000}}})] {
            assert!(save_codex_usage(&home, &invalid, &json!("a")).is_err());
            assert_eq!(used(), [("5h".into(), 0.0), ("weekly".into(), 3.0)]);
        }
        fs::write(dir.join("auth.json"), auth("b")).unwrap();
        assert_eq!(used(), [("weekly".into(), 60.0)]);
        fs::write(dir.join("auth.json"), auth("a")).unwrap();
        fs::write(&rollout, log("2999-01-01T00:00:00Z")).unwrap();
        assert_eq!(used(), [("weekly".into(), 60.0)]);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn rejects_header_injection() {
        assert!(header_value("test-token_123.abc"));
        for bad in ["", "token\nOther: header", "token\rOther: header", "token value"] {
            assert!(!header_value(bad));
        }
    }
}
