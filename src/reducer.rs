use crate::model::{Event, PaneRecord, ParsedEvent, State, Subagent};
use crate::store;

/// Finished subagents are kept this long, so a `done` child stays visible for
/// a moment after the run that produced it.
const CHILD_TTL_SECS: i64 = 600;

/// What applying an event asks the caller to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Applied {
    /// The pane's own state changed: worth a sound, a tmux cue and a log line.
    pub parent_changed: bool,
    /// The sound key to play, if any. A subagent event only ever asks for
    /// `needs_input`, and only when the harness said the human is blocked.
    pub sound: Option<&'static str>,
}

/// Apply a parsed event to a pane record, in place.
///
/// An event carrying `agent_id` updates the pane's `children` and leaves the
/// pane's own state alone; everything else is a parent transition.
pub fn apply(rec: &mut PaneRecord, parsed: &ParsedEvent, now: &str) -> Applied {
    if let Some(agent_id) = &parsed.agent_id {
        let out = apply_child(rec, agent_id, parsed, now);
        prune_children(rec, now);
        return out;
    }
    if let Some(sid) = &parsed.session_id {
        rec.session_id = Some(sid.clone());
    }
    if let Some(cwd) = &parsed.cwd {
        rec.cwd = Some(cwd.clone());
        rec.project = project_of(cwd);
    }

    let next = match &parsed.event {
        Event::SessionStart => State::Idle,
        Event::UserPromptSubmit => State::Working,
        Event::Stop { last_message } => {
            if let Some(m) = last_message {
                rec.last_message = Some(one_line(m));
            }
            State::Done
        }
        Event::NeedsInput { reason } => {
            rec.last_message = Some(one_line(reason));
            State::NeedsInput
        }
        Event::Completed => State::Done,
        Event::SessionEnd => State::Ended,
        // A subagent event that names no child moves nothing.
        Event::SubagentStart { .. } | Event::SubagentStop { .. } => return Applied::default(),
    };

    // A new turn, or the end of one, retires the subagents it spawned.
    if matches!(parsed.event, Event::UserPromptSubmit | Event::Stop { .. }) {
        rec.children.retain(|c| c.state != State::Done);
    }
    prune_children(rec, now);

    let changed = rec.state != next;
    if changed {
        rec.state = next;
        rec.since = now.to_string();
    }
    Applied {
        parent_changed: changed,
        sound: changed.then(|| sound_for(next)).flatten(),
    }
}

fn sound_for(state: State) -> Option<&'static str> {
    match state {
        State::Done => Some("done"),
        State::NeedsInput => Some("needs_input"),
        _ => None,
    }
}

/// Fold a subagent event into the parent record's `children`.
fn apply_child(rec: &mut PaneRecord, agent_id: &str, parsed: &ParsedEvent, now: &str) -> Applied {
    let (state, message, agent_type) = match &parsed.event {
        Event::SubagentStart { agent_type } => (State::Working, None, agent_type.clone()),
        Event::SubagentStop { last_message } => (
            State::Done,
            last_message.as_ref().map(|m| one_line(m)),
            None,
        ),
        Event::Stop { last_message } => (
            State::Done,
            last_message.as_ref().map(|m| one_line(m)),
            None,
        ),
        Event::NeedsInput { reason } => (State::NeedsInput, Some(one_line(reason)), None),
        Event::SessionEnd => (State::Ended, None, None),
        // A subagent has no session of its own to start or prompt.
        _ => return Applied::default(),
    };

    let sound =
        matches!(&parsed.event, Event::NeedsInput { reason } if reason == "agent_needs_input")
            .then_some("needs_input");

    match rec.children.iter_mut().find(|c| c.id == agent_id) {
        Some(child) => {
            if child.state != state {
                child.state = state;
                child.since = now.to_string();
            }
            if message.is_some() {
                child.last_message = message;
            }
            if agent_type.is_some() {
                child.agent_type = agent_type;
            }
        }
        None => rec.children.push(Subagent {
            id: agent_id.to_string(),
            agent_type,
            state,
            since: now.to_string(),
            last_message: message,
        }),
    }
    Applied {
        parent_changed: false,
        sound,
    }
}

/// Drop finished subagents older than the TTL. Called on every write, so a
/// pane that goes quiet still sheds its children on the next event.
fn prune_children(rec: &mut PaneRecord, now: &str) {
    let Ok(now) = chrono::DateTime::parse_from_rfc3339(now) else {
        return;
    };
    let now = now.with_timezone(&chrono::Utc);
    rec.children.retain(|c| {
        !matches!(c.state, State::Done | State::Ended)
            || store::age_secs(&c.since, now) <= CHILD_TTL_SECS
    });
}

/// Last path component of a cwd, used as the project label.
pub fn project_of(cwd: &str) -> Option<String> {
    let trimmed = cwd.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    trimmed.rsplit('/').next().map(|s| s.to_string())
}

/// Collapse whitespace and cap length so the record stays small.
pub fn one_line(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&flat, 400)
}

/// Truncate on a character boundary, appending an ellipsis when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Helpers shared by the reducer's test modules.
#[cfg(test)]
pub mod tests_support {
    use crate::model::{Event, ParsedEvent};

    pub fn top(event: Event) -> ParsedEvent {
        ParsedEvent::top_level(event, None, None)
    }

    pub fn child(agent_id: &str, event: Event) -> ParsedEvent {
        ParsedEvent {
            event,
            session_id: None,
            cwd: None,
            agent_id: Some(agent_id.to_string()),
        }
    }

    pub fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    pub fn ago(secs: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::seconds(secs)).to_rfc3339()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Harness;

    fn ev(event: Event) -> ParsedEvent {
        ParsedEvent::top_level(event, None, None)
    }

    fn rec() -> PaneRecord {
        PaneRecord::new("%1", Harness::Claude, "t0")
    }

    #[test]
    fn session_start_goes_idle() {
        let mut r = rec();
        assert!(apply(&mut r, &ev(Event::SessionStart), "t1").parent_changed);
        assert_eq!(r.state, State::Idle);
        assert_eq!(r.since, "t1");
    }

    #[test]
    fn prompt_then_stop() {
        let mut r = rec();
        apply(&mut r, &ev(Event::UserPromptSubmit), "t1");
        assert_eq!(r.state, State::Working);
        apply(
            &mut r,
            &ev(Event::Stop {
                last_message: Some("all  done\nhere".into()),
            }),
            "t2",
        );
        assert_eq!(r.state, State::Done);
        assert_eq!(r.last_message.as_deref(), Some("all done here"));
    }

    #[test]
    fn needs_input_and_completed() {
        let mut r = rec();
        apply(
            &mut r,
            &ev(Event::NeedsInput {
                reason: "permission_prompt".into(),
            }),
            "t1",
        );
        assert_eq!(r.state, State::NeedsInput);
        apply(&mut r, &ev(Event::Completed), "t2");
        assert_eq!(r.state, State::Done);
    }

    #[test]
    fn session_end_ends() {
        let mut r = rec();
        apply(&mut r, &ev(Event::SessionEnd), "t1");
        assert_eq!(r.state, State::Ended);
    }

    #[test]
    fn unchanged_state_keeps_since() {
        let mut r = rec();
        apply(&mut r, &ev(Event::UserPromptSubmit), "t1");
        assert!(!apply(&mut r, &ev(Event::UserPromptSubmit), "t2").parent_changed);
        assert_eq!(r.since, "t1");
    }

    #[test]
    fn cwd_sets_project() {
        let mut r = rec();
        let mut p = ev(Event::SessionStart);
        p.cwd = Some("/Users/x/00_development/perch/".into());
        p.session_id = Some("abc".into());
        apply(&mut r, &p, "t1");
        assert_eq!(r.project.as_deref(), Some("perch"));
        assert_eq!(r.session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("héllo", 3), "h\u{e9}…");
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn ranks_order_needs_input_first() {
        let mut v = [State::Idle, State::Done, State::NeedsInput, State::Working];
        v.sort_by_key(|s| s.rank());
        assert_eq!(v[0], State::NeedsInput);
        assert_eq!(v[1], State::Done);
        assert_eq!(v[2], State::Working);
    }
}

#[cfg(test)]
mod subagent_tests {
    use super::tests_support::*;
    use super::*;
    use crate::model::{Harness, State};

    #[test]
    fn start_then_stop_tracks_one_child() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        let out = apply(
            &mut r,
            &child(
                "a-1",
                Event::SubagentStart {
                    agent_type: Some("Explore".into()),
                },
            ),
            "t1",
        );
        assert!(!out.parent_changed, "a subagent never moves the pane");
        assert_eq!(out.sound, None);
        assert_eq!(r.state, State::Starting);
        assert_eq!(r.children.len(), 1);
        assert_eq!(r.children[0].agent_type.as_deref(), Some("Explore"));
        assert_eq!(r.children[0].state, State::Working);

        apply(
            &mut r,
            &child(
                "a-1",
                Event::SubagentStop {
                    last_message: Some("found  it".into()),
                },
            ),
            "t2",
        );
        assert_eq!(r.children.len(), 1);
        assert_eq!(r.children[0].state, State::Done);
        assert_eq!(r.children[0].last_message.as_deref(), Some("found it"));
    }

    #[test]
    fn a_notification_blocks_the_child_and_sounds() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(
            &mut r,
            &child("a-1", Event::SubagentStart { agent_type: None }),
            "t1",
        );
        let out = apply(
            &mut r,
            &child(
                "a-1",
                Event::NeedsInput {
                    reason: "agent_needs_input".into(),
                },
            ),
            "t2",
        );
        assert_eq!(out.sound, Some("needs_input"));
        assert!(!out.parent_changed);
        assert_eq!(r.children[0].state, State::NeedsInput);

        // Any other notification type is silent.
        let out = apply(
            &mut r,
            &child(
                "a-2",
                Event::NeedsInput {
                    reason: "permission_prompt".into(),
                },
            ),
            "t3",
        );
        assert_eq!(out.sound, None);
    }

    #[test]
    fn a_new_turn_clears_done_children_but_keeps_running_ones() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(
            &mut r,
            &child("done", Event::SubagentStop { last_message: None }),
            &now(),
        );
        apply(
            &mut r,
            &child("busy", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        apply(&mut r, &top(Event::UserPromptSubmit), &now());
        assert_eq!(r.children.len(), 1);
        assert_eq!(r.children[0].id, "busy");
    }

    #[test]
    fn finished_children_are_pruned_after_ten_minutes() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(
            &mut r,
            &child("old", Event::SubagentStop { last_message: None }),
            &ago(700),
        );
        apply(
            &mut r,
            &child("fresh", Event::SubagentStop { last_message: None }),
            &ago(60),
        );
        // Any later write prunes.
        apply(
            &mut r,
            &child("busy", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        let ids: Vec<&str> = r.children.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["fresh", "busy"]);
    }

    #[test]
    fn an_unparseable_now_never_prunes() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(
            &mut r,
            &child("old", Event::SubagentStop { last_message: None }),
            &ago(700),
        );
        apply(
            &mut r,
            &child("busy", Event::SubagentStart { agent_type: None }),
            "not-a-time",
        );
        assert_eq!(r.children.len(), 2);
    }
}
