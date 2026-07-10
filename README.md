# claude-widget

An always-on-top desktop card for Linux/X11 showing live Claude Code status: which
sessions are running, working/idle/needs-you, model and context size per session,
usage limits, effort-colored visuals, and desktop notifications. Single self-contained
binary (Rust + egui), fed entirely by Claude Code's own hooks and files under `~/.claude/`.

## Install & run

```
cargo build --release
./target/release/claude-widget install   # merges hooks + statusline into ~/.claude/settings.json
./target/release/claude-widget           # run the card (X11 session required)
```

`claude-widget uninstall` removes exactly what install added and nothing else.

## Behavior notes (read before filing a bug)

- **Effort color updates at turn boundaries.** The effort level travels inside Claude
  Code's hook payloads, which report the effort actually used by the model. Change the
  effort selector and the card recolors when Claude next reports it — at the end of the
  current turn in the worst case. The widget paints the new value the instant it arrives;
  the timing is upstream semantics, not widget lag.
- **The purple wave is the top tier's signature.** Rings radiate from the spark only
  while a session is working at **xhigh** effort (ultracode included — Claude Code
  reports ultracode as xhigh). Working at other efforts shows the rotating spark only.
- **Usage bars (5h/7d) have two sources.** The default is the statusline, which Claude
  Code runs only in terminal sessions — never in the VS Code panel — so panel-only
  workflows see the last terminal snapshot, aging into an explicit "stale" label.
  For live numbers without terminal sessions, opt in to the usage API:
  `claude-widget usage-api on` (persisted; revoke with `off` — a running widget picks
  up either change within seconds, no restart needed).
  When enabled, the widget reads the OAuth token Claude Code already stores locally,
  polls the account usage endpoint every 300s (backing off on rate limits), and
  **never calls any token-refresh endpoint or logs the token**. Both sources write the
  same snapshot; the freshest wins.
- **Dimmed session rows** are sessions that exist (per Claude Code's registry) but have
  never fired a hook or produced transcript data — e.g. the spare panel session VS Code
  keeps around. They brighten the moment they do anything.

## Test knobs

`CW_DEBUG=1` (log to `~/.claude/claude-widget-debug.log`), `CW_STALL_MS`, `CW_NOTIFY_MS`,
`CW_NO_NOTIFY`, `CW_USAGE_STALE_MS`, `CW_PORT` (hermetic test instance).
