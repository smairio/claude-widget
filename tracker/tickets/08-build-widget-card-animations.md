---
id: 8
title: Build the widget card & animations
labels: [wayfinder:task]
status: open
assignee:
blocked-by: [3, 4]
parent: map
---

## Question

Implement the accepted mockup from "Prototype the card: mascot, effort colors, wave" in the chosen stack:

- Frameless always-on-top sticky window: `alwaysOnTop`, `visibleOnAllWorkspaces`, `skipTaskbar`, decorations off, **opaque** (no `transparent: true` — NVIDIA/webkitgtk bug; keep `WEBKIT_DISABLE_DMABUF_RENDERER=1` as the known fix if the window renders blank).
- Mascot + purple wave animation while working; distinct waiting-for-permission visual; effort-level colors with the rainbow-pixel animated background at max effort; animations pause when idle/hidden (24/7 CPU budget).
- Live binding of model name, tokens, per-session rows to daemon events — every field updates within ~1s (socket-like), including mid-session model changes.
- Drag to reposition across monitors (X11 single coordinate space).

Verify with `/verify` against a real working session.
