//! Opt-in poller for the account usage endpoint — the spec's OAuth rung (issue #14).
//!
//! OFF by default; enabled only by the persisted consent `claude-widget usage-api on`.
//! The thread always runs but re-reads the consent switch before every poll, so `on`
//! takes effect within ~10s on a running widget and `off` guarantees no further
//! request — no restart needed for either. When enabled it reads the stored OAuth
//! access token from `~/.claude/.credentials.json` and GETs the usage endpoint every
//! [`POLL_SECS`], writing the result to the same atomic snapshot file the statusline
//! emitter writes — the gauge, freshness label, and staleness logic are unchanged and
//! the freshest source simply wins.
//!
//! Hard rules (spec + security review):
//! - NEVER call any token-refresh endpoint — we only ever READ the stored token.
//! - The token never appears in logs, argv (visible in /proc), or error messages; it
//!   is handed to curl via `--config -` on stdin, with `-q` FIRST so a user `~/.curlrc`
//!   (trace/proxy/dump-header options) cannot see or reroute the header, and the token
//!   charset is validated so it cannot inject extra curl-config lines.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const BETA_HEADER: &str = "anthropic-beta: oauth-2025-04-20";
/// Default cadence (spec floor is 180s).
pub(crate) const POLL_SECS: u64 = 300;
/// Backoff cap after repeated 429s.
const BACKOFF_MAX_SECS: u64 = 3600;
/// How often the sleeping thread re-reads the consent switch.
const SWITCH_CHECK_SECS: u64 = 10;

fn credentials_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".claude").join(".credentials.json")
}

/// Read the stored OAuth access token. Accepts the documented layout
/// (`claudeAiOauth.accessToken`) and a top-level `accessToken` fallback. Tokens with
/// characters outside the RFC 6750 token68 set are rejected — defense-in-depth against
/// a tampered credentials file injecting extra curl-config lines (newlines/quotes).
fn read_token(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let tok = v
        .pointer("/claudeAiOauth/accessToken")
        .or_else(|| v.get("accessToken"))
        .and_then(serde_json::Value::as_str)?;
    let valid = !tok.is_empty()
        && tok.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b));
    valid.then(|| tok.to_string())
}

/// One GET against the endpoint. Returns `(http_status, body)`. The token travels to
/// curl on stdin (`--config -`), never in argv. `-q` must stay the FIRST argument: it
/// disables `~/.curlrc`, which could otherwise trace or proxy the Authorization header.
fn fetch(token: &str) -> Option<(u16, String)> {
    let mut child = std::process::Command::new("curl")
        .args([
            "-q", "-s", "--max-time", "15",
            "-o", "-", "-w", "\n__HTTP_STATUS__:%{http_code}",
            "--config", "-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let fed = (|| -> Option<()> {
        let stdin = child.stdin.as_mut()?;
        writeln!(stdin, "url = \"{ENDPOINT}\"").ok()?;
        writeln!(stdin, "header = \"Authorization: Bearer {token}\"").ok()?;
        writeln!(stdin, "header = \"{BETA_HEADER}\"").ok()?;
        Some(())
    })();
    if fed.is_none() {
        // Don't leak a zombie: reap the child before bailing.
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // rsplit: the LAST sentinel wins, so a body containing the marker cannot spoof it.
    let (body, status_line) = text.rsplit_once("\n__HTTP_STATUS__:")?;
    let status: u16 = status_line.trim().parse().ok()?;
    Some((status, body.to_string()))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Spawn the polling thread. `log` receives status lines (never the token).
pub fn spawn(log: impl Fn(String) + Send + 'static) {
    std::thread::spawn(move || {
        let mut delay = POLL_SECS;
        loop {
            // Consent is re-read before EVERY request: `usage-api off` guarantees no
            // further fetch, and `on` is picked up within one switch-check.
            let enabled = crate::config::load(&crate::config::config_path()).usage_api;
            let sleep_secs = if enabled {
                let outcome = poll_with(fetch, &credentials_path(), &crate::emitter::snapshot_path());
                match &outcome {
                    PollOutcome::Ok => log("usage-api: 200, snapshot written".into()),
                    PollOutcome::RateLimited => log(format!(
                        "usage-api: 429, backing off to {}s",
                        next_delay(delay, &outcome)
                    )),
                    PollOutcome::Failed(why) => log(format!("usage-api: {why}")),
                }
                delay = next_delay(delay, &outcome);
                delay
            } else {
                SWITCH_CHECK_SECS
            };
            // Sleep in slices so a consent flip never waits behind a long backoff.
            let mut slept = 0;
            while slept < sleep_secs {
                let slice = SWITCH_CHECK_SECS.min(sleep_secs - slept);
                std::thread::sleep(Duration::from_secs(slice));
                slept += slice;
                if crate::config::load(&crate::config::config_path()).usage_api != enabled {
                    break;
                }
            }
        }
    });
}

#[derive(Debug, PartialEq, Eq)]
enum PollOutcome {
    Ok,
    RateLimited,
    Failed(String),
}

/// Next poll delay: success resets to the base cadence, 429 doubles up to the cap, and
/// other failures KEEP the current delay (so a 429→timeout→429 run can't undo backoff).
fn next_delay(current: u64, outcome: &PollOutcome) -> u64 {
    match outcome {
        PollOutcome::Ok => POLL_SECS,
        PollOutcome::RateLimited => (current * 2).clamp(POLL_SECS, BACKOFF_MAX_SECS),
        PollOutcome::Failed(_) => current.max(POLL_SECS),
    }
}

/// One poll cycle: read token → fetch → (on 401, re-read the stored token ONCE and
/// retry only if it changed — never any refresh flow) → parse → atomic snapshot write.
/// The fetch function is injected so the 401/paths are unit-testable without curl.
fn poll_with(
    fetch: impl Fn(&str) -> Option<(u16, String)>,
    creds: &Path,
    snapshot: &Path,
) -> PollOutcome {
    let Some(token) = read_token(creds) else {
        return PollOutcome::Failed("no stored token readable".into());
    };
    let Some((mut status, mut body)) = fetch(&token) else {
        return PollOutcome::Failed("curl failed".into());
    };
    if status == 401 {
        // Claude Code may have rotated the stored token since we read it.
        if let Some(fresh) = read_token(creds) {
            if fresh != token {
                if let Some(r) = fetch(&fresh) {
                    (status, body) = r;
                }
            }
        }
    }
    match status {
        200 => {
            let Some(mut snap) = widget_core::parse_oauth_usage(&body, now_secs()) else {
                return PollOutcome::Failed("200 but unparseable body".into());
            };
            if !snap.has_windows() {
                return PollOutcome::Failed("200 but no usage windows in body".into());
            }
            // Preserve the statusline's effort pass-through: this source has no effort
            // opinion and must not erase a field another source provided.
            snap.effort_level = std::fs::read_to_string(snapshot)
                .ok()
                .and_then(|s| widget_core::parse_usage_snapshot(&s))
                .and_then(|s| s.effort_level);
            match crate::emitter::write_atomically(snapshot, &widget_core::snapshot_json(&snap)) {
                Ok(()) => PollOutcome::Ok,
                Err(e) => PollOutcome::Failed(format!("snapshot write failed: {e}")),
            }
        }
        429 => PollOutcome::RateLimited,
        s => PollOutcome::Failed(format!("HTTP {s}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cw-ua-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_creds(dir: &Path, token: &str) -> PathBuf {
        let p = dir.join(".credentials.json");
        std::fs::write(&p, format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}"}}}}"#)).unwrap();
        p
    }

    #[test]
    fn reads_token_from_documented_layout_and_rejects_injection() {
        let dir = temp_dir("tok");
        let p = write_creds(&dir, "sk-test-123");
        assert_eq!(read_token(&p).as_deref(), Some("sk-test-123"));
        std::fs::write(&p, r#"{"accessToken":"top-level"}"#).unwrap();
        assert_eq!(read_token(&p).as_deref(), Some("top-level"));
        std::fs::write(&p, "garbage").unwrap();
        assert_eq!(read_token(&p), None);
        // A token smuggling curl-config syntax (quote/newline) is rejected outright.
        std::fs::write(&p, "{\"accessToken\":\"ab\\\"\\nurl = evil\"}").unwrap();
        assert_eq!(read_token(&p), None, "config-injection charset rejected");
    }

    #[test]
    fn backoff_grows_on_429_holds_on_failure_resets_on_success() {
        let rl = PollOutcome::RateLimited;
        assert_eq!(next_delay(300, &rl), 600);
        assert_eq!(next_delay(600, &rl), 1200);
        assert_eq!(next_delay(2400, &rl), 3600);
        assert_eq!(next_delay(3600, &rl), 3600, "capped");
        // A non-429 failure between 429s must NOT reset the backoff.
        assert_eq!(next_delay(1200, &PollOutcome::Failed("HTTP 500".into())), 1200);
        assert_eq!(next_delay(1200, &PollOutcome::Ok), POLL_SECS);
    }

    #[test]
    fn on_401_rereads_stored_token_once_and_only_retries_if_it_changed() {
        let dir = temp_dir("401");
        let creds = write_creds(&dir, "old-token");
        let snapshot = dir.join("usage.json");
        // Rotation case: first call 401s; the fetch stub swaps the stored file to the
        // rotated token, and only the rotated token succeeds.
        let calls = RefCell::new(Vec::new());
        let out = poll_with(
            |tok| {
                calls.borrow_mut().push(tok.to_string());
                if tok == "old-token" {
                    write_creds(&dir, "rotated-token");
                    Some((401, String::new()))
                } else {
                    Some((200, r#"{"five_hour":{"utilization":9}}"#.into()))
                }
            },
            &creds,
            &snapshot,
        );
        assert_eq!(out, PollOutcome::Ok);
        assert_eq!(*calls.borrow(), vec!["old-token".to_string(), "rotated-token".to_string()]);
        // Unchanged token: 401 is terminal after ONE request — no blind retry loop.
        let creds2 = write_creds(&temp_dir("401b"), "same-token");
        let count = RefCell::new(0u32);
        let out = poll_with(
            |_| {
                *count.borrow_mut() += 1;
                Some((401, String::new()))
            },
            &creds2,
            &snapshot,
        );
        assert_eq!(out, PollOutcome::Failed("HTTP 401".into()));
        assert_eq!(*count.borrow(), 1, "no retry with the same token");
    }

    #[test]
    fn success_writes_snapshot_and_preserves_statusline_effort() {
        let dir = temp_dir("ok");
        let creds = write_creds(&dir, "tok");
        let snapshot = dir.join("usage.json");
        // A statusline-written snapshot with an effort pass-through already exists.
        std::fs::write(&snapshot, r#"{"effort":{"level":"xhigh"},"five_hour":{"used_percentage":1},"written_at":5}"#).unwrap();
        let out = poll_with(
            |_| Some((200, r#"{"five_hour":{"utilization":26},"seven_day":{"utilization":25,"resets_at":1783926000}}"#.into())),
            &creds,
            &snapshot,
        );
        assert_eq!(out, PollOutcome::Ok);
        let snap = widget_core::parse_usage_snapshot(&std::fs::read_to_string(&snapshot).unwrap()).unwrap();
        assert_eq!(snap.five_hour.unwrap().used_percentage, 26.0);
        assert_eq!(snap.seven_day.unwrap().resets_at, Some(1783926000));
        assert_eq!(snap.effort_level.as_deref(), Some("xhigh"), "effort pass-through survives");
        assert!(snap.written_at > 5);
    }

    #[test]
    fn empty_or_windowless_bodies_never_clobber_the_snapshot() {
        let dir = temp_dir("noclobber");
        let creds = write_creds(&dir, "tok");
        let snapshot = dir.join("usage.json");
        std::fs::write(&snapshot, r#"{"five_hour":{"used_percentage":50},"written_at":5}"#).unwrap();
        let before = std::fs::read_to_string(&snapshot).unwrap();
        let out = poll_with(|_| Some((200, "{}".into())), &creds, &snapshot);
        assert!(matches!(out, PollOutcome::Failed(_)));
        assert_eq!(std::fs::read_to_string(&snapshot).unwrap(), before);
    }

    /// The refresh endpoint must never be referenced — greppable guarantee. The needles
    /// are assembled at runtime so their literals don't appear in this file themselves.
    #[test]
    fn never_touches_token_refresh() {
        let src = include_str!("usage_api.rs").to_lowercase();
        let snake = ["refresh", "_token"].concat();
        let endpoint = ["oauth/", "token"].concat();
        assert!(!src.contains(&snake), "no refresh-token flows");
        assert!(!src.contains(&endpoint), "no token endpoint");
    }
}
