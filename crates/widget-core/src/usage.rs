//! Usage-limit gauge logic: parse the statusline `rate_limits` JSON, normalize it into
//! an account-global snapshot, and turn a snapshot into presentation-ready gauge state.
//!
//! Data path (decided in spike #3): Claude Code pipes statusline JSON to our emitter
//! (`claude-widget statusline`), which writes one shared snapshot file; the widget reads
//! that file. The statusline never runs in the VS Code panel, so the snapshot is
//! last-known-from-any-(terminal)-session — the gauge must be honest about freshness.
//!
//! Pure like the rest of widget-core: no I/O, time injected as `now_secs`.

use serde_json::Value;

/// One rate-limit window (`five_hour` / `seven_day`) as reported by the statusline.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    /// Percent of the window's budget used, clamped to 0..=100.
    pub used_percentage: f64,
    /// When the window resets (epoch seconds). Absent if the source omitted it.
    pub resets_at: Option<u64>,
}

/// The account-global snapshot the emitter writes and the widget reads. Each window can
/// be independently absent (the statusline only includes what the account reports).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSnapshot {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    /// Effort level riding the statusline payload (pass-through for the visuals ticket).
    pub effort_level: Option<String>,
    /// When the emitter observed this data (epoch seconds).
    pub written_at: u64,
}

impl UsageSnapshot {
    /// True if there is at least one limit window worth persisting. The emitter skips the
    /// write otherwise, so an empty payload never clobbers a good last-known snapshot.
    pub fn has_windows(&self) -> bool {
        self.five_hour.is_some() || self.seven_day.is_some()
    }
}

/// Everything the emitter needs from one statusline stdin payload: the snapshot to
/// persist plus the bits used only for the emitter's own stdout line.
#[derive(Debug, Clone, PartialEq)]
pub struct StatuslineInput {
    pub snapshot: UsageSnapshot,
    /// Model display name (e.g. "Opus") for the statusline text.
    pub model_display: Option<String>,
    /// Context-window used percentage for the statusline text.
    pub context_pct: Option<f64>,
}

/// Read one window object from a JSON value. Field-name trap: the statusline calls the
/// percentage `used_percentage`; the OAuth usage endpoint calls it `utilization`.
fn window_from(v: &Value) -> Option<UsageWindow> {
    let obj = v.as_object()?;
    let pct = obj
        .get("used_percentage")
        .or_else(|| obj.get("utilization"))
        .and_then(Value::as_f64)?;
    Some(UsageWindow {
        used_percentage: pct.clamp(0.0, 100.0),
        resets_at: obj.get("resets_at").and_then(reset_epoch),
    })
}

/// `resets_at` format trap: accept epoch seconds (number) or an ISO-8601 string.
fn reset_epoch(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_f64() {
        return (n >= 0.0).then_some(n as u64);
    }
    v.as_str().and_then(iso_to_epoch)
}

/// The `effort.level` string, from either payload kind.
fn effort_level(v: &Value) -> Option<String> {
    v.pointer("/effort/level").and_then(Value::as_str).map(str::to_string)
}

/// Parse the JSON Claude Code pipes to the statusline command. Returns `None` only for
/// unparseable JSON; missing fields degrade to `None`s (each window independently).
pub fn parse_statusline_input(json: &str, now_secs: u64) -> Option<StatuslineInput> {
    let v: Value = serde_json::from_str(json).ok()?;
    let limits = v.get("rate_limits");
    let window = |name| limits.and_then(|l| l.get(name)).and_then(window_from);
    Some(StatuslineInput {
        snapshot: UsageSnapshot {
            five_hour: window("five_hour"),
            seven_day: window("seven_day"),
            effort_level: effort_level(&v),
            written_at: now_secs,
        },
        model_display: v
            .pointer("/model/display_name")
            .and_then(Value::as_str)
            .map(str::to_string),
        context_pct: v.pointer("/context_window/used_percentage").and_then(Value::as_f64),
    })
}

/// Serialize a snapshot to the JSON written to the shared snapshot file. Timestamps are
/// normalized to epoch seconds on write regardless of the source format.
pub fn snapshot_json(snap: &UsageSnapshot) -> String {
    let window = |w: &Option<UsageWindow>| {
        w.as_ref().map(|w| {
            let mut o = serde_json::Map::new();
            o.insert("used_percentage".into(), w.used_percentage.into());
            if let Some(r) = w.resets_at {
                o.insert("resets_at".into(), r.into());
            }
            Value::Object(o)
        })
    };
    let mut root = serde_json::Map::new();
    if let Some(f) = window(&snap.five_hour) {
        root.insert("five_hour".into(), f);
    }
    if let Some(s) = window(&snap.seven_day) {
        root.insert("seven_day".into(), s);
    }
    if let Some(e) = &snap.effort_level {
        root.insert("effort".into(), serde_json::json!({ "level": e }));
    }
    root.insert("written_at".into(), snap.written_at.into());
    Value::Object(root).to_string()
}

/// Parse the OAuth usage endpoint's response body into a snapshot (the opt-in rung,
/// issue #14). Same tolerance rules as the statusline path: windows independently
/// absent, `utilization`/`used_percentage` both accepted, epoch or ISO `resets_at`,
/// unknown extra windows ignored. `None` only for unparseable/shape-less JSON.
pub fn parse_oauth_usage(json: &str, now_secs: u64) -> Option<UsageSnapshot> {
    let v: Value = serde_json::from_str(json).ok()?;
    if !v.is_object() {
        return None;
    }
    Some(UsageSnapshot {
        five_hour: v.get("five_hour").and_then(window_from),
        seven_day: v.get("seven_day").and_then(window_from),
        effort_level: None,
        written_at: now_secs,
    })
}

/// Parse the shared snapshot file (the emitter's own output, or a hand-rolled one).
pub fn parse_usage_snapshot(json: &str) -> Option<UsageSnapshot> {
    let v: Value = serde_json::from_str(json).ok()?;
    if !v.is_object() {
        return None;
    }
    Some(UsageSnapshot {
        five_hour: v.get("five_hour").and_then(window_from),
        seven_day: v.get("seven_day").and_then(window_from),
        effort_level: effort_level(&v),
        written_at: v.get("written_at").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// The one-line text our statusline entry displays in Claude Code itself, e.g.
/// `Opus · ctx 34% · 5h 24% · 7d 41%`. Skips unknown segments; never empty.
pub fn statusline_text(input: &StatuslineInput) -> String {
    let mut parts = Vec::new();
    if let Some(m) = &input.model_display {
        parts.push(m.clone());
    }
    if let Some(c) = input.context_pct {
        parts.push(format!("ctx {}%", c.round() as i64));
    }
    if let Some(w) = &input.snapshot.five_hour {
        parts.push(format!("5h {}%", w.used_percentage.round() as i64));
    }
    if let Some(w) = &input.snapshot.seven_day {
        parts.push(format!("7d {}%", w.used_percentage.round() as i64));
    }
    if parts.is_empty() {
        return "Claude".into();
    }
    parts.join(" · ")
}

/// Minimal ISO-8601 → epoch seconds ("2024-01-01T00:00:00Z", fractional seconds and
/// ±HH:MM offsets accepted). Fractions truncate. `None` on anything malformed.
fn iso_to_epoch(s: &str) -> Option<u64> {
    let s = s.trim();
    let (date, rest) = s.split_at(s.find(['T', ' '])?);
    let rest = &rest[1..];

    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let m: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    if d.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }

    // Split the time from a trailing Z or ±HH:MM offset.
    let (time, offset_secs) = if let Some(t) = rest.strip_suffix(['Z', 'z']) {
        (t, 0i64)
    } else if let Some(i) = rest.rfind(['+', '-']) {
        let (t, off) = rest.split_at(i);
        let sign = if off.starts_with('-') { -1i64 } else { 1 };
        let mut p = off[1..].split(':');
        let oh: i64 = p.next()?.parse().ok()?;
        let om: i64 = p.next().unwrap_or("0").parse().ok()?;
        (t, sign * (oh * 3600 + om * 60))
    } else {
        (rest, 0) // no zone designator: treat as UTC
    };

    let time = time.split('.').next()?; // truncate fractional seconds
    let mut t = time.split(':');
    let hh: i64 = t.next()?.parse().ok()?;
    let mm: i64 = t.next()?.parse().ok()?;
    let ss: i64 = t.next().unwrap_or("0").parse().ok()?;
    if !(0..24).contains(&hh) || !(0..60).contains(&mm) || !(0..61).contains(&ss) {
        return None;
    }

    // Days since 1970-01-01 (Howard Hinnant's days-from-civil).
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719_468;

    let epoch = days * 86_400 + hh * 3_600 + mm * 60 + ss - offset_secs;
    u64::try_from(epoch).ok()
}

/// Traffic-light tier for a utilization percentage (thresholds from the CC theme:
/// calm below 70, amber 70–89, red at 90+ — the CLI's own alarm threshold).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaugeTier {
    Calm,
    Warn,
    Alert,
}

pub fn tier(pct: f64) -> GaugeTier {
    if pct >= 90.0 {
        GaugeTier::Alert
    } else if pct >= 70.0 {
        GaugeTier::Warn
    } else {
        GaugeTier::Calm
    }
}

/// One window, presentation-ready.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowView {
    /// Short label: "5h" or "7d".
    pub label: &'static str,
    /// Percent to draw, already decayed to 0 once `resets_at` has passed.
    pub pct: f64,
    pub tier: GaugeTier,
    /// Seconds until the window resets; `None` if unknown or already reset.
    pub resets_in_secs: Option<u64>,
}

/// The gauge as the card should draw it.
#[derive(Debug, Clone, PartialEq)]
pub struct GaugeView {
    /// 0–2 windows, five-hour first.
    pub windows: Vec<WindowView>,
    /// When the underlying data was observed (epoch seconds) — the "as of" label.
    pub as_of_secs: u64,
    /// True when the source has gone quiet: older than the staleness threshold.
    pub stale: bool,
}

/// Turn a snapshot into presentation state at `now_secs`. A window whose `resets_at` has
/// passed decays to 0% (the limit restarted; last-known usage no longer applies).
pub fn gauge_view(snap: &UsageSnapshot, now_secs: u64, stale_after_secs: u64) -> GaugeView {
    let view = |label: &'static str, w: &Option<UsageWindow>| {
        w.as_ref().map(|w| {
            let expired = w.resets_at.is_some_and(|r| r <= now_secs);
            let pct = if expired { 0.0 } else { w.used_percentage };
            WindowView {
                label,
                pct,
                tier: tier(pct),
                resets_in_secs: if expired { None } else { w.resets_at.map(|r| r - now_secs) },
            }
        })
    };
    GaugeView {
        windows: [view("5h", &snap.five_hour), view("7d", &snap.seven_day)]
            .into_iter()
            .flatten()
            .collect(),
        as_of_secs: snap.written_at,
        stale: now_secs.saturating_sub(snap.written_at) > stale_after_secs,
    }
}

/// Compact countdown: "45m", "2h 15m", "3d 4h"; "<1m" under a minute.
pub fn fmt_countdown(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs / 3_600) % 24, (secs / 60) % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        "<1m".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"{
        "model": { "id": "claude-opus-4-8", "display_name": "Opus" },
        "effort": { "level": "xhigh" },
        "context_window": { "context_window_size": 200000, "used_percentage": 34.2 },
        "rate_limits": {
            "five_hour": { "used_percentage": 23.5, "resets_at": 1738425600 },
            "seven_day": { "used_percentage": 41.2, "resets_at": 1738857600 }
        }
    }"#;

    // --- statusline input parsing ---

    #[test]
    fn parses_full_statusline_input() {
        let inp = parse_statusline_input(FULL, 1738420000).unwrap();
        let five = inp.snapshot.five_hour.as_ref().unwrap();
        assert_eq!(five.used_percentage, 23.5);
        assert_eq!(five.resets_at, Some(1738425600));
        let seven = inp.snapshot.seven_day.as_ref().unwrap();
        assert_eq!(seven.used_percentage, 41.2);
        assert_eq!(inp.snapshot.effort_level.as_deref(), Some("xhigh"));
        assert_eq!(inp.snapshot.written_at, 1738420000);
        assert_eq!(inp.model_display.as_deref(), Some("Opus"));
        assert_eq!(inp.context_pct, Some(34.2));
        assert!(inp.snapshot.has_windows());
    }

    #[test]
    fn replays_recorded_statusline_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tracker/assets/spike3-statusline-input.sample.json"
        );
        let data = std::fs::read_to_string(path).expect("recorded fixture is present");
        let inp = parse_statusline_input(&data, 1738420000).expect("fixture parses");
        assert!(inp.snapshot.has_windows());
        assert_eq!(inp.snapshot.five_hour.as_ref().unwrap().used_percentage, 23.5);
        assert_eq!(inp.snapshot.seven_day.as_ref().unwrap().resets_at, Some(1738857600));
        assert_eq!(inp.model_display.as_deref(), Some("Opus"));
    }

    #[test]
    fn missing_rate_limits_degrades_to_no_windows() {
        let inp = parse_statusline_input(r#"{"model":{"display_name":"Opus"}}"#, 5).unwrap();
        assert!(!inp.snapshot.has_windows());
        assert_eq!(inp.model_display.as_deref(), Some("Opus"));
    }

    #[test]
    fn windows_are_independently_absent() {
        let inp = parse_statusline_input(
            r#"{"rate_limits":{"five_hour":{"used_percentage":10}}}"#,
            5,
        )
        .unwrap();
        assert!(inp.snapshot.five_hour.is_some());
        assert!(inp.snapshot.seven_day.is_none());
        // resets_at absent inside a present window is fine too.
        assert_eq!(inp.snapshot.five_hour.unwrap().resets_at, None);
    }

    #[test]
    fn invalid_json_is_none() {
        assert!(parse_statusline_input("not json", 5).is_none());
    }

    #[test]
    fn null_buckets_are_handled() {
        // Explicit nulls (not just absent keys) at every level of rate_limits.
        let inp = parse_statusline_input(r#"{"rate_limits":null}"#, 5).unwrap();
        assert!(!inp.snapshot.has_windows());
        let inp = parse_statusline_input(
            r#"{"rate_limits":{"five_hour":null,"seven_day":{"used_percentage":7,"resets_at":null}}}"#,
            5,
        )
        .unwrap();
        assert!(inp.snapshot.five_hour.is_none());
        let seven = inp.snapshot.seven_day.unwrap();
        assert_eq!(seven.used_percentage, 7.0);
        assert_eq!(seven.resets_at, None);
        // Same tolerance when re-reading a snapshot file.
        let snap = parse_usage_snapshot(r#"{"five_hour":null,"written_at":9}"#).unwrap();
        assert!(snap.five_hour.is_none());
        assert_eq!(snap.written_at, 9);
    }

    #[test]
    fn resets_at_accepts_iso_strings() {
        // Format trap: some emitters/tools report ISO strings, not epoch seconds.
        let inp = parse_statusline_input(
            r#"{"rate_limits":{"five_hour":{"used_percentage":1,"resets_at":"2024-01-01T00:00:00Z"}}}"#,
            5,
        )
        .unwrap();
        assert_eq!(inp.snapshot.five_hour.unwrap().resets_at, Some(1704067200));
        // With a UTC offset and fractional seconds.
        let inp = parse_statusline_input(
            r#"{"rate_limits":{"five_hour":{"used_percentage":1,"resets_at":"2024-01-01T01:30:00.250+01:30"}}}"#,
            5,
        )
        .unwrap();
        assert_eq!(inp.snapshot.five_hour.unwrap().resets_at, Some(1704067200));
        // Garbage string -> window kept, reset unknown.
        let inp = parse_statusline_input(
            r#"{"rate_limits":{"five_hour":{"used_percentage":1,"resets_at":"soon"}}}"#,
            5,
        )
        .unwrap();
        assert_eq!(inp.snapshot.five_hour.unwrap().resets_at, None);
    }

    #[test]
    fn used_percentage_is_clamped() {
        let inp = parse_statusline_input(
            r#"{"rate_limits":{"five_hour":{"used_percentage":-5},"seven_day":{"used_percentage":250}}}"#,
            5,
        )
        .unwrap();
        assert_eq!(inp.snapshot.five_hour.unwrap().used_percentage, 0.0);
        assert_eq!(inp.snapshot.seven_day.unwrap().used_percentage, 100.0);
    }

    // --- OAuth usage endpoint parsing (issue #14) ---

    #[test]
    fn parses_oauth_usage_response() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tracker/assets/oauth-usage.sample.json"
        );
        let data = std::fs::read_to_string(path).expect("fixture present");
        let snap = parse_oauth_usage(&data, 1_783_690_000).expect("fixture parses");
        assert_eq!(snap.written_at, 1_783_690_000);
        let five = snap.five_hour.as_ref().unwrap();
        assert_eq!(five.used_percentage, 23.5);
        // ISO resets_at -> epoch: 2026-07-10T15:19:05Z.
        assert_eq!(five.resets_at, Some(1_783_696_745));
        assert_eq!(snap.seven_day.as_ref().unwrap().used_percentage, 41.2);
        // Unknown extra windows (seven_day_opus) are ignored, not an error.
        assert!(snap.has_windows());
    }

    #[test]
    fn oauth_usage_degrades_like_the_statusline_path() {
        // Windows independently absent; used_percentage naming also accepted; nulls ok.
        let snap = parse_oauth_usage(
            r#"{"five_hour":{"used_percentage":250,"resets_at":1738425600},"seven_day":null}"#,
            7,
        )
        .unwrap();
        assert_eq!(snap.five_hour.as_ref().unwrap().used_percentage, 100.0, "clamped");
        assert_eq!(snap.five_hour.as_ref().unwrap().resets_at, Some(1738425600));
        assert!(snap.seven_day.is_none());
        assert!(parse_oauth_usage("[]", 7).is_none(), "shape-less JSON is None");
        assert!(parse_oauth_usage("nope", 7).is_none());
    }

    // --- snapshot file round-trip ---

    #[test]
    fn snapshot_round_trips_through_json() {
        let snap = UsageSnapshot {
            five_hour: Some(UsageWindow { used_percentage: 23.5, resets_at: Some(1738425600) }),
            seven_day: None,
            effort_level: Some("max".into()),
            written_at: 1738420000,
        };
        let parsed = parse_usage_snapshot(&snapshot_json(&snap)).unwrap();
        assert_eq!(parsed, snap);
    }

    #[test]
    fn replays_recorded_snapshot_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tracker/assets/spike3-usage-snapshot.sample.json"
        );
        let data = std::fs::read_to_string(path).expect("recorded fixture is present");
        let snap = parse_usage_snapshot(&data).expect("fixture parses");
        assert_eq!(snap.written_at, 1738420000);
        assert_eq!(snap.five_hour.as_ref().unwrap().used_percentage, 23.5);
        assert_eq!(snap.seven_day.as_ref().unwrap().resets_at, Some(1738857600));
        assert_eq!(snap.effort_level.as_deref(), Some("xhigh"));
    }

    #[test]
    fn snapshot_missing_pieces_degrade() {
        let snap = parse_usage_snapshot(r#"{"five_hour":{"used_percentage":9}}"#).unwrap();
        assert!(snap.seven_day.is_none());
        assert_eq!(snap.written_at, 0, "missing written_at reads as epoch 0 (always stale)");
        assert!(parse_usage_snapshot("garbage").is_none());
    }

    // --- statusline stdout text ---

    #[test]
    fn statusline_text_shows_known_segments() {
        let inp = parse_statusline_input(FULL, 1738420000).unwrap();
        assert_eq!(statusline_text(&inp), "Opus · ctx 34% · 5h 24% · 7d 41%");
    }

    #[test]
    fn statusline_text_skips_unknown_segments_and_never_empties() {
        let inp = parse_statusline_input(r#"{"model":{"display_name":"Opus"}}"#, 5).unwrap();
        assert_eq!(statusline_text(&inp), "Opus");
        let inp = parse_statusline_input("{}", 5).unwrap();
        assert_eq!(statusline_text(&inp), "Claude");
    }

    // --- gauge presentation ---

    fn snap(five_pct: f64, five_reset: u64, seven_pct: f64, seven_reset: u64, at: u64) -> UsageSnapshot {
        UsageSnapshot {
            five_hour: Some(UsageWindow { used_percentage: five_pct, resets_at: Some(five_reset) }),
            seven_day: Some(UsageWindow { used_percentage: seven_pct, resets_at: Some(seven_reset) }),
            effort_level: None,
            written_at: at,
        }
    }

    #[test]
    fn gauge_view_fresh_snapshot() {
        let s = snap(23.5, 2_000, 41.2, 50_000, 1_000);
        let g = gauge_view(&s, 1_100, 900);
        assert!(!g.stale);
        assert_eq!(g.as_of_secs, 1_000);
        assert_eq!(g.windows.len(), 2);
        assert_eq!(g.windows[0].label, "5h");
        assert_eq!(g.windows[0].pct, 23.5);
        assert_eq!(g.windows[0].resets_in_secs, Some(900));
        assert_eq!(g.windows[1].label, "7d");
        assert_eq!(g.windows[1].resets_in_secs, Some(48_900));
    }

    #[test]
    fn gauge_window_decays_to_zero_after_reset() {
        let s = snap(80.0, 2_000, 95.0, 50_000, 1_000);
        let g = gauge_view(&s, 3_000, 1_000_000);
        assert_eq!(g.windows[0].pct, 0.0, "past resets_at the window restarted");
        assert_eq!(g.windows[0].resets_in_secs, None);
        assert_eq!(g.windows[0].tier, GaugeTier::Calm);
        // The 7d window has not reset and keeps its value.
        assert_eq!(g.windows[1].pct, 95.0);
        assert_eq!(g.windows[1].tier, GaugeTier::Alert);
    }

    #[test]
    fn gauge_goes_stale_when_source_is_quiet() {
        let s = snap(10.0, 9_000_000, 10.0, 9_000_000, 1_000);
        assert!(!gauge_view(&s, 1_000 + 900, 900).stale, "at the threshold is still fresh");
        assert!(gauge_view(&s, 1_000 + 901, 900).stale);
    }

    #[test]
    fn gauge_with_one_window_only() {
        let s = UsageSnapshot {
            five_hour: None,
            seven_day: Some(UsageWindow { used_percentage: 12.0, resets_at: None }),
            effort_level: None,
            written_at: 50,
        };
        let g = gauge_view(&s, 60, 900);
        assert_eq!(g.windows.len(), 1);
        assert_eq!(g.windows[0].label, "7d");
        assert_eq!(g.windows[0].resets_in_secs, None, "no resets_at -> no countdown");
    }

    #[test]
    fn tier_thresholds_match_the_cli() {
        assert_eq!(tier(0.0), GaugeTier::Calm);
        assert_eq!(tier(69.9), GaugeTier::Calm);
        assert_eq!(tier(70.0), GaugeTier::Warn);
        assert_eq!(tier(89.9), GaugeTier::Warn);
        assert_eq!(tier(90.0), GaugeTier::Alert);
        assert_eq!(tier(100.0), GaugeTier::Alert);
    }

    #[test]
    fn countdown_formats_compactly() {
        assert_eq!(fmt_countdown(30), "<1m");
        assert_eq!(fmt_countdown(2_700), "45m");
        assert_eq!(fmt_countdown(8_100), "2h 15m");
        assert_eq!(fmt_countdown(3 * 86_400 + 4 * 3_600 + 120), "3d 4h");
    }
}
