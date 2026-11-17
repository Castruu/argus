//! Usage limits for the signed-in Claude and Codex accounts, read from what each tool already
//! caches on disk: no tokens, no network. Numbers are as fresh as the tool's last request.
use crate::{now, read_jsonl};
use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}};

pub struct Window {
    pub label: String,
    pub used: f64,
    pub resets_at: Option<u64>,
}

/// `[(tool, windows)]` for each tool that has cached limits.
pub fn read(home: &Path) -> Vec<(&'static str, Vec<Window>)> {
    [("Claude", claude(home)), ("Codex", codex(home))].into_iter().filter(|(_, w)| !w.is_empty()).collect()
}

/// Claude Code caches its /usage numbers in ~/.claude.json.
fn claude(home: &Path) -> Vec<Window> {
    let v: Value = serde_json::from_str(&fs::read_to_string(home.join(".claude.json")).unwrap_or_default()).unwrap_or_default();
    let u = &v["cachedUsageUtilization"]["utilization"];
    let cached: Vec<Window> = [("5h", "five_hour"), ("weekly", "seven_day")].iter()
        .filter_map(|(label, key)| Some(window(label.to_string(), u[*key]["utilization"].as_f64()?,
            u[*key]["resets_at"].as_str().and_then(parse_iso))))
        .collect();
    if !cached.is_empty() {
        return cached;
    }
    // Not every Claude Code install fills that cache. hooks/statusline.sh saves the same numbers
    // from the status line input, so fall back to its file.
    let v: Value = serde_json::from_str(&fs::read_to_string(home.join(".cache/ccs/claude-limits.json")).unwrap_or_default()).unwrap_or_default();
    [("5h", "five_hour"), ("weekly", "seven_day")].iter()
        .filter_map(|(label, key)| Some(window(label.to_string(), v[*key]["used_percentage"].as_f64()?, v[*key]["resets_at"].as_u64())))
        .collect()
}

/// Codex writes `rate_limits` into its session files on every turn; use the newest one.
fn codex(home: &Path) -> Vec<Window> {
    let newest_first = |dir: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        v.sort_unstable_by(|a, b| b.cmp(a)); // YYYY/MM/DD/rollout-<timestamp> sorts by time
        v
    };
    let files = newest_first(&home.join(".codex/sessions")).into_iter()
        .flat_map(|y| newest_first(&y)).flat_map(|m| newest_first(&m)).flat_map(|d| newest_first(&d))
        .take(20); // bounded: don't read every old session if none has limits
    for f in files {
        let lines = read_jsonl(&f);
        let Some(rl) = lines.iter().rev().map(|l| &l["payload"]["rate_limits"]).find(|r| r.is_object()) else { continue };
        return ["primary", "secondary"].iter()
            .filter_map(|k| {
                let w = &rl[*k];
                Some(window(label(w["window_minutes"].as_u64()? * 60), w["used_percent"].as_f64()?, w["resets_at"].as_u64()))
            })
            .collect();
    }
    vec![]
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
}
