use crate::model::{Event, PaneRecord, ParsedEvent, State, Subagent};
use crate::store;

/// Finished subagents are kept this long, so a `done` child stays visible for
/// a moment after the run that produced it.
const CHILD_TTL_SECS: i64 = 600;

/// Hard cap on subagents kept per pane. A fan-out of fifty is a real thing
/// Claude does; the record is an attention list, not a transcript of it.
pub const MAX_CHILDREN: usize = 20;

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
    apply_with(rec, parsed, now, false)
}

/// [`apply`], told whether the pane is the active pane of an attached client.
///
/// A turn that ends while you are looking at it is not news: it goes straight
/// to `idle` and stays silent. `done` means "finished while you were
/// elsewhere".
pub fn apply_with(
    rec: &mut PaneRecord,
    parsed: &ParsedEvent,
    now: &str,
    pane_focused: bool,
) -> Applied {
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
            if pane_focused {
                State::Idle
            } else {
                State::Done
            }
        }
        Event::NeedsInput { reason } => {
            rec.last_message = Some(one_line(reason));
            State::NeedsInput
        }
        Event::Completed => State::Done,
        // A tool call proves the agent is running again, which is the only way
        // a `needs_input` that was answered outside perch's view clears.
        Event::ToolUse => {
            if rec.state == State::NeedsInput {
                State::Working
            } else {
                return Applied::default();
            }
        }
        // Recorded in the event log, never a state change.
        Event::Observed { .. } => return Applied::default(),
        Event::SessionEnd => State::Ended,
        // A subagent event that names no child moves nothing.
        Event::SubagentStart { .. } | Event::SubagentStop { .. } => return Applied::default(),
    };

    // A parent turn ending implies its subagents ended: nothing survives the
    // turn that spawned it, whatever the harness forgot to send.
    if matches!(parsed.event, Event::Stop { .. }) {
        retire_children(rec, now);
    }
    // A pane that is idle has nothing outstanding, so its finished children
    // have nothing left to say. `idle` is reached by a Stop you watched, by
    // the seen reconciliation and by `perch seen`; all three end here or in
    // `store`, and all three clear.
    if next == State::Idle || matches!(parsed.event, Event::UserPromptSubmit) {
        clear_finished_children(rec);
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

/// Mark every still-running subagent finished, leaving its message alone.
///
/// Called when the parent's turn ends: a subagent runs inside that turn, so it
/// cannot outlive it. Without this a `SubagentStop` the harness never sent
/// leaves a child claiming `working` for hours.
pub fn retire_children(rec: &mut PaneRecord, now: &str) {
    for c in &mut rec.children {
        if c.state == State::Working || c.state == State::Starting {
            c.state = State::Done;
            c.since = now.to_string();
        }
    }
}

/// Drop every finished subagent. An `idle` pane keeps none.
pub fn clear_finished_children(rec: &mut PaneRecord) {
    rec.children.retain(|c| !c.state.is_finished());
}

/// Keep the child list under [`MAX_CHILDREN`], oldest finished first.
///
/// Ties are broken towards keeping what is still running: a working child is
/// only dropped when every finished one is already gone.
fn enforce_cap(rec: &mut PaneRecord) {
    while rec.children.len() > MAX_CHILDREN {
        let victim = rec
            .children
            .iter()
            .position(|c| c.state.is_finished())
            .unwrap_or(0);
        rec.children.remove(victim);
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
        None => {
            rec.children.push(Subagent {
                id: agent_id.to_string(),
                agent_type,
                state,
                since: now.to_string(),
                last_message: message,
            });
            enforce_cap(rec);
        }
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
    fn a_parent_stop_retires_every_running_child() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        for id in ["a", "b"] {
            apply(
                &mut r,
                &child(id, Event::SubagentStart { agent_type: None }),
                &now(),
            );
        }
        apply(
            &mut r,
            &child(
                "blocked",
                Event::NeedsInput {
                    reason: "agent_needs_input".into(),
                },
            ),
            &now(),
        );
        let t = now();
        apply(&mut r, &top(Event::Stop { last_message: None }), &t);
        assert_eq!(r.state, State::Done);
        assert_eq!(r.children.len(), 3, "nothing is dropped, only retired");
        assert!(
            r.children
                .iter()
                .filter(|c| ["a", "b"].contains(&c.id.as_str()))
                .all(|c| c.state == State::Done && c.since == t),
            "{:?}",
            r.children
        );
        // A child genuinely blocked on the human is not retired by the parent.
        let blocked = r.children.iter().find(|c| c.id == "blocked").unwrap();
        assert_eq!(blocked.state, State::NeedsInput);
        assert_eq!(
            blocked.last_message.as_deref(),
            Some("agent_needs_input"),
            "retiring never rewrites a message"
        );
    }

    #[test]
    fn reaching_idle_by_any_path_clears_finished_children() {
        // A Stop you watched land: idle, and the children it retired go.
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(
            &mut r,
            &child("a", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        apply_with(
            &mut r,
            &top(Event::Stop { last_message: None }),
            &now(),
            true,
        );
        assert_eq!(r.state, State::Idle);
        assert!(r.children.is_empty(), "{:?}", r.children);

        // `perch seen` and the seen reconciliation write `idle` directly; the
        // store re-applies the same rule on every read.
        let mut r = PaneRecord::new("%2", Harness::Claude, "t0");
        apply(
            &mut r,
            &child("a", Event::SubagentStop { last_message: None }),
            &now(),
        );
        apply(
            &mut r,
            &child("busy", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        r.state = State::Idle;
        let out = store::reconcile(vec![r], &["%2".into()], chrono::Utc::now());
        assert_eq!(out[0].children.len(), 1);
        assert_eq!(out[0].children[0].id, "busy");
    }

    #[test]
    fn children_are_capped_at_twenty_oldest_finished_first() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        // Two finished, then eighteen running: the list is exactly full.
        for id in ["old-done", "newer-done"] {
            apply(
                &mut r,
                &child(id, Event::SubagentStop { last_message: None }),
                &now(),
            );
        }
        for n in 0..18 {
            apply(
                &mut r,
                &child(&format!("w{n}"), Event::SubagentStart { agent_type: None }),
                &now(),
            );
        }
        assert_eq!(r.children.len(), MAX_CHILDREN);

        // The next one evicts the oldest finished child, not a running one.
        apply(
            &mut r,
            &child("w18", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        let ids: Vec<&str> = r.children.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(r.children.len(), MAX_CHILDREN);
        assert!(!ids.contains(&"old-done"), "{ids:?}");
        assert!(ids.contains(&"newer-done"), "{ids:?}");

        // With nothing finished left, the oldest running one goes.
        apply(
            &mut r,
            &child("w19", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        apply(
            &mut r,
            &child("w20", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        let ids: Vec<&str> = r.children.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(r.children.len(), MAX_CHILDREN);
        assert!(!ids.contains(&"newer-done"), "{ids:?}");
        assert!(
            !ids.contains(&"w0"),
            "the oldest running one is next: {ids:?}"
        );
        assert!(ids.contains(&"w20"), "{ids:?}");
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

/// herdr's state definitions, which perch adopted wholesale.
#[cfg(test)]
mod semantics_tests {
    use super::tests_support::*;
    use super::*;
    use crate::model::{Harness, State};

    fn pane(state: State) -> PaneRecord {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        r.state = state;
        r
    }

    #[test]
    fn an_idle_prompt_or_quota_notice_changes_nothing() {
        for label in ["idle_prompt", "auth_success", "quota_exceeded"] {
            let mut r = pane(State::Working);
            let out = apply(
                &mut r,
                &top(Event::Observed {
                    label: label.into(),
                }),
                "t1",
            );
            assert!(!out.parent_changed, "{label}");
            assert_eq!(out.sound, None, "{label}");
            assert_eq!(r.state, State::Working, "{label}");
        }
    }

    #[test]
    fn stop_is_done_when_you_are_elsewhere_and_idle_when_you_are_looking() {
        let mut away = pane(State::Working);
        let out = apply_with(
            &mut away,
            &top(Event::Stop { last_message: None }),
            "t1",
            false,
        );
        assert_eq!(away.state, State::Done);
        assert_eq!(out.sound, Some("done"));

        let mut watching = pane(State::Working);
        let out = apply_with(
            &mut watching,
            &top(Event::Stop {
                last_message: Some("finished".into()),
            }),
            "t1",
            true,
        );
        assert_eq!(watching.state, State::Idle, "seen means idle, not done");
        assert_eq!(out.sound, None, "no chime for a turn you watched end");
        assert_eq!(watching.last_message.as_deref(), Some("finished"));
    }

    #[test]
    fn needs_input_always_sounds() {
        let mut r = pane(State::Working);
        let out = apply(
            &mut r,
            &top(Event::NeedsInput {
                reason: "permission_prompt".into(),
            }),
            "t1",
        );
        assert_eq!(r.state, State::NeedsInput);
        assert_eq!(out.sound, Some("needs_input"));
    }

    #[test]
    fn a_tool_call_clears_a_stale_needs_input_and_nothing_else() {
        let mut blocked = pane(State::NeedsInput);
        let out = apply(&mut blocked, &top(Event::ToolUse), "t1");
        assert_eq!(blocked.state, State::Working);
        assert!(out.parent_changed);
        assert_eq!(out.sound, None);

        for state in [State::Idle, State::Done, State::Working, State::Ended] {
            let mut r = pane(state);
            let out = apply(&mut r, &top(Event::ToolUse), "t1");
            assert_eq!(r.state, state, "{state:?}");
            assert!(!out.parent_changed, "{state:?}");
        }
    }

    #[test]
    fn a_new_prompt_also_clears_needs_input() {
        let mut r = pane(State::NeedsInput);
        apply(&mut r, &top(Event::UserPromptSubmit), "t1");
        assert_eq!(r.state, State::Working);
    }
}
