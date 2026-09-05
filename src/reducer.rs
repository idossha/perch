use crate::model::{Event, PaneRecord, ParsedEvent, State};

/// Apply a parsed event to a pane record, in place.
///
/// Returns `true` when the record's state changed.
pub fn apply(rec: &mut PaneRecord, parsed: &ParsedEvent, now: &str) -> bool {
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
    };

    let changed = rec.state != next;
    if changed {
        rec.state = next;
        rec.since = now.to_string();
    }
    changed
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Harness;

    fn ev(event: Event) -> ParsedEvent {
        ParsedEvent {
            event,
            session_id: None,
            cwd: None,
        }
    }

    fn rec() -> PaneRecord {
        PaneRecord::new("%1", Harness::Claude, "t0")
    }

    #[test]
    fn session_start_goes_idle() {
        let mut r = rec();
        assert!(apply(&mut r, &ev(Event::SessionStart), "t1"));
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
        assert!(!apply(&mut r, &ev(Event::UserPromptSubmit), "t2"));
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
