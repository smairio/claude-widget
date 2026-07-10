---
id: 2
title: "Spike: does rate-limit data exist for a Team seat"
labels: [wayfinder:task]
status: closed
assignee: khalil
blocked-by: []
parent: map
---

## Resolution

Resolved 2026-07-10 via GitHub issue #3. Full write-up: [spike3-findings.md](../assets/spike3-findings.md).

- **Yes — usage data exists for this Team seat**, via the official statusline `rate_limits` JSON (`five_hour`/`seven_day`, each `used_percentage` + `resets_at`). Confirmed by user testimony ("worked yesterday", visible in VS Code) and by prior art [NadimJebali/Claude-Familiar](https://github.com/NadimJebali/Claude-Familiar), a near-identical widget shipping this for the same account type.
- **Decision: usage source = official statusline emitter** (ladder rung ①); account-global, last-known-from-any-session snapshot to cover panel sessions (statusline doesn't run in the panel). The unofficial OAuth endpoint was NOT used and NOT needed; credentials left unread. Resolves the usage-source half of ticket 05 (cost half still open).
- Cross-cutting find carried to #6: the VS Code **waiting** signal is a `PreToolUse(AskUserQuestion)`, not a `Notification` — the real mechanism for permission/waiting detection (see findings doc + Claude-Familiar #52).
- Fixtures: [statusline input](../assets/spike3-statusline-input.sample.json), [usage snapshot](../assets/spike3-usage-snapshot.sample.json).

## Question

Khalil's account is a Claude **Team** org seat (`organizationType: claude_team`, `userRateLimitTier: default_claude_max_5x`), but the docs scope the statusline `rate_limits` field to Pro/Max subscribers. Does usage-limit data actually exist for this seat, on either channel?

1. **Official channel**: run one terminal session using the bundled binary (`~/.vscode/extensions/anthropic.claude-code-2.1.205-linux-x64/resources/native-binary/claude`) with a `statusLine.command` that tees stdin to a file. After the first API response, does the JSON contain the `rate_limits` key (`five_hour`/`seven_day` with `used_percentage`, `resets_at`)?
2. **Unofficial channel** (only with Khalil's explicit go-ahead — ToS caveat): a single `GET https://api.anthropic.com/api/oauth/usage` with `Authorization: Bearer <claudeAiOauth.accessToken from ~/.claude/.credentials.json>` and `anthropic-beta: oauth-2025-04-20`. Does it return 200 with utilization buckets for this seat? **Never call the token-refresh endpoint.**

Record exact observed payloads (redacting tokens). Unblocks the usage & cost data source decision. Ground truth: [research digest](../assets/research-digest-2026-07-10.md).
