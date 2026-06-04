use agentloop::events::{Command, Event};
use agentloop::tui::AppState;
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
    s.apply(Event::QuestionRaised {
        item_id: "db".into(),
        label: "db-schema".into(),
        text: "SQLite or Postgres?".into(),
        context: "storage".into(),
    });
    assert_eq!(s.jobs.len(), 1);
    assert_eq!(s.inbox.len(), 1);
    assert_eq!(s.inbox[0].item_id, "db");

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
fn standby_event_sets_flag() {
    use agentloop::events::Event;
    let mut s = agentloop::tui::AppState::new("g".into());
    s.apply(Event::EnteredStandby);
    assert!(s.standby);
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
fn typing_then_enter_answers_selected_question() {
    let mut s = start("g");
    s.apply(Event::QuestionRaised {
        item_id: "db".into(),
        label: "db".into(),
        text: "q?".into(),
        context: "".into(),
    });
    // Focus defaults to Inbox with the question selected: typing goes straight to input.
    for c in "yes".chars() {
        s.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        matches!(cmd, Some(Command::AnswerQuestion { ref item_id, ref text }) if item_id == "db" && text == "yes")
    );
    // Input is cleared after submit, so 'q' now quits.
    assert!(matches!(
        s.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        Some(Command::Quit)
    ));
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
fn typing_then_enter_adds_task_when_no_question() {
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
fn tab_switches_focus_and_empty_enter_opens_job_detail() {
    use std::path::PathBuf;
    let mut s = start("g");
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: Some(PathBuf::from("/tmp/x.log")),
    });
    // Default focus is Inbox; Tab moves it to Jobs.
    assert!(!s.focus_is_jobs());
    assert!(s
        .on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .is_none());
    assert!(s.focus_is_jobs());
    // Empty input + Enter on Jobs opens the detail view.
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
    s.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)); // focus Jobs
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

#[test]
fn target_label_tracks_focus_and_inbox() {
    let mut s = start("g");
    assert_eq!(s.input_target_label(), "Add task");
    s.apply(Event::QuestionRaised {
        item_id: "db".into(),
        label: "db".into(),
        text: "q?".into(),
        context: "".into(),
    });
    // Inbox focused (default) with a question -> answering.
    assert_eq!(s.input_target_label(), "Answering db");
    s.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)); // focus Jobs
    assert_eq!(s.input_target_label(), "Add task");
}
