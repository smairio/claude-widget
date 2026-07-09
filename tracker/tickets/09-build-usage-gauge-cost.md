---
id: 9
title: Build the usage gauge & cost display
labels: [wayfinder:task]
status: open
assignee:
blocked-by: [5, 7]
parent: map
---

## Question

Implement the usage-limits gauge and cost display per the locked decision from "Decide the usage & cost data source", on top of the running daemon:

- The chosen source with its full fallback ladder (e.g. statusline state file → opt-in OAuth poller with 180s cadence + 429 backoff → tokens-only degradation), handling the verified format traps: `used_percentage` field name, OAuth `resets_at` as ISO string vs statusline epoch seconds, independently-absent windows, nullable buckets.
- Reset-time countdowns for the 5-hour and weekly windows; "as of <time>" staleness label if the source goes quiet.
- Cost per the decision (pinned pricing table with tokens-only fallback for unknown model ids, or no $ at all).

Verify with `/verify` against real usage data.
