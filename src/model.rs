use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
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

    /// `true` for the two terminal states a subagent can end in.
    pub fn is_finished(self) -> bool {
        matches!(self, State::Done | State::Ended)
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
    Stop {
        last_message: Option<String>,
    },
    NeedsInput {
        reason: String,
    },
    Completed,
    SessionEnd,
    /// A subagent started under this pane's session.
    SubagentStart {
        agent_type: Option<String>,
    },
    /// A subagent finished; the message is its final assistant message.
    ///
    /// `agent_type` is Claude's role label, and is absent for a resumed or an
    /// internal helper agent — the reducer uses that to tell a stop it missed
    /// the start of from a helper it should never have tracked.
    SubagentStop {
        last_message: Option<String>,
        agent_type: Option<String>,
    },
    /// The parent sent a message to an existing background subagent
    /// (`SendMessage`), which resumes it. Claude fires no `SubagentStart` for
    /// a resume, so this is the only signal that the child is running again.
    SubagentResume {
        id: String,
    },
    /// A tool is about to run: the agent is working, whatever it was doing
    /// before. Only ever clears a stale `needs_input`.
    ToolUse,
    /// Understood, recorded in the event log, and deliberately not a state
    /// change — Claude's `idle_prompt` nudge, `auth_success`, quota notices.
    Observed {
        label: String,
    },
    /// The agent asked the human something through its `AskUserQuestion`
    /// tool: a `needs_input` that perch knows the text of, and — from the
    /// `--ask` hook — can answer on the harness's behalf.
    Question {
        tool_use_id: String,
        questions: Vec<Question>,
    },
}

/// One choice offered by a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// One question out of the one to four an `AskUserQuestion` call carries.
///
/// `kind` is Claude's: `choice` (options), `text` (a free line) or `number`
/// (a slider perch has no control for, so it is handed back to the harness).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub header: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
}

fn default_kind() -> String {
    "choice".to_string()
}

impl Question {
    /// Parse a tool input's `questions` array — the shape Claude's
    /// `AskUserQuestion` and perch's pi `ask_user` tool share. Entries without
    /// a `question` string are dropped; an empty result means "no question".
    pub fn parse_list(tool_input: &serde_json::Value) -> Vec<Question> {
        let Some(list) = tool_input.get("questions").and_then(|q| q.as_array()) else {
            return Vec::new();
        };
        let text = |v: &serde_json::Value, k: &str| {
            v.get(k)
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        };
        list.iter()
            .filter_map(|q| {
                let question = text(q, "question")?;
                let options = q
                    .get("options")
                    .and_then(|o| o.as_array())
                    .map(|os| {
                        os.iter()
                            .filter_map(|o| {
                                Some(QuestionOption {
                                    label: text(o, "label")?,
                                    description: text(o, "description").unwrap_or_default(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(Question {
                    question,
                    header: text(q, "header").unwrap_or_default(),
                    kind: text(q, "kind").unwrap_or_else(default_kind),
                    options,
                    multi_select: q
                        .get("multiSelect")
                        .and_then(|m| m.as_bool())
                        .unwrap_or(false),
                })
            })
            .collect()
    }
}

/// A stable key for a question set: the same questions give the same key
/// whichever hook event carried them. `PreToolUse` has a `tool_use_id`;
/// `PermissionRequest` does not, and in auto mode it is the only event that
/// fires — so a defer on one has to be recognised by the other.
pub fn question_key(questions: &[Question]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for q in questions {
        h.update(q.question.as_bytes());
        h.update([0]);
        for o in &q.options {
            h.update(o.label.as_bytes());
            h.update([1]);
        }
        h.update([2]);
    }
    let hex = format!("{:x}", h.finalize());
    format!("q-{}", &hex[..12])
}

/// A question the `--ask` hook is waiting on, kept on the pane record so the
/// dashboard can draw a form for it. `deadline` is when the hook gives up and
/// lets the harness draw its own dialog; past it the question is stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingQuestion {
    pub tool_use_id: String,
    pub questions: Vec<Question>,
    pub asked_at: String,
    pub deadline: String,
}

impl PendingQuestion {
    /// `true` while the hook that asked is still waiting for an answer.
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        DateTime::parse_from_rfc3339(&self.deadline)
            .map(|d| d.with_timezone(&Utc) > now)
            .unwrap_or(false)
    }
}

/// What the dashboard hands back to the waiting hook, as one file.
///
/// `answers` is keyed by question text — the shape Claude's tool expects —
/// with a multi-select joined by `, `. `defer` means "let the harness draw
/// its own dialog": the hook returns with no decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    #[serde(default)]
    pub pane: String,
    pub tool_use_id: String,
    #[serde(default)]
    pub answers: BTreeMap<String, String>,
    #[serde(default)]
    pub defer: bool,
}

impl Answer {
    pub fn defer(pane: &str, tool_use_id: &str) -> Self {
        Answer {
            pane: pane.to_string(),
            tool_use_id: tool_use_id.to_string(),
            answers: BTreeMap::new(),
            defer: true,
        }
    }
}

impl Event {
    pub fn kind(&self) -> &str {
        match self {
            Event::SessionStart => "session_start",
            Event::UserPromptSubmit => "user_prompt_submit",
            Event::Stop { .. } => "stop",
            Event::NeedsInput { .. } => "needs_input",
            Event::Completed => "completed",
            Event::SessionEnd => "session_end",
            Event::SubagentStart { .. } => "subagent_start",
            Event::SubagentStop { .. } => "subagent_stop",
            Event::SubagentResume { .. } => "subagent_resume",
            Event::ToolUse => "tool_use",
            Event::Observed { label } => label,
            Event::Question { .. } => "question",
        }
    }
}

/// What an adapter extracts from one raw hook payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvent {
    pub event: Event,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    /// Set when the harness attributed the event to a subagent of the session;
    /// the reducer then folds it into the parent record's `children`.
    pub agent_id: Option<String>,
}

impl ParsedEvent {
    /// A parent-session event: no subagent attribution.
    pub fn top_level(event: Event, session_id: Option<String>, cwd: Option<String>) -> Self {
        ParsedEvent {
            event,
            session_id,
            cwd,
            agent_id: None,
        }
    }
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
    /// Where the pane was in tmux the last time the hook looked: the window
    /// name, with `.<pane_index>` when the window was split. Kept so an
    /// `ended` row can still say where it used to live.
    #[serde(default)]
    pub location: Option<String>,
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
    /// The question the `--ask` hook is waiting on, if any.
    #[serde(default)]
    pub question: Option<PendingQuestion>,
    /// Key of the last question set handed back to the harness, so the next
    /// hook event for the same set (Claude asks twice: `PreToolUse`, then
    /// `PermissionRequest` for the dialog) is handed back too.
    #[serde(default)]
    pub deferred_question: Option<String>,
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
    /// `true` when at least one subagent is still running under this pane.
    pub fn has_running_children(&self) -> bool {
        self.children
            .iter()
            .any(|c| matches!(c.state, State::Working | State::Starting))
    }

    /// The state the user is shown, which is not always the pane's own.
    ///
    /// Claude Code runs subagents in the background: the main agent's turn
    /// ends — `Stop` fires — while its subagents keep working, and it is woken
    /// again when each finishes. A pane whose own last event was `Stop` but
    /// whose children are still running is therefore not waiting on the human;
    /// it is *delegating*, and delegating is a kind of working.
    pub fn effective_state(&self) -> State {
        if matches!(self.state, State::Done | State::Idle) && self.has_running_children() {
            State::Working
        } else {
            self.state
        }
    }

    /// `true` when [`effective_state`](Self::effective_state) is working only
    /// because subagents are: the word for the state cell is `delegating`.
    pub fn is_delegating(&self) -> bool {
        self.state != State::Working && self.effective_state() == State::Working
    }

    pub fn new(pane: &str, harness: Harness, now: &str) -> Self {
        PaneRecord {
            pane: pane.to_string(),
            harness,
            session_id: None,
            cwd: None,
            project: None,
            branch: None,
            location: None,
            state: State::Starting,
            since: now.to_string(),
            last_message: None,
            title: None,
            pid: None,
            last_sound_ms: None,
            children: Vec::new(),
            question: None,
            deferred_question: None,
        }
    }

    /// The question a popup can still answer: pending, and the hook that
    /// asked has not given up yet.
    pub fn live_question(&self, now: DateTime<Utc>) -> Option<&PendingQuestion> {
        self.question.as_ref().filter(|q| q.is_live(now))
    }

    /// `true` when the pane is blocked on a *question* — perch's popup or,
    /// after a hand-back, the harness's own dialog — rather than on a
    /// permission prompt. The board's state cell says `question` for it.
    pub fn is_question(&self) -> bool {
        self.state == State::NeedsInput
            && (self.question.is_some() || self.deferred_question.is_some())
    }
}
