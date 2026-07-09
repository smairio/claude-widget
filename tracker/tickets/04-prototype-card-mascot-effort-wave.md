---
id: 4
title: "Prototype the card: mascot, effort colors, wave"
labels: [wayfinder:prototype]
status: open
assignee:
blocked-by: []
parent: map
---

## Question

What does the card actually look like? Build a throwaway HTML mockup (~260×160, opens in a browser, no stack dependency) for Khalil to react to, iterating until it feels right. Must explore:

- **Mascot** — candidates to react to (what creature/shape? pixel-art vs vector?); it anchors the card and is the wave's origin.
- **Working animation** — purple wave pulsing out of the mascot: purple → transparent → purple again, playing the whole time Claude is working (decided at charting: the wave is the universal "working" signal, any session).
- **Waiting-for-permission** — a distinct, unmissable visual (this state is the widget's reason to exist).
- **Effort colors** — a color per effort level (low/medium/high/xhigh/max); at **max**, the card background becomes an animated field of rainbow pixels.
- Layout of model name, per-session tokens, usage gauge placement, and how 2–3 concurrent sessions stack.
- Animation CPU budget note: the widget runs 24/7 — animations must pause when idle/hidden.

The accepted mockup becomes the build spec for "Build the widget card & animations" and is linked here as an asset. Use the `/prototype` skill. HITL throughout.
