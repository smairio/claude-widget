---
id: 10
title: Wire notifications, tray, autostart — v1 acceptance
labels: [wayfinder:task]
status: open
assignee:
blocked-by: [7, 8, 9]
parent: map
---

## Question

Finish v1 and accept it with Khalil:

- Desktop notifications per the state-machine spec's rules (Claude finished a long turn, needs permission, usage threshold crossed) via native/`notify-send`.
- AppIndicator tray icon with right-click menu (quit, show/hide, settings entry point) — left-click events don't fire on Linux.
- Autostart: systemd user unit (`WantedBy=graphical-session.target`, `Restart=on-failure`) or XDG autostart; `XDG_SESSION_TYPE` check with a clear error on Wayland.
- **Acceptance walkthrough (HITL)**: widget stays above Chrome on both monitors and every workspace; a permission prompt raised while Khalil is in Chrome produces an unmissable alert within ~1s; model/token fields update live; wave and rainbow animations behave and pause at idle; survives logout/login; quit works from the tray.

Closing this ticket closes the map — destination reached.
