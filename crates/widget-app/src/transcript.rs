//! Tails each live session's transcript JSONL for model + token usage.
//!
//! The transcript path is derived from the session's cwd and id:
//! `~/.claude/projects/<flattened-cwd>/<sessionId>.jsonl`, where flattening replaces
//! every `/` and `.` with `-` (verified against real transcripts in spike #2). Only new
//! bytes are read each poll (a per-session byte offset), and only complete lines (up to
//! the last newline) are parsed, so a half-written final line is retried next poll.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use widget_core::{parse_transcript_line, TranscriptUpdate};

/// On a session's first poll, read at most this many trailing bytes rather than the whole
/// (possibly tens-of-MB) transcript. We only keep the latest message, which lives at the
/// end, so the tail is sufficient and bounds startup cost.
const FIRST_READ_TAIL_CAP: u64 = 512 * 1024;

#[derive(Default)]
pub struct TranscriptReader {
    /// Byte offset already consumed, per session id.
    offsets: HashMap<String, u64>,
}

impl TranscriptReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read new assistant lines for each `(session_id, cwd)` and return their updates.
    pub fn poll(&mut self, sessions: &[(String, Option<String>)]) -> Vec<TranscriptUpdate> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let mut updates = Vec::new();
        for (sid, cwd) in sessions {
            let Some(cwd) = cwd else { continue };
            let path = transcript_path(&home, cwd, sid);
            updates.extend(self.read_new(&path, sid));
        }
        updates
    }

    fn read_new(&mut self, path: &Path, sid: &str) -> Vec<TranscriptUpdate> {
        let Ok(mut file) = std::fs::File::open(path) else { return Vec::new() };
        let Ok(meta) = file.metadata() else { return Vec::new() };
        let len = meta.len();
        let prev = *self.offsets.get(sid).unwrap_or(&0);
        let first_read = !self.offsets.contains_key(sid);
        // Start where we left off; reset if the file shrank (rotation/truncation); on the
        // first read of a big file, jump to the tail so we don't slurp the whole thing.
        let mut start = if prev > len { 0 } else { prev };
        if first_read && len > FIRST_READ_TAIL_CAP {
            start = len - FIRST_READ_TAIL_CAP;
        }
        if start >= len {
            self.offsets.insert(sid.to_string(), len);
            return Vec::new();
        }
        if file.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        // Read bytes (not a String): `start` may land mid-UTF8-char after a tail seek, so
        // we must not assume the slice is valid UTF-8 until after the first newline.
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        // If we jumped into the middle of a line (tail seek), drop that partial head.
        let head = if start > prev {
            bytes.iter().position(|&b| b == b'\n').map(|i| i + 1).unwrap_or(bytes.len())
        } else {
            0
        };
        let body = &bytes[head..];
        // Only consume up to the last complete line; keep any trailing partial for later.
        let consumed_body = body.iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
        let updates: Vec<_> = body[..consumed_body]
            .split(|&b| b == b'\n')
            .filter_map(|line| std::str::from_utf8(line).ok())
            .filter_map(parse_transcript_line)
            .collect();
        self.offsets.insert(sid.to_string(), start + head as u64 + consumed_body as u64);
        updates
    }
}

fn flatten_cwd(cwd: &str) -> String {
    cwd.chars().map(|c| if c == '/' || c == '.' { '-' } else { c }).collect()
}

fn transcript_path(home: &str, cwd: &str, sid: &str) -> PathBuf {
    Path::new(home)
        .join(".claude")
        .join("projects")
        .join(flatten_cwd(cwd))
        .join(format!("{sid}.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_cwd_like_claude_code() {
        assert_eq!(flatten_cwd("/home/khalil/Desktop/claude-widget"), "-home-khalil-Desktop-claude-widget");
        assert_eq!(flatten_cwd("/a/b.c"), "-a-b-c");
    }

    #[test]
    fn reads_only_new_complete_lines() {
        let dir = std::env::temp_dir().join(format!("cw-tr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let line = |inp: u64| format!(
            "{{\"type\":\"assistant\",\"sessionId\":\"s1\",\"message\":{{\"model\":\"m\",\"usage\":{{\"input_tokens\":{inp}}}}}}}\n"
        );
        std::fs::write(&path, line(10)).unwrap();
        let mut r = TranscriptReader::new();
        let u1 = r.read_new(&path, "s1");
        assert_eq!(u1.len(), 1);
        assert_eq!(u1[0].usage.total(), 10);
        // No new bytes -> nothing.
        assert!(r.read_new(&path, "s1").is_empty());
        // Append a complete line + a partial; only the complete one is read.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(line(5).as_bytes()).unwrap();
        f.write_all(b"{\"partial\":").unwrap();
        let u2 = r.read_new(&path, "s1");
        assert_eq!(u2.len(), 1);
        assert_eq!(u2[0].usage.total(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
