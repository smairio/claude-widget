---
id: 1
title: "Spike: which Claude Code signals fire on this machine"
labels: [wayfinder:task]
status: open
assignee:
blocked-by: []
parent: map
---

## Question

On Claude Code v2.1.205 via the VS Code panel on this machine, empirically settle the four data-plane unknowns the research could not resolve from docs alone:

1. Do `~/.claude/settings.json` hooks fire for **panel** sessions — specifically `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `Notification`, `Stop`, `SessionEnd`? (Docs say yes via `settingSources:["user",...]`; GitHub issues #40029/#18547 report gaps, especially `Stop`.)
2. Does a configured `statusLine.command` ever execute for panel sessions (test with a `tee` logger + `refreshInterval: 5`, then again with `claudeCode.useTerminal: true`)? Expectation from research: never in the graphical panel.
3. Is `~/.claude/sessions/<pid>.json` removed on clean exit, and does it go stale on crash? (Sibling `ide/*.lock` files demonstrably go stale.)
4. While a permission prompt is pending, what — if anything — appears in the session's transcript JSONL? Can "waiting for permission" be inferred without hooks at all?

Method: idempotently merge temporary logging hooks into `~/.claude/settings.json` (must preserve the existing `effortLevel`/`model` keys), restart the panel session, trigger each event including a permission prompt, inspect the log, then revert. HITL: Khalil restarts the panel and answers the permission prompt.

Unblocks the state-machine spec and the usage & cost source decision. Ground truth: [research digest](../assets/research-digest-2026-07-10.md).
