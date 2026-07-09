---
id: 3
title: Choose the widget stack
labels: [wayfinder:grilling]
status: open
assignee:
blocked-by: []
parent: map
---

## Question

Which stack builds the widget, and does the data daemon live inside the same process?

- **Tauri v2** (research recommendation): ~30–60MB idle RAM, all window flags supported on X11, webkit runtime libs already installed; costs a one-time Rust toolchain install; caveats — no `transparent: true` on this NVIDIA machine, tray left-click events not emitted on Linux (right-click menu only).
- **GTK3/PyGObject**: zero new toolchain (verified working locally), ~30–50MB, but Python and hand-rolled UI; animations (rainbow pixels, waves) are more work in cairo than CSS/canvas.
- **Electron**: familiar, but current Electron requires Node ≥ 22.12 and the machine has 18.19.1 (verified), so it needs a Node upgrade and costs ~200MB+ resident for a 24/7 widget.

Also weigh: the v1 animations (constant wave while working, rainbow-pixel background at max effort) — their CPU cost on webkitgtk vs alternatives, and pausing animation when idle. Decision also fixes the daemon shape: single process (Rust core inside the Tauri app: file watcher + localhost hook listener) vs a separate daemon the UI connects to.
