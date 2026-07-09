---
id: 6
title: Spec the state machine & multi-session aggregation
labels: [wayfinder:grilling]
status: open
assignee:
blocked-by: [1]
parent: map
---

## Question

Write the spec the daemon implements (use `/domain-modeling`; grounded in what "Spike: which Claude Code signals fire on this machine" proved available):

- Per-session states — working / waiting-for-permission / idle / rate-limited / gone — and the exact signal → transition table (e.g. `UserPromptSubmit`/`PreToolUse` → working; `PermissionRequest` or `Notification[permission_prompt]` → waiting; `Stop` or `Notification[idle_prompt]` → idle; `StopFailure` with `error == "rate_limit"` → rate-limited; `SessionEnd`/pid-death → gone). Fall back to transcript-tail/mtime signals where the spike showed hooks don't fire.
- Session enumeration: `~/.claude/sessions/<pid>.json` validated against `/proc/<pid>`; keying all hook/transcript data by `session_id`.
- Aggregation across concurrent sessions: precedence (any waiting > any working > all idle), per-session rows vs single aggregate, session labels (project name from cwd).
- Zero-sessions display, staleness rules ("as of" timestamps), and which transitions emit desktop notifications.
- Edge cases: session start before first API response, after `/compact` (`current_usage` null), model switch mid-session (card must update within ~1s — the socket-like requirement).
