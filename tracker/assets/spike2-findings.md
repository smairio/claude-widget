# Spike #2 findings — do Claude Code hooks fire from the VS Code panel?

**Date:** 2026-07-10 · **Claude Code:** v2.1.205 · **Session type:** `claude-vscode` (graphical panel) · **OS:** Ubuntu 24.04, X11

## Method

Merged temporary logging hooks (append-only, always `exit 0`) into `~/.claude/settings.json` **mid-session**, for `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PermissionRequest`, `Notification`, `Stop`, `SessionEnd`, plus a `statusLine` command with `refreshInterval: 5`. Each invocation recorded a receive-timestamp and the full stdin payload. Events were then triggered by normal use of this live panel session. Instrumentation was reverted afterward; `effortLevel`/`model` in settings were preserved throughout. Sanitized event capture: [spike2-hook-events.sample.jsonl](spike2-hook-events.sample.jsonl).

## Result — hooks DO fire in the VS Code graphical panel ✅

The load-bearing question is answered **yes**. Observed firing, live:

| Event | Fired? | Notes |
|---|---|---|
| `PreToolUse` | ✅ | Fires before each tool; carries `tool_name`, `tool_input`, `tool_use_id`, `effort` |
| `PostToolUse` | ✅ | Adds `tool_response`, `duration_ms` |
| `Stop` | ✅ | **The event GitHub #40029 reported broken in the extension — it fires.** Carries `last_assistant_message`, `stop_hook_active`, `background_tasks`, `session_crons`, `effort` |
| `UserPromptSubmit` | ✅ | Carries `prompt`; **no** `effort` field |
| `Notification[idle_prompt]` | ❌ not observed | Did not fire between `Stop` and the next prompt → **use `Stop` for the idle/done transition, not the idle notification** |
| `PermissionRequest` | ⏸ untested | Auto mode suppresses the interactive dialog these key off (see below) |
| `Notification[permission_prompt]` | ⏸ untested | Same — needs interactive permission mode |
| `SessionStart` / `SessionEnd` | ⏸ untested | Needs a session open/close within the instrumented window |

**Hooks hot-reload:** the hooks were added mid-session and fired without a restart — the settings watcher picks up `~/.claude/settings.json` edits live (the file existed at session start).

### Payload key schemas (as actually received in the panel)

- **PreToolUse:** `cwd, effort, hook_event_name, permission_mode, prompt_id, session_id, tool_input, tool_name, tool_use_id, transcript_path`
- **PostToolUse:** PreToolUse keys + `tool_response, duration_ms`
- **Stop:** `background_tasks, cwd, effort, hook_event_name, last_assistant_message, permission_mode, prompt_id, session_crons, session_id, stop_hook_active, transcript_path`
- **UserPromptSubmit:** `cwd, hook_event_name, permission_mode, prompt, prompt_id, session_id, transcript_path`

Two design-relevant facts:
- **`effort.level`** rides on the tool/stop events (value `xhigh` observed) → the widget's effort-color feature can read effort straight from the hook stream.
- The hook payload does **not** include `entrypoint` (VS Code vs terminal). To label a session's origin, join on `session_id` against `~/.claude/sessions/<pid>.json`, which does carry `entrypoint`.

## statusLine — NOT executed by the panel ✅ (confirms research)

With `statusLine` configured and `refreshInterval: 5`, the logger file stayed **empty** across a multi-second window in which hooks were demonstrably active. The graphical panel does not execute `statusLine`. → It is unusable as a panel data source; **hooks are the event plane, transcripts are the data plane.** (statusLine remains available only in terminal/`useTerminal` mode — relevant to the usage-gauge spike #3.)

## Session registry liveness

`~/.claude/sessions/<pid>.json` entries map cleanly to `/proc/<pid>`; both live sessions validated ALIVE, and this session's own shell parent pid matched its registry entry. Cleanup-on-clean-exit was **not** observed in this window (needs a session close); the widget must validate `/proc/<pid>` regardless, since sibling `ide/*.lock` files are known to go stale.

## The permission-events gap (important, not blocking)

`PermissionRequest` and `Notification[permission_prompt]` fire when an **interactive permission dialog** is shown. This session ran in **auto mode**, which replaces that dialog with an automatic classifier — so no dialog appears and these hooks cannot fire, regardless of panel vs terminal. This is a property of the *permission mode*, not the panel.

**Residual risk is low:** the panel demonstrably honors the full settings.json hook set (`PreToolUse`, `PostToolUse`, `Stop`, `UserPromptSubmit` all fired) over the identical delivery path these two events use. We observed no event *failing* — only events we could not *trigger* in auto mode. Therefore the spec's escalation clause ("if `Stop` **or** the permission events do **not** fire") is **not** triggered: `Stop` fires.

**Confirm before closing the alerts ticket (#6):** switch a session to default/interactive permission mode, trigger one tool that prompts, and verify `PermissionRequest` and `Notification[permission_prompt]` fire and carry `tool_name`/`tool_input`. Also note for the product: **the widget's "waiting for permission" state only exists when the user runs in an interactive permission mode** — under auto/bypass mode there is nothing to wait for.

## Bottom line

The widget's architecture is viable as specified. The daemon can drive working/idle from `PreToolUse`/`PostToolUse`/`Stop`, read model+tokens from transcripts, and read `effort` straight off the hook stream. Walking skeleton (#4) is unblocked. The alerts ticket (#6) carries a one-step interactive pre-flight for the permission events.
