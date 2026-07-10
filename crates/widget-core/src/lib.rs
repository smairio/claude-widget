//! Pure state core for the Claude Code status widget.
//!
//! Takes the JSON a Claude Code hook delivers and folds it into per-session and
//! aggregate widget state. No I/O, no GUI. Time is injected (`now_ms`) so the stall
//! backstop is testable. This is the single test seam for the widget.
//!
//! Two sources of truth, kept separate:
//! - the **registry** (`~/.claude/sessions/*.json`, read by the daemon) says *which*
//!   sessions exist and are alive — fed in via [`Roster::retain_live`] / [`Roster::ensure_idle`];
//! - the **hooks** say what each session is *doing* — fed in via [`Roster::apply_at`].
//!
//! Because Claude Code does not fire `Stop` on a user interrupt (and fires `StopFailure`,
//! not `Stop`, on an API error), "working" cannot rely on `Stop` alone: a stall backstop
//! ([`Roster::expire_stale`]) returns a quiet "working" session to idle.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

/// What a single Claude Code session is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Working,
    Idle,
    /// Blocked on the user (an `AskUserQuestion` / permission prompt). Reserved for #6.
    WaitingForInput,
    /// The turn died on a rate limit. Reserved for #6.
    RateLimited,
}

/// The single state the card shows when several sessions are live.
/// Precedence (most attention-worthy wins): waiting > rate-limited > working > idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateState {
    NoSessions,
    Idle,
    Working,
    WaitingForInput,
    RateLimited,
}

/// A hook event, normalized from the raw payload into just what the state machine needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    UserPromptSubmit,
    PreToolUse { tool_name: String, is_subagent: bool },
    PostToolUse { tool_name: String, is_subagent: bool },
    Stop,
    /// Turn ended on an error (rate limit, overload, …). Fires *instead of* `Stop`.
    StopFailure,
    SessionEnd,
    /// A hook we observe but that does not drive a transition on its own.
    Other(String),
}

/// A parsed hook payload: which session it belongs to, and the event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvent {
    pub session_id: String,
    pub event: HookEvent,
}

#[derive(Debug)]
pub enum ParseError {
    Json(String),
    MissingSessionId,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Json(e) => write!(f, "invalid hook JSON: {e}"),
            ParseError::MissingSessionId => write!(f, "hook payload has no session_id"),
        }
    }
}

impl std::error::Error for ParseError {}

#[derive(Deserialize)]
struct RawHook {
    hook_event_name: Option<String>,
    session_id: Option<String>,
    tool_name: Option<String>,
    /// Present only when the event fires inside a subagent.
    #[serde(default)]
    agent_id: Option<String>,
}

/// Parse a single hook payload (the JSON delivered on the hook's stdin / HTTP body).
pub fn parse_hook(json: &str) -> Result<ParsedEvent, ParseError> {
    let raw: RawHook = serde_json::from_str(json).map_err(|e| ParseError::Json(e.to_string()))?;
    let session_id = raw.session_id.ok_or(ParseError::MissingSessionId)?;
    let is_subagent = raw.agent_id.is_some();
    let event = match raw.hook_event_name.as_deref() {
        Some("UserPromptSubmit") => HookEvent::UserPromptSubmit,
        Some("PreToolUse") => HookEvent::PreToolUse {
            tool_name: raw.tool_name.unwrap_or_default(),
            is_subagent,
        },
        Some("PostToolUse") => HookEvent::PostToolUse {
            tool_name: raw.tool_name.unwrap_or_default(),
            is_subagent,
        },
        Some("Stop") => HookEvent::Stop,
        Some("StopFailure") => HookEvent::StopFailure,
        Some("SessionEnd") => HookEvent::SessionEnd,
        Some(other) => HookEvent::Other(other.to_string()),
        None => HookEvent::Other(String::new()),
    };
    Ok(ParsedEvent { session_id, event })
}

/// Per-session record: current state plus the last time we saw activity (for the backstop).
#[derive(Debug, Clone, Copy)]
struct Session {
    state: SessionState,
    last_activity_ms: u64,
}

/// The transition an event drives, given the current state. `None` means "drop the session".
///
/// Skeleton scope: prompt/tool activity → Working; `Stop`/`StopFailure` → Idle;
/// `SessionEnd` → drop. (`WaitingForInput`/`RateLimited` land in #6.)
pub fn next_state(current: Option<SessionState>, event: &HookEvent) -> Option<SessionState> {
    match event {
        HookEvent::UserPromptSubmit => Some(SessionState::Working),
        HookEvent::PreToolUse { .. } => Some(SessionState::Working),
        HookEvent::PostToolUse { .. } => Some(SessionState::Working),
        HookEvent::Stop => Some(SessionState::Idle),
        HookEvent::StopFailure => Some(SessionState::Idle),
        HookEvent::SessionEnd => None,
        HookEvent::Other(_) => current,
    }
}

/// The live set of sessions and their states.
#[derive(Debug, Default, Clone)]
pub struct Roster {
    sessions: BTreeMap<String, Session>,
}

impl Roster {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one parsed event at logical time `now_ms`.
    pub fn apply_at(&mut self, ev: &ParsedEvent, now_ms: u64) {
        let current = self.sessions.get(&ev.session_id).map(|s| s.state);
        match next_state(current, &ev.event) {
            Some(state) => {
                self.sessions.insert(
                    ev.session_id.clone(),
                    Session { state, last_activity_ms: now_ms },
                );
            }
            None => {
                self.sessions.remove(&ev.session_id);
            }
        }
    }

    /// Time-agnostic apply (last-activity stamped 0). Convenient for pure transition tests.
    pub fn apply(&mut self, ev: &ParsedEvent) {
        self.apply_at(ev, 0);
    }

    /// Parse and apply a raw hook body at `now_ms`. Malformed bodies error and leave the
    /// roster untouched (the daemon logs and drops).
    pub fn apply_raw_at(&mut self, json: &str, now_ms: u64) -> Result<(), ParseError> {
        let ev = parse_hook(json)?;
        self.apply_at(&ev, now_ms);
        Ok(())
    }

    pub fn apply_raw(&mut self, json: &str) -> Result<(), ParseError> {
        self.apply_raw_at(json, 0)
    }

    /// Ensure a (registry-discovered) session exists, as Idle, without disturbing one we
    /// already track. Called when the daemon reads the session registry.
    pub fn ensure_idle(&mut self, session_id: &str, now_ms: u64) {
        self.sessions
            .entry(session_id.to_string())
            .or_insert(Session { state: SessionState::Idle, last_activity_ms: now_ms });
    }

    /// Drop every session whose id is not in `live` (its process is gone / registry entry
    /// removed). Registry files are written at session start, before any hook fires, so a
    /// hook always corresponds to a live registry entry — no live session is dropped here.
    pub fn retain_live(&mut self, live: &BTreeSet<String>) {
        self.sessions.retain(|id, _| live.contains(id));
    }

    /// Backstop for the missing terminal event: a session that has been Working with no
    /// activity for `timeout_ms` is returned to Idle. Catches user interrupts (no `Stop`),
    /// crashes, and any dropped event.
    pub fn expire_stale(&mut self, now_ms: u64, timeout_ms: u64) {
        for s in self.sessions.values_mut() {
            if s.state == SessionState::Working && now_ms.saturating_sub(s.last_activity_ms) >= timeout_ms {
                s.state = SessionState::Idle;
            }
        }
    }

    pub fn state_of(&self, session_id: &str) -> Option<SessionState> {
        self.sessions.get(session_id).map(|s| s.state)
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// True while any session is Working — the daemon uses this to decide whether to keep
    /// waking up for the stall backstop.
    pub fn any_working(&self) -> bool {
        self.sessions.values().any(|s| s.state == SessionState::Working)
    }

    /// The single state the card renders, applying the precedence order.
    pub fn aggregate(&self) -> AggregateState {
        if self.sessions.is_empty() {
            return AggregateState::NoSessions;
        }
        let mut agg = AggregateState::Idle;
        for s in self.sessions.values() {
            match s.state {
                SessionState::WaitingForInput => return AggregateState::WaitingForInput,
                SessionState::RateLimited => agg = AggregateState::RateLimited,
                SessionState::Working => {
                    if agg != AggregateState::RateLimited {
                        agg = AggregateState::Working;
                    }
                }
                SessionState::Idle => {}
            }
        }
        agg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(event: &str, session: &str) -> String {
        format!(r#"{{"hook_event_name":"{event}","session_id":"{session}"}}"#)
    }

    #[test]
    fn parses_pretooluse_with_tool_name() {
        let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"Bash"}"#;
        let parsed = parse_hook(json).unwrap();
        assert_eq!(parsed.session_id, "s1");
        assert_eq!(
            parsed.event,
            HookEvent::PreToolUse { tool_name: "Bash".into(), is_subagent: false }
        );
    }

    #[test]
    fn detects_subagent_via_agent_id() {
        let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"Bash","agent_id":"a1"}"#;
        match parse_hook(json).unwrap().event {
            HookEvent::PreToolUse { is_subagent, .. } => assert!(is_subagent),
            other => panic!("expected PreToolUse, got {other:?}"),
        }
    }

    #[test]
    fn missing_session_id_is_an_error() {
        assert!(matches!(parse_hook(r#"{"hook_event_name":"Stop"}"#), Err(ParseError::MissingSessionId)));
    }

    #[test]
    fn malformed_json_is_an_error_and_does_not_panic() {
        assert!(matches!(parse_hook("not json"), Err(ParseError::Json(_))));
    }

    #[test]
    fn stop_moves_session_to_idle() {
        let mut r = Roster::new();
        r.apply_raw(&hook("UserPromptSubmit", "s1")).unwrap();
        assert_eq!(r.state_of("s1"), Some(SessionState::Working));
        r.apply_raw(&hook("Stop", "s1")).unwrap();
        assert_eq!(r.state_of("s1"), Some(SessionState::Idle));
    }

    #[test]
    fn stop_failure_also_idles_the_session() {
        let mut r = Roster::new();
        r.apply_raw(&hook("PreToolUse", "s1")).unwrap();
        r.apply_raw(&hook("StopFailure", "s1")).unwrap();
        assert_eq!(r.state_of("s1"), Some(SessionState::Idle), "errored turn must not stick working");
    }

    #[test]
    fn session_end_drops_the_session() {
        let mut r = Roster::new();
        r.apply_raw(&hook("UserPromptSubmit", "s1")).unwrap();
        r.apply_raw(&hook("SessionEnd", "s1")).unwrap();
        assert!(r.is_empty());
        assert_eq!(r.aggregate(), AggregateState::NoSessions);
    }

    #[test]
    fn aggregate_prefers_working_over_idle() {
        let mut r = Roster::new();
        r.apply_raw(&hook("Stop", "s1")).unwrap();
        r.apply_raw(&hook("PreToolUse", "s2")).unwrap();
        assert_eq!(r.aggregate(), AggregateState::Working);
    }

    #[test]
    fn malformed_payload_leaves_roster_untouched() {
        let mut r = Roster::new();
        r.apply_raw(&hook("PreToolUse", "s1")).unwrap();
        assert!(r.apply_raw("garbage").is_err());
        assert_eq!(r.state_of("s1"), Some(SessionState::Working));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn stall_backstop_idles_an_interrupted_working_session() {
        // A turn that starts working and is then INTERRUPTED (no Stop ever arrives).
        let mut r = Roster::new();
        r.apply_raw_at(&hook("PreToolUse", "s1"), 1_000).unwrap();
        assert_eq!(r.aggregate(), AggregateState::Working);
        // Not yet stale.
        r.expire_stale(5_000, 45_000);
        assert_eq!(r.aggregate(), AggregateState::Working);
        // Past the backstop window -> idled despite never receiving Stop.
        r.expire_stale(46_001, 45_000);
        assert_eq!(r.aggregate(), AggregateState::Idle);
    }

    #[test]
    fn fresh_activity_resets_the_stall_clock() {
        let mut r = Roster::new();
        r.apply_raw_at(&hook("PreToolUse", "s1"), 1_000).unwrap();
        r.apply_raw_at(&hook("PostToolUse", "s1"), 40_000).unwrap(); // new activity
        r.expire_stale(50_000, 45_000); // only 10s since last activity
        assert_eq!(r.aggregate(), AggregateState::Working);
    }

    #[test]
    fn registry_enumerates_and_drops_gone_sessions() {
        let mut r = Roster::new();
        r.ensure_idle("live-1", 0);
        r.ensure_idle("live-2", 0);
        assert_eq!(r.len(), 2);
        assert_eq!(r.aggregate(), AggregateState::Idle);
        // live-1 goes to work, then its process disappears from the registry.
        r.apply_raw_at(&hook("PreToolUse", "live-1"), 100).unwrap();
        let mut live = BTreeSet::new();
        live.insert("live-2".to_string());
        r.retain_live(&live);
        assert_eq!(r.state_of("live-1"), None, "gone session dropped even though it was working");
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn ensure_idle_does_not_disturb_a_working_session() {
        let mut r = Roster::new();
        r.apply_raw_at(&hook("PreToolUse", "s1"), 100).unwrap();
        r.ensure_idle("s1", 200); // registry re-scan should not reset it
        assert_eq!(r.state_of("s1"), Some(SessionState::Working));
    }

    /// The seam test, replaying the ACTUAL recorded fixture through the real parser.
    #[test]
    fn replays_recorded_fixture_file() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tracker/assets/spike2-hook-events.sample.jsonl"
        );
        let data = std::fs::read_to_string(path).expect("recorded fixture is present");
        let mut roster = Roster::new();
        let mut replayed = 0;
        for line in data.lines().filter(|l| !l.trim().is_empty()) {
            let wrapper: serde_json::Value = serde_json::from_str(line).expect("fixture line is JSON");
            let payload = wrapper.get("payload").expect("fixture line has a payload");
            roster.apply_raw(&serde_json::to_string(payload).unwrap()).expect("recorded payload applies");
            replayed += 1;
        }
        assert!(replayed >= 5, "replayed {replayed} recorded events");
        assert_ne!(roster.aggregate(), AggregateState::NoSessions);
    }
}
