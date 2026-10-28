//! Subscription usage (rate-limit windows) for Claude and Codex, read the way their own CLIs
//! and Orca do: the user's OAuth token from the agent's credential store, sent only to the
//! agent vendor's usage endpoint. Tokens never leave this module: they are not logged and not
//! returned to the webview, which only sees percentages and reset times.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::fs_ops::{err, CmdResult};

#[derive(Serialize, Default)]
pub struct UsageWindow {
    /// "5h", "week", ... (derived from the window length).
    label: String,
    used_percent: f64,
    /// Reset time, ms since the epoch.
    resets_at: Option<u64>,
}

#[derive(Serialize, Default)]
pub struct ProviderUsage {
    windows: Vec<UsageWindow>,
    /// Why there is no data ("not signed in", "sign-in expired", HTTP status...).
    error: Option<String>,
}

#[derive(Serialize)]
pub struct UsageReport {
    claude: ProviderUsage,
    codex: ProviderUsage,
}

/// Kelpie identifies itself; it does not pose as the vendors' own CLIs.
const USER_AGENT: &str = concat!("kelpie/", env!("CARGO_PKG_VERSION"));

fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn window_label(minutes: u64) -> String {
    match minutes {
        0 => "?".into(),
        m if m % 10080 == 0 => if m == 10080 { "week".into() } else { format!("{}w", m / 10080) },
        m if m % 1440 == 0 => format!("{}d", m / 1440),
        m if m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}

/// RFC 3339 / ISO-8601 UTC timestamp → ms (enough of the format for these APIs).
pub(crate) fn parse_iso_ms(s: &str) -> Option<u64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, mo, da) = (d.next()??, d.next()??, d.next()??);
    let time: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == ':' || *c == '.').collect();
    let mut t = time.split(':');
    let (h, mi) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let sec: f64 = t.next().unwrap_or("0").parse().ok()?;
    let offset = &rest[time.len()..];
    let off_min = match offset {
        "" | "Z" | "z" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let mut p = o.get(1..)?.split(':');
            sign * (p.next()?.parse::<i64>().ok()? * 60 + p.next().unwrap_or("0").parse::<i64>().ok()?)
        }
    };
    // Days from civil (Howard Hinnant's algorithm).
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + da - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    let secs = days * 86400 + h * 3600 + mi * 60 - off_min * 60;
    u64::try_from(secs).ok().map(|s| s * 1000 + (sec * 1000.0) as u64)
}

// ---------- Claude ----------

fn default_claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude"))
}

fn default_codex_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}

fn claude_token(dir: &Path) -> Option<String> {
    let v: Value = std::fs::read_to_string(dir.join(".credentials.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .or_else(|| claude_keychain_json(dir))?;
    v["claudeAiOauth"]["accessToken"].as_str().filter(|t| !t.is_empty()).map(str::to_string)
}

/// macOS keeps Claude Code's login in the keychain instead of the credentials file: service
/// `Claude Code-credentials`, or with a custom `CLAUDE_CONFIG_DIR` that name plus `-` and the
/// first 8 hex digits of the SHA-256 of the directory path.
#[cfg(target_os = "macos")]
pub(crate) fn claude_keychain_json(dir: &Path) -> Option<Value> {
    use sha2::Digest as _;
    let service = if dir == home().join(".claude") {
        "Claude Code-credentials".to_string()
    } else {
        let hash = sha2::Sha256::digest(dir.to_string_lossy().as_bytes());
        let hex: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
        format!("Claude Code-credentials-{hex}")
    };
    let out = std::process::Command::new("security").args(["find-generic-password", "-s", &service, "-w"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).ok()
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn claude_keychain_json(_dir: &Path) -> Option<Value> {
    None
}

fn claude_window(v: &Value, minutes: u64) -> Option<UsageWindow> {
    let used = v["utilization"].as_f64().or(v["used_percentage"].as_f64())?;
    let resets_at = match &v["resets_at"] {
        Value::String(s) => parse_iso_ms(s),
        Value::Number(n) => n.as_f64().map(|s| if s > 1e12 { s as u64 } else { (s * 1000.0) as u64 }),
        _ => None,
    };
    Some(UsageWindow { label: window_label(minutes), used_percent: used.clamp(0.0, 100.0), resets_at })
}

fn claude_usage(agent: &ureq::Agent, dir: &Path) -> ProviderUsage {
    let Some(token) = claude_token(dir) else { return ProviderUsage { error: Some("not signed in".into()), ..Default::default() } };
    let resp = agent
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", USER_AGENT)
        .call();
    let body = match read_json(resp) {
        Ok(v) => v,
        Err(e) => return ProviderUsage { error: Some(e), ..Default::default() },
    };
    let windows = [("five_hour", 300), ("seven_day", 10080)]
        .into_iter()
        .filter_map(|(key, minutes)| claude_window(&body[key], minutes))
        .collect();
    ProviderUsage { windows, error: None }
}

// ---------- Codex ----------

fn codex_usage(agent: &ureq::Agent, dir: &Path) -> ProviderUsage {
    let auth: Option<Value> = std::fs::read_to_string(dir.join("auth.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
    let tokens = auth.as_ref().map(|a| &a["tokens"]);
    let Some(token) = tokens.and_then(|t| t["access_token"].as_str()).filter(|t| !t.is_empty()) else {
        return ProviderUsage { error: Some("not signed in with ChatGPT".into()), ..Default::default() };
    };
    let mut req = agent
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header("Authorization", &format!("Bearer {token}"))
        .header("User-Agent", USER_AGENT)
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "kelpie");
    if let Some(account) = tokens.and_then(|t| t["account_id"].as_str()) {
        req = req.header("ChatGPT-Account-Id", account);
    }
    let body = match read_json(req.call()) {
        Ok(v) => v,
        Err(e) => return ProviderUsage { error: Some(e), ..Default::default() },
    };
    let windows = ["primary_window", "secondary_window"]
        .into_iter()
        .filter_map(|key| {
            let w = &body["rate_limit"][key];
            let used = w["used_percent"].as_f64()?;
            let minutes = w["limit_window_seconds"].as_u64().map_or(0, |s| s.div_ceil(60));
            Some(UsageWindow {
                label: window_label(minutes),
                used_percent: used.clamp(0.0, 100.0),
                resets_at: w["reset_at"].as_u64().map(|s| s * 1000),
            })
        })
        .collect();
    ProviderUsage { windows, error: None }
}

fn read_json(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Value, String> {
    // Error texts are built here, never from the request, so no header value can leak.
    let mut resp = resp.map_err(|e| format!("network error: {}", e.to_string().chars().take(120).collect::<String>()))?;
    match resp.status().as_u16() {
        200 => {}
        401 | 403 => return Err("sign-in expired: run the agent once to refresh it".into()),
        429 => return Err("usage endpoint rate limited".into()),
        s => return Err(format!("HTTP {s}")),
    }
    let text = resp.body_mut().read_to_string().map_err(|_| "unreadable response".to_string())?;
    serde_json::from_str(&text).map_err(|_| "unexpected response".to_string())
}

/// Both providers' windows for the active accounts (`*_dir` = an account's config home; none =
/// the agent's default). Each provider fails independently.
#[tauri::command]
pub async fn usage_fetch(claude_dir: Option<String>, codex_dir: Option<String>) -> CmdResult<UsageReport> {
    let claude_dir = claude_dir.filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(default_claude_dir);
    let codex_dir = codex_dir.filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(default_codex_dir);
    tauri::async_runtime::spawn_blocking(move || {
        let agent = agent();
        let (claude, codex) = std::thread::scope(|s| {
            let c = s.spawn(|| claude_usage(&agent, &claude_dir));
            let x = codex_usage(&agent, &codex_dir);
            (c.join().unwrap_or_default(), x)
        });
        UsageReport { claude, codex }
    })
    .await
    .map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso_timestamps() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(parse_iso_ms("2026-10-20T06:00:00.500+00:00"), Some(1792476000500));
        assert_eq!(parse_iso_ms("2026-10-20T13:00:00+07:00"), Some(1792476000000));
    }

    /// `cargo test -p kelpie --lib -- --ignored --nocapture live_usage` — calls both endpoints
    /// with this machine's sign-in; prints only window labels, percentages or the error.
    #[test]
    #[ignore]
    fn live_usage() {
        let agent = agent();
        for (name, u) in [("claude", claude_usage(&agent, &default_claude_dir())), ("codex", codex_usage(&agent, &default_codex_dir()))] {
            let w: Vec<String> = u.windows.iter().map(|w| format!("{} {:.0}%", w.label, w.used_percent)).collect();
            println!("{name}: {w:?} error={:?}", u.error);
        }
    }

    #[test]
    fn labels_windows() {
        assert_eq!(window_label(300), "5h");
        assert_eq!(window_label(10080), "week");
        assert_eq!(window_label(1440), "1d");
    }
}
