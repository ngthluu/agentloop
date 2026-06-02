use agentloop::events::{Event, Command};
use agentloop::tui::AppState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn applies_events_to_view_model() {
    let mut s = AppState::new("build a todo app".into());
    s.apply(Event::JobDispatched { id: "it-1".into(), label: "scaffold".into(), tool: "codex".into(), model: "gpt-5".into(), log_path: None });
    s.apply(Event::QuestionRaised { item_id: "db".into(), label: "db-schema".into(), text: "SQLite or Postgres?".into(), context: "storage".into() });
    assert_eq!(s.jobs.len(), 1);
    assert_eq!(s.inbox.len(), 1);
    assert_eq!(s.inbox[0].item_id, "db");

    s.apply(Event::JobStatus { id: "it-1".into(), status: "merged".into() });
    assert_eq!(s.jobs.iter().find(|j| j.id == "it-1").unwrap().status, "merged");
}

#[test]
fn key_input_maps_to_commands() {
    let mut s = AppState::new("g".into());
    s.apply(Event::QuestionRaised { item_id: "db".into(), label: "db".into(), text: "q?".into(), context: "".into() });

    assert!(s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none()); // opens editor
    s.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    s.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    s.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::AnswerQuestion { ref item_id, ref text }) if item_id == "db" && text == "yes"));

    assert!(matches!(s.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)), Some(Command::Quit)));
}

#[test]
fn add_task_key_path_emits_command() {
    use agentloop::events::Command;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = agentloop::tui::AppState::new("g".into());

    assert!(s.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)).is_none()); // open editor
    for c in "due flag".chars() { s.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)); }
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::AddTask { ref request }) if request == "due flag"));
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
    assert_eq!(j.log_path.as_deref(), Some(std::path::Path::new("/tmp/item-it-1.log")));

    s.apply(Event::JobStatus { id: "it-1".into(), status: "merged".into() });
    let j = s.jobs.iter().find(|j| j.id == "it-1").unwrap();
    assert!(j.frozen.is_some(), "timer freezes on a terminal status");
}

#[test]
fn tab_toggles_focus_and_enter_opens_job_detail() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;
    let mut s = AppState::new("g".into());
    s.apply(Event::JobDispatched {
        id: "it-1".into(), label: "scaffold".into(), tool: "codex".into(),
        model: "gpt-5".into(), log_path: Some(PathBuf::from("/tmp/x.log")),
    });

    // Default focus is Inbox; Tab moves it to Jobs.
    assert!(!s.focus_is_jobs());
    assert!(s.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).is_none());
    assert!(s.focus_is_jobs());

    // Enter on the Jobs pane opens the detail view; no command emitted.
    assert!(s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());
    assert!(s.in_job_detail());

    // Esc returns to the list.
    assert!(s.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).is_none());
    assert!(!s.in_job_detail());
}

#[test]
fn enter_on_inbox_focus_still_answers() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = AppState::new("g".into());
    s.apply(Event::QuestionRaised { item_id: "db".into(), label: "db".into(), text: "q?".into(), context: "".into() });
    // Focus defaults to Inbox: Enter opens the answer editor (no command yet).
    assert!(s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());
    for c in "yes".chars() { s.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)); }
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::AnswerQuestion { ref item_id, .. }) if item_id == "db"));
}
