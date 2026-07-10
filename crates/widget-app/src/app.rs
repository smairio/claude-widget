//! The eframe UI: an opaque, frameless status card. Skeleton scope = working/idle.

use std::io::Write;
use std::sync::mpsc::Receiver;
use std::time::{SystemTime, UNIX_EPOCH};

use widget_core::{AggregateState, Backdrop, EffortLevel, ParsedEvent, Roster};

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
    /// Last effort level rendered (for change logging only).
    last_effort: Option<EffortLevel>,
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
            last_effort: None,
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
/// Calm/Warn reuse the effort palette (same theme colors); Alert is the CLI's error red.
fn gauge_color(tier: widget_core::GaugeTier) -> egui::Color32 {
    match tier {
        widget_core::GaugeTier::Calm => rgb(widget_core::EFFORT_GREEN),
        widget_core::GaugeTier::Warn => rgb(widget_core::EFFORT_AMBER),
        widget_core::GaugeTier::Alert => egui::Color32::from_rgb(255, 107, 128),
    }
}

fn rgb(c: widget_core::Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(c.0, c.1, c.2)
}

/// Blend a tint over a base color at `t` (the core's lerp keeps the math in one place).
fn mix(base: egui::Color32, tint: widget_core::Rgb, t: f32) -> egui::Color32 {
    rgb(widget_core::lerp_rgb((base.r(), base.g(), base.b()), tint, t))
}

/// The spark mascot: an 8-armed starburst (long cardinal arms, short diagonals) with a
/// filled core. `angle` rotates it; everything else is state expressed as color/size.
fn draw_spark(painter: &egui::Painter, center: egui::Pos2, r: f32, angle: f32, color: egui::Color32) {
    for i in 0..8 {
        let a = angle + i as f32 * std::f32::consts::FRAC_PI_4;
        let (len, width) = if i % 2 == 0 { (r, 3.4) } else { (r * 0.55, 2.4) };
        let tip = center + egui::vec2(a.cos(), a.sin()) * len;
        painter.line_segment([center, tip], egui::Stroke::new(width, color));
    }
    painter.circle_filled(center, r * 0.24, color);
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

        // Effort drives the card's color scheme. Live per-session hooks win; the
        // account-global statusline snapshot is a fallback only while it is FRESH —
        // a stale snapshot must not tint the card indefinitely.
        let effort = self.roster.current_effort().or_else(|| {
            let snap = self.usage.as_ref()?;
            if (now / 1000).saturating_sub(snap.written_at) > self.usage_stale_secs {
                return None;
            }
            snap.effort_level.as_deref().and_then(EffortLevel::from_level)
        });
        if effort != self.last_effort {
            self.dbg(&format!("effort={effort:?}"));
            self.last_effort = effort;
        }

        // Animations run only while the card has something to say (any session working,
        // or waiting on the user) and the window is visible; otherwise the animation
        // clock freezes (backdrops render, but static) and no fast repaints are
        // scheduled — idle CPU stays at the 1s heartbeat. Note the wave keys on
        // any_working(), not the aggregate: a waiting session outranks Working in the
        // headline, but another session's wave must keep emanating.
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        let any_working = self.roster.any_working();
        let animate = (any_working || matches!(agg, AggregateState::Working | AggregateState::WaitingForInput))
            && !minimized;
        let anim_t = if animate { now } else { 0 };

        let backdrop = widget_core::effort_backdrop(effort, anim_t);
        let base = egui::Color32::from_rgb(18, 18, 22);
        let bg = match &backdrop {
            Backdrop::Tint { color, blend } => mix(base, *color, *blend),
            _ => base,
        };

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(bg).inner_margin(16.0))
            .show(ctx, |ui| {
                let card = ui.max_rect();
                let painter = ui.painter().clone();

                // Max effort: the rainbow-pixel field, under everything else (drawn
                // first). Cells drift diagonally as `phase` advances. Full-bleed: the
                // grid covers the frame margin too, not just the content rect.
                if let Backdrop::RainbowPixels { phase } = backdrop {
                    const CELL: f32 = 9.0;
                    let full = card.expand(16.0);
                    let cols = (full.width() / CELL).ceil() as i32;
                    let rows = (full.height() / CELL).ceil() as i32;
                    for row in 0..rows {
                        for col in 0..cols {
                            let c = widget_core::rainbow_color(phase, (row + col) as f32 * 0.045);
                            let cell = egui::Rect::from_min_size(
                                full.min + egui::vec2(col as f32 * CELL, row as f32 * CELL),
                                egui::vec2(CELL, CELL),
                            );
                            painter.rect_filled(cell, 0.0, mix(base, c, 0.30));
                        }
                    }
                }

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

                // Header: the spark mascot, with the status beside it.
                ui.horizontal(|ui| {
                    let (m_rect, _) =
                        ui.allocate_exact_size(egui::vec2(46.0, 46.0), egui::Sense::hover());
                    let center = m_rect.center();

                    // The working wave: staggered purple rings emanating from the mascot
                    // (purple → transparent → purple), fading as they cross the card.
                    if animate && any_working {
                        let max_r = card.width().max(card.height());
                        for k in 0..3 {
                            let (prog, alpha) = widget_core::wave_ring(k, 3, anim_t);
                            if prog < 0.02 {
                                continue;
                            }
                            let c = widget_core::lerp_rgb(
                                widget_core::WAVE_PURPLE_BRIGHT,
                                widget_core::WAVE_PURPLE_DIM,
                                prog,
                            );
                            let ring = egui::Color32::from_rgba_unmultiplied(
                                c.0,
                                c.1,
                                c.2,
                                (alpha * 255.0) as u8,
                            );
                            painter.circle_stroke(center, prog * max_r, egui::Stroke::new(7.0, ring));
                        }
                    }

                    // The spark's expression is its color/size/motion per state:
                    // rotating coral while working, red urgency pulse when Claude needs
                    // you, dimmed at idle, gray with no sessions.
                    let (color, radius, angle) = match agg {
                        AggregateState::Working => {
                            // Wrap time BEFORE the f32 conversion: at epoch-ms magnitude
                            // an f32 step is ~2^17 ms, which would collapse all arm
                            // angles into one and freeze the rotation.
                            let angle = (anim_t % 8000) as f32 / 8000.0 * std::f32::consts::TAU;
                            (rgb(widget_core::SPARK_CORAL), 16.0, angle)
                        }
                        AggregateState::WaitingForInput => {
                            let s = 0.5
                                + 0.5
                                    * ((anim_t % 1200) as f32 / 1200.0 * std::f32::consts::TAU)
                                        .sin();
                            (mix(rgb(widget_core::SPARK_CORAL), (240, 70, 60), s), 15.0 + 2.5 * s, 0.0)
                        }
                        AggregateState::RateLimited => (rgb((240, 95, 90)), 15.0, 0.0),
                        AggregateState::Idle => {
                            (rgb(widget_core::SPARK_CORAL).gamma_multiply(0.55), 14.0, 0.0)
                        }
                        AggregateState::NoSessions => (egui::Color32::from_gray(95), 14.0, 0.0),
                    };
                    draw_spark(&painter, center, radius, angle, color);

                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Claude")
                                .size(13.0)
                                .color(egui::Color32::from_gray(205)),
                        );
                        ui.label(egui::RichText::new(label).size(21.0).color(accent));
                    });
                });
                ui.add_space(6.0);

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

        // Schedule the next animation frame (~30 fps) only while animating; otherwise
        // the 1s heartbeat is the only wakeup and CPU stays near zero.
        if animate {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
}
