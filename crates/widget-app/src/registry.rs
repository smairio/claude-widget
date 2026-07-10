//! Reads Claude Code's live-session registry (`~/.claude/sessions/<pid>.json`) and
//! validates each entry against `/proc/<pid>` (Linux; the widget is X11-only anyway).
//!
//! This is the authority for *which* sessions exist. The hooks are the authority for
//! what each is *doing*. Registry files are written at session start — before any hook
//! fires — so a hook always corresponds to a live registry entry.

use std::collections::BTreeSet;
use std::path::Path;

fn sessions_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".claude").join("sessions")
}

fn pid_alive(pid: u64) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// The set of session ids whose process is currently alive.
///
/// Returns `None` if the registry directory could not be read at all (transient error) —
/// callers must treat that as "don't know", NOT "no sessions", so a blip never wipes the
/// roster. `Some(set)` (possibly empty) means the directory was read successfully.
pub fn live_session_ids() -> Option<BTreeSet<String>> {
    let entries = std::fs::read_dir(sessions_dir()).ok()?;
    let mut live = BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let pid = value.get("pid").and_then(serde_json::Value::as_u64);
        let sid = value.get("sessionId").and_then(serde_json::Value::as_str);
        if let (Some(pid), Some(sid)) = (pid, sid) {
            if pid_alive(pid) {
                live.insert(sid.to_string());
            }
        }
    }
    Some(live)
}
