---
id: 1
title: "Spike: which Claude Code signals fire on this machine"
labels: [wayfinder:task]
status: closed
assignee: khalil
blocked-by: []
parent: map
---

## Resolution

Resolved 2026-07-10 via GitHub issue #2. Full write-up: [spike2-findings.md](../assets/spike2-findings.md); sanitized event capture: [spike2-hook-events.sample.jsonl](../assets/spike2-hook-events.sample.jsonl).

- **Hooks fire in the VS Code panel — yes** (the load-bearing answer). `PreToolUse`, `PostToolUse`, `Stop`, `UserPromptSubmit` all observed firing live in a `claude-vscode` session on v2.1.205; `Stop` (the GitHub #40029 concern) works. Hooks hot-reload on mid-session settings edits.
- **statusLine is NOT executed by the panel** (empty even with `refreshInterval: 5`) → hooks are the event plane, transcripts the data plane; statusLine only in terminal/`useTerminal` mode.
- **Idle transition** should key off `Stop`, not `Notification[idle_prompt]` (the latter never fired).
- **Registry** `pid` validates against `/proc/<pid>`; clean-exit deletion not observed in-window, so always validate liveness.
- **Permission-dialog events** (`PermissionRequest`, `Notification[permission_prompt]`) could not be triggered under auto mode (it suppresses the interactive dialog) — deferred to the alerts ticket (#6) with a one-step interactive pre-flight. Spec escalation NOT triggered (`Stop` fires). `effort.level` rides the hook stream; `entrypoint` does not (join to the session registry for it).

## Question

On Claude Code v2.1.205 via the VS Code panel on this machine, empirically settle the four data-plane unknowns the research could not resolve from docs alone:

1. Do `~/.claude/settings.json` hooks fire for **panel** sessions — specifically `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `Notification`, `Stop`, `SessionEnd`? (Docs say yes via `settingSources:["user",...]`; GitHub issues #40029/#18547 report gaps, especially `Stop`.)
2. Does a configured `statusLine.command` ever execute for panel sessions (test with a `tee` logger + `refreshInterval: 5`, then again with `claudeCode.useTerminal: true`)? Expectation from research: never in the graphical panel.
3. Is `~/.claude/sessions/<pid>.json` removed on clean exit, and does it go stale on crash? (Sibling `ide/*.lock` files demonstrably go stale.)
4. While a permission prompt is pending, what — if anything — appears in the session's transcript JSONL? Can "waiting for permission" be inferred without hooks at all?

Method: idempotently merge temporary logging hooks into `~/.claude/settings.json` (must preserve the existing `effortLevel`/`model` keys), restart the panel session, trigger each event including a permission prompt, inspect the log, then revert. HITL: Khalil restarts the panel and answers the permission prompt.

Unblocks the state-machine spec and the usage & cost source decision. Ground truth: [research digest](../assets/research-digest-2026-07-10.md).
