---
id: 5
title: Decide the usage & cost data source
labels: [wayfinder:grilling]
status: open
assignee:
blocked-by: [1, 2]
parent: map
---

## Question

Given the spike results ("Spike: which Claude Code signals fire on this machine", "Spike: does rate-limit data exist for a Team seat"), which source feeds the usage-limits gauge, and does the widget show dollar cost?

Usage gauge options, in preference order (standing preference: official first):
1. **Statusline tee** — official, zero API calls; but stale between Claude Code turns and (per research) never fires from the graphical panel, so it may require occasional terminal-mode sessions.
2. **Opt-in OAuth polling** — `GET /api/oauth/usage`, fresh and covers claude.ai chat usage too; unofficial, ToS caveat, poll ≥180s with ≥300s backoff on 429, never refresh the token.
3. **Local approximation** — replicate `/usage`-style computation from transcript JSONL (what ccusage does); approximate, this-machine-only.
4. **Tokens-only** — cut the gauge if nothing works for a Team seat.

Cost display: pinned pricing table (fable ids may be missing from LiteLLM's dataset — verified `additionalModelCostsCache` is empty and transcripts carry no `costUSD`) vs tokens-only, and what "cost" even means on a Team seat. Output: the locked decision plus fallback ladder the build implements.
