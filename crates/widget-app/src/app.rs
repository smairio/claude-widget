//! The eframe UI: an opaque, frameless status card. Skeleton scope = working/idle.

use std::io::Write;
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use widget_core::{AggregateState, ParsedEvent, Roster};

/// A "working" session with no activity for this long is assumed idle (the stall
/// backstop). Generous so a single long thinking pass is not falsely idled; it exists to
/// recover from user interrupts (no `Stop`) and dropped events, not to be the primary signal.
const STALL_TIMEOUT_MS: u64 = 45_000;

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
        }
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

        // Stall backstop: recover a "working" session that never received a terminal
        // event (user interrupt fires no Stop; an error fires StopFailure).
        self.roster.expire_stale(now, self.stall_ms);

        // Log aggregate transitions from ANY cause (events or the backstop).
        let agg = self.roster.aggregate();
        if Some(agg) != self.last_agg {
            self.dbg(&format!("-> aggregate={agg:?} sessions={}", self.roster.len()));
            self.last_agg = Some(agg);
        }

        // Keep waking (~1s) while anything is working, so the backstop fires without
        // needing a new hook event. When idle, no repaint is scheduled (cheap).
        if self.roster.any_working() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

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
