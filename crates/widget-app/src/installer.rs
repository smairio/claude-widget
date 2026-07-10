//! Idempotent install / uninstall of the widget's hooks and statusline entry into
//! `~/.claude/settings.json`.
//!
//! The widget receives Claude Code hook events by having each hook POST its stdin
//! JSON to the daemon's loopback listener via `curl` (a `command` hook — empirically
//! confirmed to fire in the VS Code panel in spike #2, unlike `statusLine`).
//!
//! The usage gauge (#8) is fed by a `statusLine` entry running our emitter
//! (`claude-widget statusline`). Claude Code supports exactly ONE statusline, so a
//! foreign one is left untouched (with a warning) — we never steal the slot.
//!
//! The merge is surgical: it only ever adds/refreshes/removes hook groups that carry
//! our sentinel URL and a statusline that is recognizably ours, and it never touches
//! unrelated settings keys (e.g. `effortLevel`, `model`) or a user's own hooks. If the
//! settings file exists but does not parse, we refuse to write rather than destroy
//! hand-edited content.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

/// Events the widget listens on. Skeleton scope; the alerts slice (#6) may add more.
const EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

/// Default settings file location (`$HOME/.claude/settings.json`).
pub fn settings_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".claude").join("settings.json")
}

/// The command each hook runs: pipe the hook's stdin JSON to our listener, never block.
fn hook_command(port: u16) -> String {
    format!("curl -s -m 2 -X POST --data-binary @- http://127.0.0.1:{port}/event >/dev/null 2>&1 || true")
}

/// Sentinel substring identifying a hook group as ours (so we refresh/remove only ours).
fn sentinel(port: u16) -> String {
    format!("http://127.0.0.1:{port}/event")
}

/// How the `statusLine` slot ended up after an install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatuslineOutcome {
    /// Our emitter is (now) the configured statusline.
    Installed,
    /// A foreign statusline was already configured; we left it alone, so the usage
    /// gauge has no data source. Carries the foreign command for the warning.
    KeptForeign(String),
}

/// The statusline command for the currently running binary. Quoted: the path may
/// contain spaces once users unpack the portable build anywhere.
fn statusline_command() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "claude-widget".into());
    format!("\"{exe}\" statusline")
}

/// True if a `statusLine` value is exactly the shape we write: `"<path>" statusline`
/// with a binary named `claude-widget`. A substring test is not enough — a user's own
/// script may well live under a path containing "claude-widget" (this repo does), and
/// misclassifying it as ours would displace it on install and delete it on uninstall.
fn statusline_is_ours(v: &Value) -> bool {
    v.get("command")
        .and_then(Value::as_str)
        .and_then(|c| c.strip_prefix('"'))
        .and_then(|c| c.strip_suffix("\" statusline"))
        .map(|exe| Path::new(exe).file_name() == Some(std::ffi::OsStr::new("claude-widget")))
        .unwrap_or(false)
}

fn invalid_data(msg: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

/// Read the settings file. A missing or empty file is an empty object; a present-but-
/// unparseable file is an error (so we never overwrite content we could not understand).
fn read_settings(path: &Path) -> std::io::Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(json!({})),
        Ok(s) => serde_json::from_str(&s).map_err(|e| {
            invalid_data(format!(
                "{} is not valid JSON ({e}); refusing to overwrite it",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e),
    }
}

fn write_settings(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into());
    std::fs::write(path, pretty + "\n")
}

/// True if this hook group is one of ours (any command mentions our sentinel URL).
fn group_is_ours(group: &Value, sentinel: &str) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .map(|c| c.contains(sentinel))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Filter out every hook group carrying our sentinel (foreign groups kept, order preserved).
fn strip_ours(arr: &[Value], sentinel: &str) -> Vec<Value> {
    arr.iter()
        .filter(|g| !group_is_ours(g, sentinel))
        .cloned()
        .collect()
}

/// Install (or refresh) the widget's hooks + statusline at the default settings path.
pub fn install(port: u16) -> std::io::Result<(PathBuf, StatuslineOutcome)> {
    let path = settings_path();
    let outcome = install_at(&path, port, &statusline_command())?;
    Ok((path, outcome))
}

/// Remove the widget's hooks and statusline at the default settings path.
pub fn uninstall(port: u16) -> std::io::Result<PathBuf> {
    let path = settings_path();
    uninstall_at(&path, port)?;
    Ok(path)
}

/// Install into an explicit settings file. Idempotent: re-running replaces only our
/// groups/statusline and leaves foreign hooks and every other settings key untouched.
pub fn install_at(path: &Path, port: u16, statusline_cmd: &str) -> std::io::Result<StatuslineOutcome> {
    let mut root = read_settings(path)?;
    let obj = root
        .as_object_mut()
        .ok_or_else(|| invalid_data(format!("{} top level is not a JSON object; refusing to modify", path.display())))?;

    let sentinel = sentinel(port);
    let command = hook_command(port);

    let hooks = obj.entry("hooks").or_insert_with(|| Value::Object(Map::new()));
    if !hooks.is_object() {
        *hooks = Value::Object(Map::new());
    }
    let hooks_obj = hooks.as_object_mut().unwrap();

    for event in EVENTS {
        let our_group = json!({
            "hooks": [ { "type": "command", "command": command, "timeout": 5 } ]
        });
        let existing = hooks_obj.get(*event).and_then(Value::as_array).cloned().unwrap_or_default();
        let mut kept = strip_ours(&existing, &sentinel);
        kept.push(our_group);
        hooks_obj.insert((*event).to_string(), Value::Array(kept));
    }

    // The statusline slot: take it if free or already ours (refreshing the path in case
    // the binary moved); never displace a foreign statusline.
    let outcome = match obj.get("statusLine") {
        Some(existing) if !statusline_is_ours(existing) => StatuslineOutcome::KeptForeign(
            existing
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("<non-command statusline>")
                .to_string(),
        ),
        _ => {
            obj.insert(
                "statusLine".into(),
                json!({ "type": "command", "command": statusline_cmd }),
            );
            StatuslineOutcome::Installed
        }
    };

    write_settings(path, &root)?;
    Ok(outcome)
}

/// Remove only the widget's hooks from an explicit settings file. Prunes empty event
/// arrays and an empty `hooks` object so settings return to their prior shape.
pub fn uninstall_at(path: &Path, port: u16) -> std::io::Result<()> {
    let mut root = read_settings(path)?;
    let sentinel = sentinel(port);

    // Drop the statusline only if it is ours; a foreign one is not ours to remove.
    if let Some(obj) = root.as_object_mut() {
        if obj.get("statusLine").map(statusline_is_ours).unwrap_or(false) {
            obj.remove("statusLine");
        }
    }

    if let Some(hooks_obj) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("hooks"))
        .and_then(Value::as_object_mut)
    {
        let events: Vec<String> = hooks_obj.keys().cloned().collect();
        for event in events {
            if let Some(arr) = hooks_obj.get(&event).and_then(Value::as_array) {
                let kept = strip_ours(arr, &sentinel);
                if kept.is_empty() {
                    hooks_obj.remove(&event);
                } else {
                    hooks_obj.insert(event, Value::Array(kept));
                }
            }
        }
        if hooks_obj.is_empty() {
            root.as_object_mut().unwrap().remove("hooks");
        }
    }

    write_settings(path, &root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const PORT: u16 = 43110;
    /// A stand-in for the real exe-derived statusline command.
    const CMD: &str = "\"/opt/claude-widget\" statusline";

    /// A fresh, unique settings path per test — no shared global state, parallel-safe.
    fn temp_settings() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("cw-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn install_preserves_unrelated_keys() {
        let path = temp_settings();
        write_settings(&path, &json!({"effortLevel": "xhigh", "model": "opus[1m]"})).unwrap();
        install_at(&path, PORT, CMD).unwrap();
        let v = read(&path);
        assert_eq!(v["effortLevel"], "xhigh");
        assert_eq!(v["model"], "opus[1m]");
        assert!(v["hooks"]["Stop"].is_array());
    }

    #[test]
    fn install_is_idempotent() {
        let path = temp_settings();
        install_at(&path, PORT, CMD).unwrap();
        install_at(&path, PORT, CMD).unwrap();
        let v = read(&path);
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn install_preserves_foreign_hooks_on_same_event() {
        let path = temp_settings();
        write_settings(
            &path,
            &json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo foreign"}]}]}}),
        )
        .unwrap();
        install_at(&path, PORT, CMD).unwrap();
        let stop = read(&path)["hooks"]["Stop"].as_array().unwrap().clone();
        assert_eq!(stop.len(), 2, "foreign group kept + ours added");
        assert!(stop.iter().any(|g| g["hooks"][0]["command"] == "echo foreign"));
    }

    #[test]
    fn uninstall_removes_only_ours() {
        let path = temp_settings();
        write_settings(
            &path,
            &json!({"effortLevel": "xhigh", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo foreign"}]}]}}),
        )
        .unwrap();
        install_at(&path, PORT, CMD).unwrap();
        uninstall_at(&path, PORT).unwrap();
        let v = read(&path);
        assert_eq!(v["effortLevel"], "xhigh");
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0]["hooks"][0]["command"], "echo foreign");
    }

    #[test]
    fn uninstall_with_no_foreign_hooks_drops_hooks_key() {
        let path = temp_settings();
        write_settings(&path, &json!({"model": "opus"})).unwrap();
        install_at(&path, PORT, CMD).unwrap();
        uninstall_at(&path, PORT).unwrap();
        let v = read(&path);
        assert_eq!(v["model"], "opus");
        assert!(v.get("hooks").is_none());
    }

    #[test]
    fn install_takes_the_free_statusline_slot() {
        let path = temp_settings();
        let outcome = install_at(&path, PORT, CMD).unwrap();
        assert_eq!(outcome, StatuslineOutcome::Installed);
        let v = read(&path);
        assert_eq!(v["statusLine"]["type"], "command");
        assert_eq!(v["statusLine"]["command"], CMD);
    }

    #[test]
    fn install_refreshes_our_statusline_when_the_binary_moved() {
        let path = temp_settings();
        install_at(&path, PORT, CMD).unwrap();
        let moved = "\"/new/place/claude-widget\" statusline";
        let outcome = install_at(&path, PORT, moved).unwrap();
        assert_eq!(outcome, StatuslineOutcome::Installed);
        assert_eq!(read(&path)["statusLine"]["command"], moved);
    }

    #[test]
    fn foreign_statusline_is_never_displaced() {
        let path = temp_settings();
        write_settings(
            &path,
            &json!({"statusLine": {"type": "command", "command": "my-fancy-prompt"}}),
        )
        .unwrap();
        let outcome = install_at(&path, PORT, CMD).unwrap();
        assert_eq!(outcome, StatuslineOutcome::KeptForeign("my-fancy-prompt".into()));
        let v = read(&path);
        assert_eq!(v["statusLine"]["command"], "my-fancy-prompt");
        assert!(v["hooks"]["Stop"].is_array(), "hooks still installed");
        // Uninstall must not remove the foreign statusline either.
        uninstall_at(&path, PORT).unwrap();
        assert_eq!(read(&path)["statusLine"]["command"], "my-fancy-prompt");
    }

    #[test]
    fn uninstall_removes_our_statusline() {
        let path = temp_settings();
        install_at(&path, PORT, CMD).unwrap();
        uninstall_at(&path, PORT).unwrap();
        assert!(read(&path).get("statusLine").is_none());
    }

    #[test]
    fn foreign_statusline_under_a_claude_widget_path_is_still_foreign() {
        // Regression: a user's own script may live under a path containing
        // "claude-widget" (this repo does). It must never be classified as ours.
        let foreign = "/home/user/Desktop/claude-widget/scripts/statusline.sh";
        let path = temp_settings();
        write_settings(&path, &json!({"statusLine": {"type": "command", "command": foreign}}))
            .unwrap();
        let outcome = install_at(&path, PORT, CMD).unwrap();
        assert_eq!(outcome, StatuslineOutcome::KeptForeign(foreign.into()));
        assert_eq!(read(&path)["statusLine"]["command"], foreign);
        uninstall_at(&path, PORT).unwrap();
        assert_eq!(read(&path)["statusLine"]["command"], foreign, "not ours to remove");
    }

    #[test]
    fn unparseable_settings_is_never_clobbered() {
        let path = temp_settings();
        std::fs::write(&path, "{ this is not json,,, ").unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(install_at(&path, PORT, CMD).is_err(), "must refuse to write");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "file left intact");
    }

    #[test]
    fn non_object_settings_is_rejected() {
        let path = temp_settings();
        std::fs::write(&path, "[1, 2, 3]").unwrap();
        assert!(install_at(&path, PORT, CMD).is_err());
    }
}
