//! The `claude-widget statusline` subcommand — the usage-gauge data source.
//!
//! Claude Code runs the configured statusline command on every update and pipes it a
//! JSON payload on stdin; whatever the command prints becomes the session's status
//! line. Ours does two things: persist the payload's `rate_limits` as the shared
//! account-global snapshot the widget reads, and print a compact status text so the
//! entry is useful in Claude Code itself.
//!
//! It must never break the user's statusline: any failure (bad JSON, unwritable file)
//! still prints a line and exits 0.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use widget_core::{parse_statusline_input, snapshot_json, statusline_text};

/// Refuse to slurp an absurd stdin (the payload is a few KB).
const MAX_STDIN: u64 = 256 * 1024;

/// Where the account-global snapshot lives (`$HOME/.claude/claude-widget-usage.json`).
pub fn snapshot_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".claude").join("claude-widget-usage.json")
}

/// Entry point for the subcommand: stdin → snapshot file + stdout line.
pub fn run() {
    let mut input = String::new();
    let _ = std::io::stdin().lock().take(MAX_STDIN).read_to_string(&mut input);
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{}", emit(&input, &snapshot_path(), now_secs));
}

/// Parse one statusline payload, persist the snapshot (only when it actually carries
/// limit windows — an empty payload must not clobber a good last-known snapshot), and
/// return the status text to print. Injectable path so tests stay hermetic.
pub fn emit(input: &str, path: &Path, now_secs: u64) -> String {
    let Some(parsed) = parse_statusline_input(input, now_secs) else {
        return "Claude".into();
    };
    if parsed.snapshot.has_windows() {
        let _ = write_atomically(path, &snapshot_json(&parsed.snapshot));
    }
    statusline_text(&parsed)
}

/// Write via a same-directory temp file + rename, so the widget (which polls the file)
/// never reads a torn snapshot. The temp name is per-process: Claude Code runs one
/// statusline per session, so two terminal sessions can emit concurrently — a shared
/// temp file would let them interleave. Last rename wins, which is fine (both are fresh).
/// Also used by the usage-API poller (#14) — one write path for every snapshot source.
pub(crate) fn write_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cw-em-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("usage.json")
    }

    #[test]
    fn emit_persists_snapshot_and_prints_text() {
        let path = temp_path("persist");
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tracker/assets/spike3-statusline-input.sample.json"
        );
        let input = std::fs::read_to_string(fixture).unwrap();
        let text = emit(&input, &path, 1_738_420_000);
        assert_eq!(text, "Opus · ctx 34% · 5h 24% · 7d 41%");
        let written = std::fs::read_to_string(&path).unwrap();
        let snap = widget_core::parse_usage_snapshot(&written).unwrap();
        assert_eq!(snap.written_at, 1_738_420_000);
        assert_eq!(snap.five_hour.unwrap().used_percentage, 23.5);
        assert_eq!(snap.effort_level.as_deref(), Some("xhigh"));
    }

    #[test]
    fn payload_without_limits_never_clobbers_last_known() {
        let path = temp_path("noclobber");
        emit(
            r#"{"rate_limits":{"five_hour":{"used_percentage":50}}}"#,
            &path,
            100,
        );
        let good = std::fs::read_to_string(&path).unwrap();
        // Later payloads without rate_limits (pre-first-API-response) skip the write.
        let text = emit(r#"{"model":{"display_name":"Opus"}}"#, &path, 200);
        assert_eq!(text, "Opus");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), good);
    }

    #[test]
    fn garbage_input_still_prints_something() {
        let path = temp_path("garbage");
        assert_eq!(emit("not json at all", &path, 5), "Claude");
        assert!(!path.exists());
    }
}
