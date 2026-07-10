# Spike #3 findings — usage-limit data for a Team seat

**Date:** 2026-07-10 · **Account:** `claude_team` / `team_tier_1` / `default_claude_max_5x`, extra usage enabled (currently `out_of_credits`) · **Claude Code:** v2.1.205

## Answer: yes — via the official statusline channel

Usage-limit data **is** available for this Team seat, through the **statusline JSON `rate_limits` object** — the officially documented, ToS-clean channel. Confirmed two ways:

1. **User testimony:** Khalil sees the 5h/7d usage in VS Code and a Claude session; "it worked yesterday."
2. **Prior art:** [NadimJebali/Claude-Familiar](https://github.com/NadimJebali/Claude-Familiar) — a near-identical Claude Code mascot/status widget — ships exactly this. Its PRD (issue #44) states: *"The statusline input JSON is the only official programmatic source of subscription usage,"* and it works for this same account type.

**Decision — usage-gauge data source = rung ① (official statusline emitter).** No OAuth endpoint, no reading `~/.claude/.credentials.json` (which exists, 0600, keys `claudeAiOauth`/`organizationUuid` — left unread), no consumer-terms question. The unofficial `/api/oauth/usage` path stays documented in the spec as a future opt-in only; v1 does not use it.

### Statusline `rate_limits` schema (documented; confirmed shape)

```
"rate_limits": {                                                  // subscribers only, after first API response
  "five_hour": { "used_percentage": 0-100, "resets_at": <epoch seconds> },  // optional, independently absent
  "seven_day": { "used_percentage": 0-100, "resets_at": <epoch seconds> }   // optional
}
```

Sanitized fixtures: statusline stdin sample [spike3-statusline-input.sample.json](spike3-statusline-input.sample.json); widget account-global snapshot [spike3-usage-snapshot.sample.json](spike3-usage-snapshot.sample.json).

## The panel limitation and how prior art handles it

My spike #2 proved the statusline **never executes in the VS Code panel**. Claude-Familiar resolves this by making usage **account-global and last-known-from-any-session**: whichever session (terminal/CLI) last ran the statusline writes a single shared snapshot; every card — including panel-session cards — shows that last-known snapshot, decayed by `resets_at`. Implication for our #8: the gauge stays "roughly current" and refreshes whenever a terminal/`useTerminal` session runs; label it "as of <time>". This matches how Khalil already sees it (panel shows last value; a terminal session refreshes it).

## BOMBSHELL for #6 (waiting-for-permission): it's `AskUserQuestion`, not `Notification`

Claude-Familiar's **open bug #52** ("needs-you/waiting state never fires from VS Code sessions") found the root cause, confirmed by that maintainer:

> Under VS Code a "needs you" arrives as an **`AskUserQuestion` tool call (a `PreToolUse`)**, not a `Notification` hook. The state machine treated any `PreToolUse` as `working`, so `waiting` never fired.

Their fix (applies everywhere, not just VS Code):
- main-thread `PreToolUse` with `tool_name == "AskUserQuestion"` → **waiting** state, carrying the question text for the bubble/toast;
- the answering `PostToolUse` → resume to **working**;
- a nested `AskUserQuestion` inside a subagent (payload carries `agent_id`) stays **working** so it can't hijack the card.

This **resolves the permission-events gap I deferred from spike #2**: `PreToolUse` fires in the panel (proven), so the waiting state IS reachable there — via `AskUserQuestion`, not the interactive permission dialog. Directly actionable for our #6.

**Still open (inherit into #6):** the separate *tool-permission* prompt ("allow this Bash command?") may travel a different path than `AskUserQuestion`. Claude-Familiar hasn't confirmed it (needs a debug capture during a real permission prompt). Our #6 pre-flight should capture both an `AskUserQuestion` and a tool-allow prompt in interactive mode and confirm which hook each emits.

## Effort palette + animations (prior art for #7), exact RGBs from the CC 2.1.205 binary

From Claude-Familiar #44/#48 (values extracted from the binary's `effortOptions` + theme tables, so they match the CLI pixel-for-pixel):

| effort | role | dark-theme RGB | card treatment |
|---|---|---|---|
| low | warning | 255,193,7 | static amber tint (~18% blend) |
| medium | success | 78,186,101 | static green tint |
| high | permission | 177,185,249 | static periwinkle tint |
| xhigh (and ultracode) | autoAccept shimmer | 62,22,118 ↔ 140,80,240 | **purple wave**, sinusoidal ~2.4s period, ~30–35% blend |
| max | rainbow | 235,95,87 · 245,139,87 · 250,195,95 · 145,200,130 · 130,170,220 · 155,130,200 · 200,130,180 | **rainbow cycle**, ~6s period |

- **Effort source:** the `CLAUDE_EFFORT` env var is exposed to hook commands (and rides the hook payload as `effort.level`, per spike #2) — works in the panel.
- Usage-bar traffic-light thresholds reuse the theme: calm below 70%, warning amber (255,193,7) at 70–89%, error red (255,107,128) at ≥90% (the CLI's own 0.9 alarm threshold).
- **Note our variation:** Khalil wants the wave *emanating from the mascot* (a wave that grows out of the mascot: purple → transparent → purple), which is richer than Claude-Familiar's panel-fill wave. Keep these exact palette values; make the geometry mascot-centered in the #7 prototype.

## StopFailure field-name discrepancy to resolve (for #5/#6)

Claude-Familiar (#4) keys the rate-limited state on `error_type`; my spike #2 doc-verification found the v2.1.205 field is **`error`** (with `error_details`), not `error_type`. Claude-Familiar's #4 is still open pending a real-limit capture, so neither is confirmed against a live payload. Our state machine should match on `error` per current docs and capture a real `StopFailure` payload to settle it (same HITL "hit a real limit" step Claude-Familiar is still waiting on).

## Bottom line

Usage gauge (#8) is unblocked with a decided, official, ToS-clean source. The biggest downstream risk — waiting-state detection in the panel (#6, and my spike #2 deferral) — now has a proven mechanism (`AskUserQuestion` PreToolUse). Prior art [Claude-Familiar](https://github.com/NadimJebali/Claude-Familiar) is a Python/Tk→Qt implementation of essentially this same product and is worth mining per ticket.
