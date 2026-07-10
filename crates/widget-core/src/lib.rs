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

pub mod usage;
pub use usage::*;

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
    /// `error` is the error kind used for matching (docs v2.1.205 field is `error`;
    /// some tools call it `error_type` — we accept either).
    StopFailure { error: Option<String> },
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
    /// StopFailure error kind (v2.1.205 name).
    #[serde(default)]
    error: Option<String>,
    /// StopFailure error kind (alternate name some tools use).
    #[serde(default)]
    error_type: Option<String>,
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
        Some("StopFailure") => HookEvent::StopFailure { error: raw.error.or(raw.error_type) },
        Some("SessionEnd") => HookEvent::SessionEnd,
        Some(other) => HookEvent::Other(other.to_string()),
        None => HookEvent::Other(String::new()),
    };
    Ok(ParsedEvent { session_id, event })
}

/// Token usage of a single assistant message. The widget tracks the LATEST message's
/// usage as the session's "current context footprint" — cumulative sums balloon because
/// `cache_read_input_tokens` re-counts the same context on every message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

impl Usage {
    /// The message's total token footprint (input + output + both cache kinds) — a proxy
    /// for how large the session's context currently is.
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_input_tokens + self.cache_creation_input_tokens
    }
}

/// One assistant message parsed from a transcript line: which session, its model, and the
/// message's token usage (added to the session's running total).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptUpdate {
    pub session_id: String,
    pub model: Option<String>,
    pub usage: Usage,
}

#[derive(Deserialize)]
struct RawTranscript {
    #[serde(rename = "type")]
    typ: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize, Default)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

/// Parse one transcript JSONL line. Returns `Some` only for an assistant message that
/// carries a session id; every other line type (user, queue-operation, …) returns `None`.
/// Transcripts key the session as `sessionId` (camelCase), unlike hooks' `session_id`.
pub fn parse_transcript_line(json: &str) -> Option<TranscriptUpdate> {
    let raw: RawTranscript = serde_json::from_str(json).ok()?;
    if raw.typ.as_deref() != Some("assistant") {
        return None;
    }
    let session_id = raw.session_id?;
    let message = raw.message?;
    let u = message.usage.unwrap_or_default();
    Some(TranscriptUpdate {
        session_id,
        model: message.model,
        usage: Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_read_input_tokens: u.cache_read_input_tokens,
            cache_creation_input_tokens: u.cache_creation_input_tokens,
        },
    })
}

/// The last path component of a working directory (the "project"), or "?" if unknown.
pub fn project_label(cwd: Option<&str>) -> String {
    cwd.and_then(|c| c.rsplit('/').find(|s| !s.is_empty()))
        .unwrap_or("?")
        .to_string()
}

/// A per-session snapshot for the UI to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub session_id: String,
    pub project: String,
    pub state: SessionState,
    pub model: Option<String>,
    pub tokens: u64,
}

/// Per-session record: state + last-activity (for the backstop) + registry cwd + the
/// model and running token total harvested from the transcript.
#[derive(Debug, Clone)]
struct Session {
    state: SessionState,
    last_activity_ms: u64,
    cwd: Option<String>,
    model: Option<String>,
    usage: Usage,
}

impl Session {
    fn new_idle(now_ms: u64) -> Self {
        Session {
            state: SessionState::Idle,
            last_activity_ms: now_ms,
            cwd: None,
            model: None,
            usage: Usage::default(),
        }
    }
}

/// The transition an event drives, given the current state. `None` means "drop the session".
///
/// Skeleton scope: prompt/tool activity → Working; `Stop`/`StopFailure` → Idle;
/// `SessionEnd` → drop. (`WaitingForInput`/`RateLimited` land in #6.)
pub fn next_state(current: Option<SessionState>, event: &HookEvent) -> Option<SessionState> {
    match event {
        HookEvent::UserPromptSubmit => Some(SessionState::Working),
        HookEvent::PreToolUse { tool_name, is_subagent } => {
            // A main-thread AskUserQuestion means Claude is blocked on the user (this is
            // how "needs you" surfaces in the VS Code panel — see spike #3 / Claude-Familiar
            // #52). A subagent's AskUserQuestion must NOT hijack the main card.
            if tool_name == "AskUserQuestion" && !is_subagent {
                Some(SessionState::WaitingForInput)
            } else {
                Some(SessionState::Working)
            }
        }
        // A completed tool call (including the user answering the question) resumes work.
        HookEvent::PostToolUse { .. } => Some(SessionState::Working),
        HookEvent::Stop => Some(SessionState::Idle),
        HookEvent::StopFailure { error } => {
            if error.as_deref() == Some("rate_limit") {
                Some(SessionState::RateLimited)
            } else {
                Some(SessionState::Idle)
            }
        }
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
                // Update state in place so the session's cwd/model/usage survive.
                let entry = self
                    .sessions
                    .entry(ev.session_id.clone())
                    .or_insert_with(|| Session::new_idle(now_ms));
                entry.state = state;
                entry.last_activity_ms = now_ms;
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

    /// Ensure a (registry-discovered) session exists, as Idle, recording its cwd, without
    /// disturbing the state/model/usage of one we already track. Called on registry sync.
    pub fn ensure_session(&mut self, session_id: &str, cwd: Option<&str>, now_ms: u64) {
        let entry = self
            .sessions
            .entry(session_id.to_string())
            .or_insert_with(|| Session::new_idle(now_ms));
        if entry.cwd.is_none() {
            entry.cwd = cwd.map(str::to_string);
        }
    }

    /// Back-compat helper: ensure a session exists as Idle with no cwd.
    pub fn ensure_idle(&mut self, session_id: &str, now_ms: u64) {
        self.ensure_session(session_id, None, now_ms);
    }

    /// Apply an assistant message from the transcript: set the session's model and add the
    /// message's tokens to its running total. Ignored if the session isn't tracked (the
    /// registry is the authority for existence).
    pub fn apply_transcript(&mut self, update: &TranscriptUpdate) {
        if let Some(s) = self.sessions.get_mut(&update.session_id) {
            if update.model.is_some() {
                s.model = update.model.clone();
            }
            // Latest message = current context footprint (see Usage docs).
            s.usage = update.usage;
        }
    }

    /// Per-session snapshots for the UI, ordered by session id (stable rows).
    pub fn sessions_view(&self) -> Vec<SessionView> {
        self.sessions
            .iter()
            .map(|(id, s)| SessionView {
                session_id: id.clone(),
                project: project_label(s.cwd.as_deref()),
                state: s.state,
                model: s.model.clone(),
                tokens: s.usage.total(),
            })
            .collect()
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
    fn main_thread_ask_user_question_sets_waiting() {
        let mut r = Roster::new();
        let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"AskUserQuestion"}"#;
        r.apply_raw(json).unwrap();
        assert_eq!(r.state_of("s1"), Some(SessionState::WaitingForInput));
        assert_eq!(r.aggregate(), AggregateState::WaitingForInput);
    }

    #[test]
    fn subagent_ask_user_question_does_not_hijack() {
        let mut r = Roster::new();
        let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"AskUserQuestion","agent_id":"sub-1"}"#;
        r.apply_raw(json).unwrap();
        assert_eq!(r.state_of("s1"), Some(SessionState::Working), "subagent question must not set waiting");
    }

    #[test]
    fn answering_the_question_resumes_working() {
        let mut r = Roster::new();
        r.apply_raw(r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"AskUserQuestion"}"#).unwrap();
        assert_eq!(r.state_of("s1"), Some(SessionState::WaitingForInput));
        r.apply_raw(r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"AskUserQuestion"}"#).unwrap();
        assert_eq!(r.state_of("s1"), Some(SessionState::Working));
    }

    #[test]
    fn stop_failure_rate_limit_sets_rate_limited() {
        let mut r = Roster::new();
        r.apply_raw(r#"{"hook_event_name":"PreToolUse","session_id":"s1"}"#).unwrap();
        r.apply_raw(r#"{"hook_event_name":"StopFailure","session_id":"s1","error":"rate_limit"}"#).unwrap();
        assert_eq!(r.state_of("s1"), Some(SessionState::RateLimited));
        assert_eq!(r.aggregate(), AggregateState::RateLimited);
    }

    #[test]
    fn stop_failure_accepts_error_type_alias() {
        let mut r = Roster::new();
        r.apply_raw(r#"{"hook_event_name":"StopFailure","session_id":"s1","error_type":"rate_limit"}"#).unwrap();
        assert_eq!(r.state_of("s1"), Some(SessionState::RateLimited));
    }

    #[test]
    fn waiting_survives_the_stall_backstop() {
        // A pending question may sit for a long time while the user is away; the short
        // backstop (which only idles Working) must not clear it.
        let mut r = Roster::new();
        r.apply_raw_at(r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"AskUserQuestion"}"#, 1_000).unwrap();
        r.expire_stale(1_000_000, 45_000);
        assert_eq!(r.state_of("s1"), Some(SessionState::WaitingForInput));
    }

    #[test]
    fn aggregate_prefers_waiting_over_working() {
        let mut r = Roster::new();
        r.apply_raw(r#"{"hook_event_name":"PreToolUse","session_id":"a","tool_name":"Bash"}"#).unwrap(); // working
        r.apply_raw(r#"{"hook_event_name":"PreToolUse","session_id":"b","tool_name":"AskUserQuestion"}"#).unwrap(); // waiting
        assert_eq!(r.aggregate(), AggregateState::WaitingForInput);
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

    #[test]
    fn parses_assistant_transcript_line() {
        let line = r#"{"type":"assistant","sessionId":"s1","message":{"model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":20}}}"#;
        let u = parse_transcript_line(line).unwrap();
        assert_eq!(u.session_id, "s1");
        assert_eq!(u.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(u.usage.total(), 135);
    }

    #[test]
    fn non_assistant_transcript_lines_are_ignored() {
        assert!(parse_transcript_line(r#"{"type":"user","sessionId":"s1"}"#).is_none());
        assert!(parse_transcript_line(r#"{"type":"queue-operation"}"#).is_none());
        assert!(parse_transcript_line("not json").is_none());
    }

    #[test]
    fn apply_transcript_sets_model_and_reports_latest_footprint() {
        let mut r = Roster::new();
        r.ensure_session("s1", Some("/home/khalil/Desktop/claude-widget"), 0);
        let mk = |inp: u64| TranscriptUpdate {
            session_id: "s1".into(),
            model: Some("claude-fable-5".into()),
            usage: Usage { input_tokens: inp, ..Default::default() },
        };
        r.apply_transcript(&mk(100));
        r.apply_transcript(&mk(50));
        let view = r.sessions_view();
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].model.as_deref(), Some("claude-fable-5"));
        assert_eq!(view[0].tokens, 50, "shows the latest message's footprint, not a cache-inflated sum");
        assert_eq!(view[0].project, "claude-widget");
    }

    #[test]
    fn transcript_update_for_unknown_session_is_ignored() {
        let mut r = Roster::new();
        r.apply_transcript(&TranscriptUpdate {
            session_id: "ghost".into(),
            model: Some("m".into()),
            usage: Usage { input_tokens: 5, ..Default::default() },
        });
        assert!(r.is_empty(), "no session created for a transcript-only id");
    }

    #[test]
    fn hook_state_change_preserves_model_and_tokens() {
        let mut r = Roster::new();
        r.ensure_session("s1", Some("/x/proj"), 0);
        r.apply_transcript(&TranscriptUpdate {
            session_id: "s1".into(),
            model: Some("claude-opus-4-8".into()),
            usage: Usage { output_tokens: 42, ..Default::default() },
        });
        // A hook flips the session to working; model/tokens must survive.
        r.apply_raw(r#"{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"Bash"}"#).unwrap();
        let v = &r.sessions_view()[0];
        assert_eq!(v.state, SessionState::Working);
        assert_eq!(v.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(v.tokens, 42);
    }

    #[test]
    fn project_label_takes_basename() {
        assert_eq!(project_label(Some("/home/khalil/Desktop/claude-widget")), "claude-widget");
        assert_eq!(project_label(Some("/x/")), "x");
        assert_eq!(project_label(None), "?");
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
