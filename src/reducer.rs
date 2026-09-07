use crate::model::{Event, PaneRecord, ParsedEvent, PendingQuestion, State, Subagent};
use crate::store;

/// Finished subagents are kept this long, so a `done` child stays visible for
/// a moment after the run that produced it.
const CHILD_TTL_SECS: i64 = 600;

/// Hard cap on subagents kept per pane. A fan-out of fifty is a real thing
/// Claude does; the record is an attention list, not a transcript of it.
pub const MAX_CHILDREN: usize = 20;

/// A subagent still claiming `working` after this long is a `SubagentStop` the
/// harness never sent. Background subagents legitimately outlive their
/// parent's turn, so time is the only backstop left.
pub const CHILD_MAX_RUNNING_SECS: i64 = 2 * 60 * 60;

/// How long the `--ask` hook waits for an answer from the dashboard before it
/// returns empty-handed and the harness draws its own dialog. The installed
/// hook timeout must be longer than this, or the harness kills the hook
/// first. Overridable for tests with `PERCH_ASK_TIMEOUT_SECS`.
pub const ASK_WAIT_SECS: i64 = 59 * 60;

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
    // A resume is a parent-session payload that names a child: the pane's own
    // state is untouched, the child's is not.
    if let Event::SubagentResume { id } = &parsed.event {
        let id = id.clone();
        let out = apply_child(rec, &id, parsed, now);
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
        // A question is a `needs_input` perch knows the text of. The record
        // keeps what was asked so the dashboard can draw a form under it.
        Event::Question {
            tool_use_id,
            questions,
        } => {
            if let Some(first) = questions.first() {
                rec.last_message = Some(one_line(&first.question));
            }
            rec.question = Some(PendingQuestion {
                tool_use_id: tool_use_id.clone(),
                questions: questions.clone(),
                asked_at: now.to_string(),
                deadline: deadline_from(now),
            });
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
        Event::SubagentStart { .. } | Event::SubagentStop { .. } | Event::SubagentResume { .. } => {
            return Applied::default()
        }
    };

    // A `Stop` does *not* retire children: Claude runs subagents in the
    // background, so the parent's turn ends while they keep going and it is
    // woken when each finishes. Only the session going away does.
    if matches!(parsed.event, Event::SessionEnd) {
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
    // Whatever moves the pane off `needs_input` retires the question: it was
    // answered in the pane, overtaken by a new prompt, or the session ended.
    if next != State::NeedsInput {
        rec.question = None;
        rec.deferred_question = None;
    }

    let changed = rec.state != next;
    if changed {
        rec.state = next;
        rec.since = now.to_string();
    }
    let mut sound = changed.then(|| sound_for(next)).flatten();
    // A question always chimes, even on a pane that was already blocked: it
    // is a new thing to answer, not the same wait continuing.
    if matches!(parsed.event, Event::Question { .. }) {
        sound = Some("needs_input");
    }
    // The turn ended, the job did not: the subagents it spawned are still
    // running and the pane will be woken when they finish. Chiming now would
    // send the user to a pane that is still busy. `needs_input` is untouched —
    // that one really is blocked on the human.
    if sound == Some("done") && rec.has_running_children() {
        sound = None;
    }
    Applied {
        parent_changed: changed,
        sound,
    }
}

/// The answer came through perch: the agent is unblocked this instant, not on
/// its next tool call. Silent — the human did this, there is nobody to tell.
pub fn answered(rec: &mut PaneRecord, now: &str) {
    rec.question = None;
    rec.deferred_question = None;
    rec.state = State::Working;
    rec.since = now.to_string();
}

/// `now` plus the ask wait, as RFC3339; an unparseable `now` yields itself,
/// which reads as already expired.
fn deadline_from(now: &str) -> String {
    let secs = ask_wait_secs();
    chrono::DateTime::parse_from_rfc3339(now)
        .map(|t| (t.with_timezone(&chrono::Utc) + chrono::Duration::seconds(secs)).to_rfc3339())
        .unwrap_or_else(|_| now.to_string())
}

/// [`ASK_WAIT_SECS`], or `PERCH_ASK_TIMEOUT_SECS` when set.
pub fn ask_wait_secs() -> i64 {
    std::env::var("PERCH_ASK_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(ASK_WAIT_SECS)
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
/// A backstop, not a rule: called when the session ends, when the pane is
/// `ended`, and for a child that has claimed `working` for over
/// [`CHILD_MAX_RUNNING_SECS`]. Without it a `SubagentStop` the harness never
/// sent leaves a child running forever, and its pane delegating forever.
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
    let known = rec.children.iter().any(|c| c.id == agent_id);
    let (state, message, agent_type) = match &parsed.event {
        Event::SubagentStart { agent_type } => (State::Working, None, agent_type.clone()),
        // A resume restarts a child perch may never have seen start: Claude
        // sends `SubagentStart` only for a fresh spawn.
        Event::SubagentResume { .. } => (State::Working, None, None),
        // A stop for an id perch does not know, with no `agent_type`, is one
        // of Claude's internal helper agents: they run constantly and produce
        // unpaired stops (310 of them in one day's capture). Inventing a child
        // for each would fill every pane with agents the user never asked for,
        // so the event is logged by the hook and dropped here. A stop that
        // does carry an `agent_type` is a real spawned agent whose start was
        // missed, and is worth recording as finished.
        Event::SubagentStop {
            agent_type: None, ..
        } if !known => return Applied::default(),
        Event::SubagentStop {
            last_message,
            agent_type,
        } => (
            State::Done,
            last_message.as_ref().map(|m| one_line(m)),
            agent_type.clone(),
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
            // A resume restarts the clock even when the child was already
            // `working`: the pane is delegating again from now.
            if child.state != state || matches!(parsed.event, Event::SubagentResume { .. }) {
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

/// Retire subagents that have claimed `working` for over
/// [`CHILD_MAX_RUNNING_SECS`], then drop finished ones older than the TTL.
/// Called on every write, so a pane that goes quiet still sheds its children
/// on the next event.
fn prune_children(rec: &mut PaneRecord, now_str: &str) {
    let Ok(now) = chrono::DateTime::parse_from_rfc3339(now_str) else {
        return;
    };
    let now = now.with_timezone(&chrono::Utc);
    retire_stale_children(rec, now_str, now);
    rec.children.retain(|c| {
        !matches!(c.state, State::Done | State::Ended)
            || store::age_secs(&c.since, now) <= CHILD_TTL_SECS
    });
}

/// Retire every running subagent older than [`CHILD_MAX_RUNNING_SECS`].
pub fn retire_stale_children(
    rec: &mut PaneRecord,
    now_str: &str,
    now: chrono::DateTime<chrono::Utc>,
) {
    for c in &mut rec.children {
        if matches!(c.state, State::Working | State::Starting)
            && store::age_secs(&c.since, now) > CHILD_MAX_RUNNING_SECS
        {
            c.state = State::Done;
            c.since = now_str.to_string();
        }
    }
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

    /// A `SubagentStop` for a real spawned agent: it carries an `agent_type`,
    /// so the reducer will record it even for an id it never saw start.
    pub fn stop(last_message: Option<&str>) -> Event {
        Event::SubagentStop {
            last_message: last_message.map(|s| s.to_string()),
            agent_type: Some("Explore".into()),
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

        apply(&mut r, &child("a-1", stop(Some("found  it"))), "t2");
        assert_eq!(r.children.len(), 1);
        assert_eq!(r.children[0].state, State::Done);
        assert_eq!(r.children[0].last_message.as_deref(), Some("found it"));
    }

    /// `SendMessage` to an id perch never saw start: Claude fires no
    /// `SubagentStart` for a resume, so the resume is the whole signal.
    #[test]
    fn a_resume_of_an_unknown_id_creates_a_working_child() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        let out = apply(
            &mut r,
            &top(Event::SubagentResume {
                id: "a41eb56e05dc8146f".into(),
            }),
            "t1",
        );
        assert!(!out.parent_changed, "a resume never moves the pane");
        assert_eq!(out.sound, None);
        assert_eq!(r.children.len(), 1);
        assert_eq!(r.children[0].id, "a41eb56e05dc8146f");
        assert_eq!(r.children[0].agent_type, None);
        assert_eq!(r.children[0].state, State::Working);
        assert_eq!(r.children[0].since, "t1");
    }

    #[test]
    fn a_resume_of_a_finished_child_puts_it_back_to_work() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(
            &mut r,
            &child(
                "a-1",
                Event::SubagentStart {
                    agent_type: Some("Explore".into()),
                },
            ),
            &now(),
        );
        apply(&mut r, &child("a-1", stop(Some("found it"))), &now());
        assert_eq!(r.children[0].state, State::Done);

        let t = now();
        apply(&mut r, &top(Event::SubagentResume { id: "a-1".into() }), &t);
        assert_eq!(r.children.len(), 1, "the same child, not a second one");
        assert_eq!(r.children[0].state, State::Working);
        assert_eq!(r.children[0].since, t);
        assert_eq!(
            r.children[0].agent_type.as_deref(),
            Some("Explore"),
            "a resume keeps what the spawn told us"
        );
        assert_eq!(r.children[0].last_message.as_deref(), Some("found it"));
    }

    /// The pane goes `done` while the resumed agent runs — Claude wakes the
    /// parent with a task notification — so the board must say delegating
    /// until the matching `SubagentStop`.
    #[test]
    fn a_resumed_child_keeps_the_pane_delegating_until_its_stop() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(&mut r, &top(Event::UserPromptSubmit), &now());
        apply(
            &mut r,
            &top(Event::SubagentResume { id: "a-1".into() }),
            &now(),
        );
        let out = apply_with(
            &mut r,
            &top(Event::Stop { last_message: None }),
            &now(),
            false,
        );
        assert_eq!(r.state, State::Done);
        assert_eq!(out.sound, None, "no chime while a resumed agent runs");
        assert!(r.is_delegating());
        assert_eq!(r.effective_state(), State::Working);

        apply(&mut r, &child("a-1", stop(Some("done at last"))), &now());
        assert_eq!(r.effective_state(), State::Done, "back to its own state");
        assert!(!r.is_delegating());
    }

    /// Claude's internal helper agents produce a constant stream of unpaired
    /// `SubagentStop`s with an empty `agent_type` — 310 in one day's capture.
    /// They are logged and dropped, never turned into children.
    #[test]
    fn a_helper_stop_for_an_unknown_id_is_ignored() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        let out = apply(
            &mut r,
            &child(
                "helper-1",
                Event::SubagentStop {
                    last_message: None,
                    agent_type: None,
                },
            ),
            &now(),
        );
        assert!(r.children.is_empty(), "{:?}", r.children);
        assert_eq!(out, Applied::default());

        // But a stop for a child perch is already tracking still lands, and so
        // does one that names an agent_type: that is a spawn whose start we
        // missed, not a helper.
        apply(
            &mut r,
            &top(Event::SubagentResume { id: "a-1".into() }),
            &now(),
        );
        apply(
            &mut r,
            &child(
                "a-1",
                Event::SubagentStop {
                    last_message: None,
                    agent_type: None,
                },
            ),
            &now(),
        );
        assert_eq!(r.children[0].state, State::Done);
        apply(&mut r, &child("a-2", stop(None)), &now());
        assert_eq!(r.children.len(), 2);
        assert_eq!(r.children[1].state, State::Done);
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
        apply(&mut r, &child("done", stop(None)), &now());
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
        apply(&mut r, &child("old", stop(None)), &ago(700));
        apply(&mut r, &child("fresh", stop(None)), &ago(60));
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
    fn a_parent_stop_leaves_running_children_alone_and_stays_silent() {
        // Claude runs subagents in the background: the parent's turn ends and
        // they keep going, so the pane is delegating, not waiting on anyone.
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        for id in ["a", "b"] {
            apply(
                &mut r,
                &child(id, Event::SubagentStart { agent_type: None }),
                &now(),
            );
        }
        let out = apply_with(
            &mut r,
            &top(Event::Stop { last_message: None }),
            &now(),
            false,
        );
        assert_eq!(r.state, State::Done, "the pane's own turn did end");
        assert_eq!(out.sound, None, "no chime: the job is not done");
        assert!(
            r.children.iter().all(|c| c.state == State::Working),
            "{:?}",
            r.children
        );
        assert_eq!(r.effective_state(), State::Working);
        assert!(r.is_delegating());

        // The last child finishing is still silent: the parent is woken by the
        // task notification and its next Stop chimes normally.
        apply(&mut r, &child("a", stop(None)), &now());
        assert_eq!(r.effective_state(), State::Working, "b is still running");
        let out = apply(&mut r, &child("b", stop(None)), &now());
        assert_eq!(out.sound, None);
        assert!(!out.parent_changed);
        assert_eq!(r.effective_state(), State::Done, "own state, at last");
        assert!(!r.is_delegating());

        // And that next Stop does chime.
        r.state = State::Working;
        let out = apply_with(
            &mut r,
            &top(Event::Stop { last_message: None }),
            &now(),
            false,
        );
        assert_eq!(out.sound, Some("done"));
    }

    #[test]
    fn a_parent_needs_input_still_chimes_while_delegating() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(
            &mut r,
            &child("a", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        let out = apply(
            &mut r,
            &top(Event::NeedsInput {
                reason: "permission_prompt".into(),
            }),
            &now(),
        );
        assert_eq!(out.sound, Some("needs_input"));
        assert_eq!(r.effective_state(), State::NeedsInput);
    }

    #[test]
    fn a_child_running_for_two_hours_is_retired() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(
            &mut r,
            &child("stuck", Event::SubagentStart { agent_type: None }),
            &ago(CHILD_MAX_RUNNING_SECS + 60),
        );
        apply(
            &mut r,
            &child("fresh", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        let by = |id: &str| r.children.iter().find(|c| c.id == id).unwrap().state;
        assert_eq!(by("stuck"), State::Done, "a SubagentStop that never came");
        assert_eq!(by("fresh"), State::Working);
    }

    #[test]
    fn session_end_retires_every_running_child() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(
            &mut r,
            &child("a", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        let t = now();
        apply(&mut r, &top(Event::SessionEnd), &t);
        assert_eq!(r.state, State::Ended);
        assert_eq!(r.children[0].state, State::Done);
        assert_eq!(r.children[0].since, t);
        assert_eq!(r.effective_state(), State::Ended);
    }

    #[test]
    fn reaching_idle_by_any_path_clears_finished_children() {
        // A Stop you watched land: idle, and its finished children go — but a
        // running one stays, and keeps the pane delegating.
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        apply(&mut r, &child("done", stop(None)), &now());
        apply(
            &mut r,
            &child("busy", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        apply_with(
            &mut r,
            &top(Event::Stop { last_message: None }),
            &now(),
            true,
        );
        assert_eq!(r.state, State::Idle);
        let ids: Vec<&str> = r.children.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["busy"], "finished cleared, running kept");
        assert_eq!(
            r.effective_state(),
            State::Working,
            "idle by the seen rule, delegating to the user"
        );

        // `perch seen` and the seen reconciliation write `idle` directly; the
        // store re-applies the clearing rule on every read, and no longer
        // retires the running child.
        let mut r = PaneRecord::new("%2", Harness::Claude, "t0");
        apply(&mut r, &child("a", stop(None)), &now());
        apply(
            &mut r,
            &child("busy", Event::SubagentStart { agent_type: None }),
            &now(),
        );
        r.state = State::Idle;
        let out = store::reconcile(vec![r], &["%2".into()], chrono::Utc::now());
        let ids: Vec<&str> = out[0].children.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["busy"], "{:?}", out[0].children);
        assert_eq!(out[0].effective_state(), State::Working);
    }

    #[test]
    fn children_are_capped_at_twenty_oldest_finished_first() {
        let mut r = PaneRecord::new("%1", Harness::Claude, "t0");
        // Two finished, then eighteen running: the list is exactly full.
        for id in ["old-done", "newer-done"] {
            apply(&mut r, &child(id, stop(None)), &now());
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
        apply(&mut r, &child("old", stop(None)), &ago(700));
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
