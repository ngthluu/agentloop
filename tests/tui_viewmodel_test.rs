use agentloop::events::{Command, Event};
use agentloop::tui::{AppState, RoleEntry};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Drive an AppState past the goal-entry screen into the List view.
fn start(goal: &str) -> AppState {
    let mut s = AppState::new(goal.into());
    let _ = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)); // commit goal
    s
}

#[test]
fn applies_events_to_view_model() {
    let mut s = AppState::new("build a todo app".into());
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: None,
    });
    assert_eq!(s.jobs.len(), 1);

    s.apply(Event::JobStatus {
        id: "it-1".into(),
        status: "merged".into(),
    });
    assert_eq!(
        s.jobs.iter().find(|j| j.id == "it-1").unwrap().status,
        "merged"
    );
}

#[test]
fn standby_event_sets_flag_and_reason() {
    use agentloop::events::Event;
    let mut s = agentloop::tui::AppState::new("g".into());
    s.apply(Event::EnteredStandby {
        reason: "no progress (stall) · 3 open / 1 failed".into(),
    });
    assert!(s.standby);
    assert_eq!(s.standby_reason, "no progress (stall) · 3 open / 1 failed");
}

#[test]
fn standby_clears_when_the_loop_re_engages() {
    use agentloop::events::Event;
    let mut s = agentloop::tui::AppState::new("g".into());
    s.apply(Event::EnteredStandby {
        reason: "budget exhausted · 2 open / 0 failed".into(),
    });
    assert!(s.standby);
    // A new dispatch means the loop is working again: the banner must drop.
    s.apply(Event::JobDispatched {
        id: "manager".into(),
        label: "managing".into(),
        tool: "claude".into(),
        model: "sonnet".into(),
        log_path: None,
    });
    assert!(!s.standby);
}

#[test]
fn dispatch_starts_timer_and_stores_log_path_then_freezes() {
    use std::path::PathBuf;
    let mut s = AppState::new("g".into());
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: Some(PathBuf::from("/tmp/item-it-1.log")),
    });
    let j = s.jobs.iter().find(|j| j.id == "it-1").unwrap();
    assert!(j.started.is_some(), "timer starts on dispatch");
    assert!(j.frozen.is_none(), "not frozen while running");
    assert_eq!(
        j.log_path.as_deref(),
        Some(std::path::Path::new("/tmp/item-it-1.log"))
    );

    s.apply(Event::JobStatus {
        id: "it-1".into(),
        status: "merged".into(),
    });
    let j = s.jobs.iter().find(|j| j.id == "it-1").unwrap();
    assert!(j.frozen.is_some(), "timer freezes on a terminal status");
}

#[test]
fn goal_entry_commit_emits_start_run() {
    let mut s = AppState::new(String::new());
    assert!(s.in_goal_entry());
    // Empty input: Enter is a no-op (still on the entry screen).
    assert!(s
        .on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .is_none());
    assert!(s.in_goal_entry());
    for c in "build app".chars() {
        s.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::StartRun { ref goal }) if goal == "build app"));
    assert!(!s.in_goal_entry());
}

#[test]
fn goal_entry_prefill_commits_existing_goal() {
    let mut s = AppState::new("resume this goal".into());
    // Pre-filled with the existing goal; Enter commits it unchanged.
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::StartRun { ref goal }) if goal == "resume this goal"));
}

#[test]
fn shift_enter_inserts_newline_in_goal_entry() {
    let mut s = AppState::new(String::new());
    s.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    s.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    assert_eq!(s.input_buffer(), "a\nb");
    assert!(s.in_goal_entry(), "shift+enter does not commit");
}

#[test]
fn customer_review_statuses_freeze_timer() {
    for status in ["approved", "rejected"] {
        let mut s = AppState::new("g".into());
        s.apply(Event::JobDispatched {
            id: "task-1-customer".into(),
            label: "customer review".into(),
            tool: "claude".into(),
            model: "sonnet".into(),
            log_path: None,
        });
        s.apply(Event::JobStatus {
            id: "task-1-customer".into(),
            status: status.into(),
        });
        let j = s.jobs.iter().find(|j| j.id == "task-1-customer").unwrap();
        assert!(j.frozen.is_some(), "{status} must freeze the working timer");
        assert_eq!(j.status, status);
    }
}

#[test]
fn typing_then_enter_adds_task() {
    let mut s = start("g");
    // No questions: target is Add task.
    for c in "due flag".chars() {
        s.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::AddTask { ref request }) if request == "due flag"));
}

#[test]
fn q_quits_only_when_input_empty() {
    let mut s = start("g");
    // Empty input: 'q' quits.
    assert!(matches!(
        s.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        Some(Command::Quit)
    ));
    // With text, 'q' types a literal q.
    let mut s2 = start("g");
    s2.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(s2
        .on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .is_none());
    assert_eq!(s2.input_buffer(), "xq");
}

#[test]
fn empty_enter_opens_job_detail_and_esc_returns() {
    use std::path::PathBuf;
    let mut s = start("g");
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: Some(PathBuf::from("/tmp/x.log")),
    });
    // Empty input + Enter opens the detail view for the selected job.
    assert!(s
        .on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .is_none());
    assert!(s.in_job_detail());
    // Esc returns to the list.
    assert!(s
        .on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .is_none());
    assert!(!s.in_job_detail());
}

#[test]
fn q_quits_from_job_detail_when_input_empty() {
    use std::path::PathBuf;
    let mut s = start("g");
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: Some(PathBuf::from("/tmp/x.log")),
    });
    s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)); // open detail (empty input)
    assert!(s.in_job_detail());
    // q with empty input quits from detail.
    assert!(matches!(
        s.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        Some(Command::Quit)
    ));
    // But with text typed, q types a literal q (does not quit).
    s.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(s
        .on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .is_none());
    assert_eq!(s.input_buffer(), "xq");
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// An AppState seeded with two routing rows (sorted, as app.rs provides them).
fn routed(goal: &str) -> AppState {
    let mut s = AppState::new(goal.into());
    s.set_routing(vec![
        RoleEntry {
            role: "architect".into(),
            tool: "claude".into(),
            model: "opus".into(),
            effort: "high".into(),
        },
        RoleEntry {
            role: "builder".into(),
            tool: "codex".into(),
            model: String::new(), // unpinned: tool default
            effort: "high".into(),
        },
    ]);
    s
}

#[test]
fn ctrl_o_opens_model_config_and_esc_returns_to_the_previous_view() {
    // From goal entry…
    let mut s = routed("");
    assert!(s.in_goal_entry());
    assert!(s.on_key(ctrl('o')).is_none());
    assert!(s.in_model_config());
    assert!(s.on_key(key(KeyCode::Esc)).is_none());
    assert!(s.in_goal_entry(), "esc returns to where ctrl-o was pressed");

    // …and from the list view.
    let mut s = routed("g");
    s.on_key(key(KeyCode::Enter)); // commit goal -> List
    s.on_key(ctrl('o'));
    assert!(s.in_model_config());
    s.on_key(ctrl('o')); // ctrl-o also closes
    assert!(!s.in_model_config() && !s.in_goal_entry());
}

#[test]
fn arrows_move_the_cell_cursor_within_bounds() {
    let mut s = routed("");
    s.on_key(ctrl('o'));
    assert_eq!(s.model_selection(), (0, 0));
    s.on_key(key(KeyCode::Up)); // already at the top
    s.on_key(key(KeyCode::Left)); // already leftmost
    assert_eq!(s.model_selection(), (0, 0));
    s.on_key(key(KeyCode::Down));
    s.on_key(key(KeyCode::Right));
    s.on_key(key(KeyCode::Right));
    s.on_key(key(KeyCode::Right)); // clamped at effort
    assert_eq!(s.model_selection(), (1, 2));
    s.on_key(key(KeyCode::Down)); // clamped at the last row
    assert_eq!(s.model_selection(), (1, 2));
}

#[test]
fn enter_on_tool_cycles_claude_codex_and_emits_set_role() {
    let mut s = routed("");
    s.on_key(ctrl('o')); // row 0 = architect, col 0 = tool
    let cmd = s.on_key(key(KeyCode::Enter));
    assert!(
        matches!(
            cmd,
            Some(Command::SetRole { ref role, ref tool, ref model, ref effort })
                if role == "architect" && tool == "codex" && model == "opus" && effort == "high"
        ),
        "got {cmd:?}"
    );
    assert_eq!(s.model_rows()[0].tool, "codex");
    // Cycles back.
    let cmd = s.on_key(key(KeyCode::Enter));
    assert!(matches!(cmd, Some(Command::SetRole { ref tool, .. }) if tool == "claude"));
}

#[test]
fn editing_model_commits_on_enter_and_emits_set_role() {
    let mut s = routed("");
    s.on_key(ctrl('o'));
    s.on_key(key(KeyCode::Down)); // builder row
    s.on_key(key(KeyCode::Right)); // model column
    assert!(
        s.on_key(key(KeyCode::Enter)).is_none(),
        "enter starts the edit"
    );
    assert_eq!(
        s.model_edit_buffer(),
        Some(""),
        "unpinned model edits from empty"
    );
    for c in "gpt-5.5".chars() {
        s.on_key(key(KeyCode::Char(c)));
    }
    let cmd = s.on_key(key(KeyCode::Enter));
    assert!(
        matches!(
            cmd,
            Some(Command::SetRole { ref role, ref model, .. })
                if role == "builder" && model == "gpt-5.5"
        ),
        "got {cmd:?}"
    );
    assert_eq!(s.model_rows()[1].model, "gpt-5.5");
    assert_eq!(s.model_edit_buffer(), None, "edit closed after commit");
}

#[test]
fn esc_cancels_an_edit_without_committing() {
    let mut s = routed("");
    s.on_key(ctrl('o'));
    s.on_key(key(KeyCode::Right)); // architect / model
    s.on_key(key(KeyCode::Enter)); // edit "opus"
    s.on_key(key(KeyCode::Char('X')));
    assert!(s.on_key(key(KeyCode::Esc)).is_none());
    assert_eq!(s.model_rows()[0].model, "opus", "value unchanged");
    assert!(
        s.in_model_config(),
        "esc in an edit closes the edit, not the panel"
    );
}

#[test]
fn clearing_a_model_commits_empty_meaning_tool_default() {
    let mut s = routed("");
    s.on_key(ctrl('o'));
    s.on_key(key(KeyCode::Right)); // architect / model
    s.on_key(key(KeyCode::Enter)); // edit "opus"
    for _ in 0..4 {
        s.on_key(key(KeyCode::Backspace));
    }
    let cmd = s.on_key(key(KeyCode::Enter));
    assert!(
        matches!(cmd, Some(Command::SetRole { ref model, .. }) if model.is_empty()),
        "empty model = unset = tool default, got {cmd:?}"
    );
    assert_eq!(s.model_rows()[0].model, "");
}

#[test]
fn editing_effort_commits_on_enter_and_emits_set_role() {
    let mut s = routed("");
    s.on_key(ctrl('o'));
    s.on_key(key(KeyCode::Right));
    s.on_key(key(KeyCode::Right)); // effort column
    assert!(
        s.on_key(key(KeyCode::Enter)).is_none(),
        "enter starts the edit"
    );
    assert_eq!(s.model_edit_buffer(), Some("high"));
    for _ in 0..4 {
        s.on_key(key(KeyCode::Backspace));
    }
    for c in "low".chars() {
        s.on_key(key(KeyCode::Char(c)));
    }
    let cmd = s.on_key(key(KeyCode::Enter));
    assert!(
        matches!(
            cmd,
            Some(Command::SetRole { ref role, ref effort, .. })
                if role == "architect" && effort == "low"
        ),
        "got {cmd:?}"
    );
    assert_eq!(s.model_rows()[0].effort, "low");
}

#[test]
fn ctrl_o_from_job_detail_returns_to_job_detail() {
    let mut s = routed("g");
    s.on_key(key(KeyCode::Enter)); // commit goal -> List
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: String::new(),
        log_path: None,
    });
    s.on_key(key(KeyCode::Enter)); // open the selected job -> JobDetail
    assert!(s.in_job_detail());
    s.on_key(ctrl('o'));
    assert!(s.in_model_config());
    s.on_key(key(KeyCode::Esc));
    assert!(s.in_job_detail(), "esc returns to the job-detail view");
}
