use serde::{Deserialize, Serialize};

/// Which coding agent produced an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
    Pi,
}

impl Harness {
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
        }
    }
}

impl std::str::FromStr for Harness {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "claude" => Ok(Harness::Claude),
            "codex" => Ok(Harness::Codex),
            "pi" => Ok(Harness::Pi),
            other => Err(format!("unknown harness: {other}")),
        }
    }
}

/// Lifecycle state of a pane, as understood by the reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Starting,
    Working,
    Done,
    NeedsInput,
    Idle,
    Ended,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Working => "working",
            State::Done => "done",
            State::NeedsInput => "needs_input",
            State::Idle => "idle",
            State::Ended => "ended",
        }
    }

    /// Sort rank for the TUI / `next`: needs_input, done, working, idle, ...
    pub fn rank(self) -> u8 {
        match self {
            State::NeedsInput => 0,
            State::Done => 1,
            State::Working => 2,
            State::Starting => 3,
            State::Idle => 4,
            State::Ended => 5,
        }
    }
}

/// A harness-neutral event, produced by an adapter from raw harness JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    SessionStart,
    UserPromptSubmit,
    Stop { last_message: Option<String> },
    NeedsInput { reason: String },
    Completed,
    SessionEnd,
}

impl Event {
    pub fn kind(&self) -> &'static str {
        match self {
            Event::SessionStart => "session_start",
            Event::UserPromptSubmit => "user_prompt_submit",
            Event::Stop { .. } => "stop",
            Event::NeedsInput { .. } => "needs_input",
            Event::Completed => "completed",
            Event::SessionEnd => "session_end",
        }
    }
}

/// What an adapter extracts from one raw hook payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvent {
    pub event: Event,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

/// The persisted per-pane record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneRecord {
    pub pane: String,
    pub harness: Harness,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    pub state: State,
    /// RFC3339 timestamp of the last state change.
    pub since: String,
    #[serde(default)]
    pub last_message: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    /// Last sound played, epoch millis, used for the per-pane cooldown.
    #[serde(default)]
    pub last_sound_ms: Option<i64>,
    /// Subagents the harness reported for this pane (Claude `agent_id`, Codex SubagentStart).
    /// They live in-process with the parent, so jumping to one lands on the parent pane.
    #[serde(default)]
    pub children: Vec<Subagent>,
}

/// One subagent under a pane. `id` is the harness's agent id; `agent_type` is its
/// role label when the harness gives one (Claude `agent_type`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subagent {
    pub id: String,
    #[serde(default)]
    pub agent_type: Option<String>,
    pub state: State,
    /// RFC3339 timestamp of the last state change.
    pub since: String,
    #[serde(default)]
    pub last_message: Option<String>,
}

impl PaneRecord {
    pub fn new(pane: &str, harness: Harness, now: &str) -> Self {
        PaneRecord {
            pane: pane.to_string(),
            harness,
            session_id: None,
            cwd: None,
            project: None,
            branch: None,
            state: State::Starting,
            since: now.to_string(),
            last_message: None,
            title: None,
            pid: None,
            last_sound_ms: None,
            children: Vec::new(),
        }
    }
}
