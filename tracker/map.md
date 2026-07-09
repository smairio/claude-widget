---
title: Claude Code status widget — v1 map
labels: [wayfinder:map]
---

## Destination

Working widget v1 on Khalil's desktop: an always-on-top X11 card, visible over Chrome on any monitor and workspace, showing live Claude Code state — a mascot pulsing purple waves while Claude works, a distinct waiting-for-permission alert, current model, session tokens, effort level rendered as colors (rainbow-pixel animated background at max effort), and a best-effort usage-limits gauge — plus desktop notifications, a tray icon with menu, and autostart on login. Socket-like freshness: every field reflects a change within ~1 second.

## Notes

- **This map carries execution** (override of the plan-only default): build tickets are in scope through v1 acceptance on this machine.
- Tracker: local markdown — operations in [README.md](README.md).
- Skills per ticket type: `/grilling` + `/domain-modeling` for decision tickets, `/prototype` for the UI ticket, `/verify` after build tickets, `/research` for any new external facts.
- Standing preferences: prefer officially documented data sources; the unofficial `/api/oauth/usage` endpoint only as an explicit opt-in, and **never call the token-refresh endpoint** (rotating the refresh token can log Claude Code out); X11-only — detect `XDG_SESSION_TYPE` and fail loudly; avoid `transparent: true` windows on this NVIDIA machine (documented webkitgtk crash/blank bug; fallback env `WEBKIT_DISABLE_DMABUF_RENDERER=1`).
- Ground truth: [research digest 2026-07-10](assets/research-digest-2026-07-10.md) — 80 verified facts from local inspection, official docs, and OSS monitor source code. Machine/account context also in Claude memory (`claude-widget-project`, `khalil-machine`).
- Field-name traps (verified): it is `used_percentage` not `used_percent`; StopFailure stdin fields are `error`/`error_details` not `error_type`/`error_message`; OAuth `resets_at` is an ISO string while statusline `resets_at` is epoch seconds; transcripts contain **no** `costUSD` field.

## Decisions so far

<!-- one line per closed ticket: gist + link -->

## Not yet specified

- Card interactions — what clicking and dragging do (open the session's VS Code window via `vscode://anthropic.claude-code/open`? settings surface? position memory per monitor?). Sharpens after the card prototype.
- Notification rules detail — exact events, usage thresholds (e.g. >80%), quiet behavior, sound. Sharpens after the state-machine spec.
- Pricing-table upkeep for new model ids (fable family) and what "cost" means for a Team seat. Sharpens after the usage & cost source decision.
- Resilience states — what the card shows when the daemon dies, when data is stale, when zero sessions are live. Sharpens during daemon/card builds.

## Out of scope

- **Wayland support** — v1 is X11-only; a session switch needs a different windowing approach (layer-shell/GNOME extension) and would be a fresh effort.
- **Distribution to others** — no public packaging, README-for-strangers, or cross-distro testing; installs on this machine only.
- **OTel/Prometheus pipeline** — heavier alternative data plane; hooks + transcripts cover v1.
- **Multi-machine aggregation** — this PC's sessions only.
