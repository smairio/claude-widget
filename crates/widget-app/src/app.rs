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
            if let Some(live) = crate::registry::live_session_ids() {
                for id in &live {
                    self.roster.ensure_idle(id, now);
                }
                self.roster.retain_live(&live);
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
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("{} session(s)", self.roster.len()))
                        .size(12.0)
                        .color(egui::Color32::from_gray(140)),
                );
            });
    }
}
