---
id: 7
title: Build the data daemon & hooks installer
labels: [wayfinder:task]
status: open
assignee:
blocked-by: [3, 6]
parent: map
---

## Question

Implement the data plane per the stack decision ("Choose the widget stack") and the spec ("Spec the state machine & multi-session aggregation"):

- Idempotent hooks installer: merge hook entries (`"type": "http"` → `http://127.0.0.1:43110/event`) into `~/.claude/settings.json` without clobbering existing keys; clean uninstall path.
- Localhost HTTP listener for hook events; single-instance enforcement.
- Watcher on `~/.claude/sessions/` (+ `/proc` validation) and transcript tailer on `~/.claude/projects/**` — inotify-based, new-directory aware, persisted byte offsets, tolerant of partial JSONL lines. (If any Node 18 component: recursive `fs.watch` is unavailable on Linux — use chokidar or per-dir watches.)
- State machine per spec; push updates to the UI within ~1s of any change.

Verify with `/verify`: drive a real session and watch states/fields change end-to-end.
