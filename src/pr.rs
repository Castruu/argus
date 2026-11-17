//! Only canonical PR links on GitHub or hosts registered by the user's gh CLI.
use std::{env, fs::File, io::{BufRead, BufReader}, path::{Path, PathBuf}};

fn gh_config_dir(home: &Path) -> PathBuf {
    let dir = |name| env::var_os(name).filter(|p| !p.is_empty()).map(PathBuf::from);
    dir("GH_CONFIG_DIR").or_else(|| dir("XDG_CONFIG_HOME").map(|p| p.join("gh")))
        .unwrap_or_else(|| home.join(".config/gh"))
}

pub fn number(url: &str, home: &Path) -> Option<u64> {
    parse(url, |host| {
        // gh writes its hosts as top-level hostname keys. Ignore all account and token values.
        File::open(gh_config_dir(home).join("hosts.yml")).ok().is_some_and(|f| {
            BufReader::new(f).lines().map_while(Result::ok)
                .any(|line| line.trim_end().strip_suffix(':').is_some_and(|key| key.eq_ignore_ascii_case(host)))
        })
    })
}

fn parse(url: &str, enterprise_host: impl FnOnce(&str) -> bool) -> Option<u64> {
    let mut parts = url.strip_prefix("https://")?.trim_end_matches('/').split('/');
    let (host, owner, repo, pull, number) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let name = |s: &str| !matches!(s, "" | "." | "..")
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c));
    if parts.next().is_some() || !name(owner) || !name(repo) || pull != "pull"
        || host.is_empty() || !host.bytes().all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
        || !number.bytes().all(|c| c.is_ascii_digit()) || number.starts_with('0') {
        return None;
    }
    let number = number.parse().ok()?;
    (host.eq_ignore_ascii_case("github.com") || enterprise_host(host)).then_some(number)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allows_only_trusted_pr_destinations() {
        let check = |s| parse(s, |host| host == "git.example.com");
        assert_eq!(check("https://github.com/org/a.b-c_d/pull/42"), Some(42));
        assert_eq!(check("https://git.example.com/org/repo/pull/1/"), Some(1));
        for url in ["file:///org/repo/pull/1", "http://github.com/o/r/pull/1", "https://github.com.evil/o/r/pull/1",
            "https://git.example.com@elsewhere/o/r/pull/1", "https://github.com/o/../pull/1",
            "https://github.com/o/r/issues/1", "https://github.com/o/r/pull/1?q=x",
            "https://github.com/o/r/pull/1/extra", "https://github.com/o/r/pull/0"] {
            assert_eq!(check(url), None, "{url}");
        }
    }
}
