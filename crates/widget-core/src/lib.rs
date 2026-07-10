//! Pure state core for the Claude Code status widget.
//!
//! This crate is the single test seam for the whole widget (ADR: daemon boundary).
//! It takes the JSON a Claude Code hook delivers and folds it into per-session and
//! aggregate widget state. It has no I/O, no GUI, and no clock — everything here is
//! a pure function of the events it is given, so the daemon and the UI can be
//! exercised end-to-end by replaying recorded hook payloads (see
//! `tracker/assets/spike2-hook-events.sample.jsonl`).

use std::collections::BTreeMap;

use serde::Deserialize;

/// What a single Claude Code session is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// A prompt or tool call is in flight.
    Working,
    /// The turn finished; Claude is waiting for the next prompt.
    Idle,
    /// Claude is blocked on the user (an `AskUserQuestion` / permission prompt).
    /// Not yet produced by the skeleton — reserved for the alerts slice (#6).
    WaitingForInput,
    /// The turn died on a rate limit. Reserved for the alerts slice (#6).
    RateLimited,
}

/// The single state the card shows when several sessions are live.
///
/// Precedence (most attention-worthy wins): waiting > rate-limited > working > idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateState {
    NoSessions,
    Idle,
    Working,
    WaitingForInput,
    RateLimited,
}

/// A hook event, normalized from the raw payload into just what the state
/// machine cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    UserPromptSubmit,
    PreToolUse { tool_name: String, is_subagent: bool },
    PostToolUse { tool_name: String, is_subagent: bool },
    Stop,
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
    /// Present only when the event fires inside a subagent; used to keep a
    /// subagent's tool calls from hijacking the main-thread card (#6).
    #[serde(default)]
    agent_id: Option<String>,
}

/// Parse a single hook payload (one line of JSON as delivered on the hook's stdin
/// / HTTP body) into a [`ParsedEvent`].
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
        Some("SessionEnd") => HookEvent::SessionEnd,
        Some(other) => HookEvent::Other(other.to_string()),
        None => HookEvent::Other(String::new()),
    };
    Ok(ParsedEvent { session_id, event })
}

/// The transition function: given a session's current state (if any) and an event,
/// return its next state, or `None` if the session should be dropped from the roster.
///
/// Skeleton scope: prompt/tool activity means Working, `Stop` means Idle, `SessionEnd`
/// removes the session. `WaitingForInput` / `RateLimited` transitions (AskUserQuestion,
/// StopFailure) land in the alerts slice (#6).
pub fn next_state(current: Option<SessionState>, event: &HookEvent) -> Option<SessionState> {
    match event {
        HookEvent::UserPromptSubmit => Some(SessionState::Working),
        HookEvent::PreToolUse { .. } => Some(SessionState::Working),
        HookEvent::PostToolUse { .. } => Some(SessionState::Working),
        HookEvent::Stop => Some(SessionState::Idle),
        HookEvent::SessionEnd => None,
        HookEvent::Other(_) => current,
    }
}

/// The live set of sessions and their states.
#[derive(Debug, Default, Clone)]
pub struct Roster {
    sessions: BTreeMap<String, SessionState>,
}

impl Roster {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one parsed event, mutating the roster.
    pub fn apply(&mut self, ev: &ParsedEvent) {
        let current = self.sessions.get(&ev.session_id).copied();
        match next_state(current, &ev.event) {
            Some(state) => {
                self.sessions.insert(ev.session_id.clone(), state);
            }
            None => {
                self.sessions.remove(&ev.session_id);
            }
        }
    }

    /// Convenience: parse a raw hook payload and apply it. Malformed payloads are
    /// returned as an error and leave the roster untouched (the daemon logs and drops).
    pub fn apply_raw(&mut self, json: &str) -> Result<(), ParseError> {
        let ev = parse_hook(json)?;
        self.apply(&ev);
        Ok(())
    }

    pub fn state_of(&self, session_id: &str) -> Option<SessionState> {
        self.sessions.get(session_id).copied()
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// The single state the card renders, applying the precedence order.
    pub fn aggregate(&self) -> AggregateState {
        if self.sessions.is_empty() {
            return AggregateState::NoSessions;
        }
        let mut agg = AggregateState::Idle;
        for state in self.sessions.values() {
            match state {
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
        let parsed = parse_hook(json).unwrap();
        match parsed.event {
            HookEvent::PreToolUse { is_subagent, .. } => assert!(is_subagent),
            other => panic!("expected PreToolUse, got {other:?}"),
        }
    }

    #[test]
    fn missing_session_id_is_an_error() {
        let json = r#"{"hook_event_name":"Stop"}"#;
        assert!(matches!(parse_hook(json), Err(ParseError::MissingSessionId)));
    }

    #[test]
    fn malformed_json_is_an_error_and_does_not_panic() {
        assert!(matches!(parse_hook("not json"), Err(ParseError::Json(_))));
    }

    #[test]
    fn unknown_event_is_other_and_does_not_change_state() {
        let mut r = Roster::new();
        r.apply_raw(&hook("PreToolUse", "s1")).unwrap();
        assert_eq!(r.state_of("s1"), Some(SessionState::Working));
        r.apply_raw(&hook("Notification", "s1")).unwrap();
        // Other(...) leaves the session's state untouched in the skeleton.
        assert_eq!(r.state_of("s1"), Some(SessionState::Working));
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
    fn session_end_drops_the_session() {
        let mut r = Roster::new();
        r.apply_raw(&hook("UserPromptSubmit", "s1")).unwrap();
        assert_eq!(r.len(), 1);
        r.apply_raw(&hook("SessionEnd", "s1")).unwrap();
        assert!(r.is_empty());
        assert_eq!(r.aggregate(), AggregateState::NoSessions);
    }

    #[test]
    fn aggregate_prefers_working_over_idle() {
        let mut r = Roster::new();
        r.apply_raw(&hook("Stop", "s1")).unwrap(); // idle
        r.apply_raw(&hook("PreToolUse", "s2")).unwrap(); // working
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

    /// The seam test, replaying the ACTUAL recorded fixture through the real parser.
    /// Each line of the recording is `{"event":..,"payload":{..}}`; the daemon receives
    /// the `payload` object as a hook POST body, so we feed exactly that to `apply_raw`.
    /// Proves every recorded payload parses and drives the roster (AC5), end to end.
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
            let body = serde_json::to_string(payload).unwrap();
            roster.apply_raw(&body).expect("recorded payload parses and applies");
            replayed += 1;
        }
        assert!(replayed >= 5, "replayed {replayed} recorded events");
        // The recording contains no SessionEnd, so at least one session remains live.
        assert_ne!(roster.aggregate(), AggregateState::NoSessions);
    }

    /// A hand-built sequence mirroring spike #2, asserting the aggregate after each step.
    #[test]
    fn replays_spike2_sequence() {
        let session = "00000000-0000-0000-0000-000000000000";
        let sequence = [
            ("PreToolUse", AggregateState::Working),
            ("PostToolUse", AggregateState::Working),
            ("PreToolUse", AggregateState::Working),
            ("PostToolUse", AggregateState::Working),
            ("Stop", AggregateState::Idle),
            ("UserPromptSubmit", AggregateState::Working),
            ("PreToolUse", AggregateState::Working),
        ];
        let mut r = Roster::new();
        for (event, expected) in sequence {
            r.apply_raw(&hook(event, session)).unwrap();
            assert_eq!(r.aggregate(), expected, "after {event}");
        }
    }
}
