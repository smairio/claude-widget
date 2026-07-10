//! The CW_DEBUG log sink (`~/.claude/claude-widget-debug.log`), shared by the app's
//! per-frame logging and the usage-API poller thread. Callers gate on CW_DEBUG
//! themselves; this just appends a timestamped line and never fails loudly.

use std::io::Write;

pub(crate) fn append(line: &str) {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let path = std::path::Path::new(&home).join(".claude").join("claude-widget-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(f, "{ms} {line}");
    }
}
