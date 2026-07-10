//! The eframe UI: an opaque, frameless status card. Skeleton scope = working/idle.

use std::io::Write;
use std::sync::mpsc::Receiver;
use std::time::{SystemTime, UNIX_EPOCH};

use widget_core::{AggregateState, ParsedEvent, Roster};

/// A "working" session with no activity for this long is assumed idle (the stall
/// backstop). Generous so a single long thinking pass is not falsely idled; it exists to
/// recover from user interrupts (no `Stop`) and dropped events, not to be the primary signal.
const STALL_TIMEOUT_MS: u64 = 45_000;

/// How often to re-read the session registry (enumerate new sessions, drop gone ones).
const REGISTRY_POLL_MS: u64 = 2_000;

/// Only notify "Claude finished" for turns that ran at least this long, so quick turns
/// don't spam. (Needs-you / rate-limit notifications always fire — they're actionable.)
const NOTIFY_LONG_TURN_MS: u64 = 20_000;

/// A usage snapshot older than this is shown as stale. The statusline refreshes it every
/// few seconds while any terminal session is active, so minutes of silence means the
/// source is quiet (e.g. only panel sessions, which never run the statusline).
const USAGE_STALE_SECS: u64 = 15 * 60;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct WidgetApp {
    roster: Roster,
    rx: Receiver<ParsedEvent>,
    debug: bool,
    stall_ms: u64,
    last_agg: Option<AggregateState>,
    last_registry_ms: u64,
    last_count: usize,
    /// When the aggregate last entered Working (for the long-turn-finished notification).
    working_since: Option<u64>,
    /// Notifications on unless CW_NO_NOTIFY is set.
    notify_enabled: bool,
    /// Minimum turn length to notify "finished" (default 20s; override CW_NOTIFY_MS).
    notify_ms: u64,
    transcript: crate::transcript::TranscriptReader,
    /// Last-known account-global usage snapshot (written by the statusline emitter).
    usage: Option<widget_core::UsageSnapshot>,
    /// Snapshot file mtime at the last read, so unchanged files are not re-parsed.
    usage_mtime: Option<std::time::SystemTime>,
    usage_stale_secs: u64,
}

impl WidgetApp {
    pub fn new(rx: Receiver<ParsedEvent>) -> Self {
        Self {
            roster: Roster::new(),
            rx,
            debug: std::env::var_os("CW_DEBUG").is_some(),
            // Tunable backstop window (default 45s); override with CW_STALL_MS.
            stall_ms: std::env::var("CW_STALL_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(STALL_TIMEOUT_MS),
            last_agg: None,
            last_registry_ms: 0,
            last_count: 0,
            working_since: None,
            notify_enabled: std::env::var_os("CW_NO_NOTIFY").is_none(),
            notify_ms: std::env::var("CW_NOTIFY_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(NOTIFY_LONG_TURN_MS),
            transcript: crate::transcript::TranscriptReader::new(),
            usage: None,
            usage_mtime: None,
            usage_stale_secs: std::env::var("CW_USAGE_STALE_MS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map(|ms| (ms / 1000).max(1))
                .unwrap_or(USAGE_STALE_SECS),
        }
    }

    /// Re-read the shared usage snapshot if its file changed. A missing or unparseable
    /// file keeps the last-known snapshot (staleness will surface it honestly).
    fn poll_usage(&mut self) {
        let path = crate::emitter::snapshot_path();
        let Ok(meta) = std::fs::metadata(&path) else { return };
        let mtime = meta.modified().ok();
        // Skip only on a *known-equal* mtime; if mtime is unreadable, keep re-reading
        // rather than wedging on None == None forever.
        if self.usage.is_some() && mtime.is_some() && mtime == self.usage_mtime {
            return;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else { return };
        if let Some(snap) = widget_core::parse_usage_snapshot(&contents) {
            self.dbg(&format!(
                "usage snapshot: 5h={:?} 7d={:?} written_at={}",
                snap.five_hour.as_ref().map(|w| w.used_percentage),
                snap.seven_day.as_ref().map(|w| w.used_percentage),
                snap.written_at
            ));
            self.usage = Some(snap);
            self.usage_mtime = mtime;
        }
    }

    /// Fire a desktop notification (and log it under CW_DEBUG). Never blocks or panics.
    fn notify(&self, summary: &str, body: &str) {
        self.dbg(&format!("notify: {summary} — {body}"));
        if !self.notify_enabled {
            return;
        }
        // Fire on a background thread — show() is a synchronous DBus round-trip and must
        // not stall the render thread.
        let (summary, body) = (summary.to_string(), body.to_string());
        std::thread::spawn(move || {
            let _ = notify_rust::Notification::new()
                .summary(&summary)
                .body(&body)
                .appname("Claude Widget")
                .show();
        });
    }

    /// When CW_DEBUG is set, append a timestamped line to ~/.claude/claude-widget-debug.log.
    fn dbg(&self, msg: &str) {
        if !self.debug {
            return;
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let path = std::path::Path::new(&home).join(".claude").join("claude-widget-debug.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{} {msg}", now_ms());
        }
    }
}

impl WidgetApp {
    /// Fire notifications and maintain the turn timer on an aggregate-state change.
    /// Called only when `to != from`, so entering a state notifies exactly once.
    fn handle_transition(&mut self, from: Option<AggregateState>, to: AggregateState, now: u64, saw_stop: bool) {
        use AggregateState::*;
        match to {
            WaitingForInput => self.notify("Claude needs you", "A session is waiting for your input."),
            RateLimited => self.notify("Claude — rate limit reached", "The turn stopped on a rate limit."),
            Idle | NoSessions => {
                if from == Some(Working) && saw_stop {
                    if let Some(ws) = self.working_since {
                        if now.saturating_sub(ws) >= self.notify_ms {
                            self.notify("Claude finished", "A long turn just completed.");
                        }
                    }
                }
                self.working_since = None;
            }
            Working => {}
        }
        // Start the turn timer when entering Working (spans a mid-turn wait/resume).
        if to == Working && self.working_since.is_none() {
            self.working_since = Some(now);
        }
    }
}

/// Trim the redundant "claude-" prefix for display (claude-opus-4-8 -> opus-4-8).
fn short_model(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
}

/// Compact token count: 512, 34k, 1.2M.
fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Traffic-light color for a gauge tier (RGBs from the Claude Code theme; see spike #3).
fn gauge_color(tier: widget_core::GaugeTier) -> egui::Color32 {
    match tier {
        widget_core::GaugeTier::Calm => egui::Color32::from_rgb(78, 186, 101),
        widget_core::GaugeTier::Warn => egui::Color32::from_rgb(255, 193, 7),
        widget_core::GaugeTier::Alert => egui::Color32::from_rgb(255, 107, 128),
    }
}

fn presentation(state: AggregateState) -> (&'static str, egui::Color32) {
    match state {
        AggregateState::NoSessions => ("no sessions", egui::Color32::from_gray(95)),
        AggregateState::Idle => ("idle", egui::Color32::from_gray(160)),
        AggregateState::Working => ("working", egui::Color32::from_rgb(80, 140, 240)),
        AggregateState::WaitingForInput => ("needs you", egui::Color32::from_rgb(240, 95, 90)),
        AggregateState::RateLimited => ("rate limited", egui::Color32::from_rgb(240, 95, 90)),
    }
}

impl eframe::App for WidgetApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = now_ms();

        // Drain any hook events delivered since the last frame.
        let mut drained = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            drained.push(ev);
        }
        for ev in &drained {
            self.dbg(&format!("recv session={} event={:?}", ev.session_id, ev.event));
            self.roster.apply_at(ev, now);
        }
        // "Finished" should mean a real turn end, not an interrupt (backstop) or a session
        // disappearing — so only when an actual Stop arrived this frame.
        let saw_stop = drained
            .iter()
            .any(|e| matches!(e.event, widget_core::HookEvent::Stop));

        // Registry sync (throttled): the session registry is the authority for WHICH
        // sessions exist. Enumerate live ones (so the card is populated at startup, not
        // empty) and drop any whose process is gone. Never wipe on a transient read error.
        if now.saturating_sub(self.last_registry_ms) >= REGISTRY_POLL_MS {
            self.last_registry_ms = now;
            self.poll_usage();
            if let Some(live) = crate::registry::live_sessions() {
                let ids: std::collections::BTreeSet<String> =
                    live.iter().map(|s| s.session_id.clone()).collect();
                for s in &live {
                    self.roster.ensure_session(&s.session_id, s.cwd.as_deref(), now);
                }
                self.roster.retain_live(&ids);
                // Tail each live session's transcript for model + accumulated tokens.
                let sessions: Vec<(String, Option<String>)> =
                    live.into_iter().map(|s| (s.session_id, s.cwd)).collect();
                for update in self.transcript.poll(&sessions) {
                    self.roster.apply_transcript(&update);
                }
                if self.debug {
                    for v in self.roster.sessions_view() {
                        let id: String = v.session_id.chars().take(8).collect();
                        self.dbg(&format!(
                            "view {id} project={} model={} tokens={}",
                            v.project,
                            v.model.as_deref().unwrap_or("-"),
                            v.tokens
                        ));
                    }
                }
            }
            if self.roster.len() != self.last_count {
                self.dbg(&format!("registry sync: sessions={}", self.roster.len()));
                self.last_count = self.roster.len();
            }
        }

        // Stall backstop: recover a "working" session that never received a terminal
        // event (user interrupt fires no Stop; an error fires StopFailure).
        self.roster.expire_stale(now, self.stall_ms);

        // React to aggregate transitions (from events OR the backstop): log, notify, and
        // track the turn timer.
        let agg = self.roster.aggregate();
        if Some(agg) != self.last_agg {
            self.dbg(&format!("-> aggregate={agg:?} sessions={}", self.roster.len()));
            self.handle_transition(self.last_agg, agg, now, saw_stop);
            self.last_agg = Some(agg);
        }

        // Periodic ticks are driven by the heartbeat thread (see main.rs), which reliably
        // wakes the loop even while idle/unfocused; nothing to schedule here.

        let (label, accent) = presentation(agg);
        let bg = egui::Color32::from_rgb(18, 18, 22);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(bg).inner_margin(16.0))
            .show(ctx, |ui| {
                // Whole-card drag: hand the move to the window manager exactly ONCE, when a
                // drag begins. Using `drag_started()` (not `dragged()`) means a plain click
                // does nothing and the window doesn't jitter during the move.
                let drag = ui.interact(
                    ui.max_rect(),
                    egui::Id::new("card-drag"),
                    egui::Sense::click_and_drag(),
                );
                if drag.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                ui.label(
                    egui::RichText::new("Claude")
                        .size(14.0)
                        .color(egui::Color32::from_gray(210)),
                );
                ui.add_space(8.0);
                ui.label(egui::RichText::new(format!("● {label}")).size(24.0).color(accent));
                ui.add_space(8.0);

                // Per-session rows: project · model · tokens.
                let rows = self.roster.sessions_view();
                if rows.is_empty() {
                    ui.label(
                        egui::RichText::new("no active sessions")
                            .size(12.0)
                            .color(egui::Color32::from_gray(120)),
                    );
                }
                for v in rows.iter().take(3) {
                    let model = v.model.as_deref().map(short_model).unwrap_or("—");
                    ui.label(
                        egui::RichText::new(format!("{} · {} · {}", v.project, model, fmt_tokens(v.tokens)))
                            .size(12.0)
                            .color(egui::Color32::from_gray(150)),
                    );
                }
                if rows.len() > 3 {
                    ui.label(
                        egui::RichText::new(format!("+{} more", rows.len() - 3))
                            .size(11.0)
                            .color(egui::Color32::from_gray(110)),
                    );
                }

                // Usage-limit gauge: account-global 5h/7d bars from the statusline
                // snapshot. No snapshot (or no windows) -> section simply absent, the
                // card degrades to the tokens-only rows above.
                if let Some(snap) = &self.usage {
                    let g = widget_core::gauge_view(snap, now / 1000, self.usage_stale_secs);
                    if !g.windows.is_empty() {
                        ui.add_space(10.0);
                        for w in &g.windows {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(w.label)
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(140)),
                                );
                                // Track + fill; stale data draws dimmed, never as current.
                                let width = (ui.available_width() - 88.0).max(30.0);
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(width, 7.0),
                                    egui::Sense::hover(),
                                );
                                let mut fill_color = gauge_color(w.tier);
                                if g.stale {
                                    fill_color = fill_color.gamma_multiply(0.45);
                                }
                                let painter = ui.painter();
                                painter.rect_filled(rect, 3.5, egui::Color32::from_gray(45));
                                if w.pct > 0.0 {
                                    let mut fill = rect;
                                    fill.set_width(rect.width() * (w.pct as f32 / 100.0));
                                    painter.rect_filled(fill, 3.5, fill_color);
                                }
                                let right = match w.resets_in_secs {
                                    Some(s) => format!(
                                        "{}% · {}",
                                        w.pct.round() as i64,
                                        widget_core::fmt_countdown(s)
                                    ),
                                    None => format!("{}%", w.pct.round() as i64),
                                };
                                ui.label(
                                    egui::RichText::new(right)
                                        .size(10.0)
                                        .color(egui::Color32::from_gray(140)),
                                );
                            });
                        }
                        // Freshness, always: the snapshot is last-known-from-any-terminal-
                        // session (the statusline never runs in the VS Code panel).
                        let age = (now / 1000).saturating_sub(g.as_of_secs);
                        let mut as_of = format!("as of {} ago", widget_core::fmt_countdown(age));
                        if g.stale {
                            as_of.push_str(" · stale");
                        }
                        ui.label(
                            egui::RichText::new(as_of)
                                .size(10.0)
                                .color(egui::Color32::from_gray(if g.stale { 130 } else { 105 })),
                        );
                    }
                }
            });
    }
}
