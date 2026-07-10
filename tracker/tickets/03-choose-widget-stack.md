---
id: 3
title: Choose the widget stack
labels: [wayfinder:grilling]
status: closed
assignee: khalil
blocked-by: []
parent: map
---

## Resolution

Resolved 2026-07-10 (decided with the user, implemented in #4 / commit `25cf90e`).

**Stack = Rust + `eframe`/`egui`.** Single process: the eframe app hosts the loopback hook listener, the (future) file watcher, and the UI. Chosen over the map's earlier Tauri lean once the user made **portability-for-others** a first-class goal: egui compiles to a **single self-contained native binary** with no webview dependency, whereas Tauri's Linux AppImage still leans on the host's WebKitGTK (its weakest point for distribution). egui also wins on performance and is the most natural fit for the custom per-frame animations (mascot wave, rainbow-pixel field). GTK3/PySide6 (Python) were the fallbacks; Electron is out (needs Node ≥22.12, machine has 18).

Window management (the main risk of the winit path) validated live: always-on-top + sticky set via `x11rb` EWMH `_NET_WM_STATE_ABOVE`/`_STICKY`, confirmed with `xprop`. Idle animation-pause and CPU budget carry into #7.

## Question

Which stack builds the widget, and does the data daemon live inside the same process?

- **Tauri v2** (research recommendation): ~30–60MB idle RAM, all window flags supported on X11, webkit runtime libs already installed; costs a one-time Rust toolchain install; caveats — no `transparent: true` on this NVIDIA machine, tray left-click events not emitted on Linux (right-click menu only).
- **GTK3/PyGObject**: zero new toolchain (verified working locally), ~30–50MB, but Python and hand-rolled UI; animations (rainbow pixels, waves) are more work in cairo than CSS/canvas.
- **Electron**: familiar, but current Electron requires Node ≥ 22.12 and the machine has 18.19.1 (verified), so it needs a Node upgrade and costs ~200MB+ resident for a 24/7 widget.

Also weigh: the v1 animations (constant wave while working, rainbow-pixel background at max effort) — their CPU cost on webkitgtk vs alternatives, and pausing animation when idle. Decision also fixes the daemon shape: single process (Rust core inside the Tauri app: file watcher + localhost hook listener) vs a separate daemon the UI connects to.
