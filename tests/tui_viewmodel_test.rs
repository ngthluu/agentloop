use agentloop::events::{Event, Command};
use agentloop::tui::AppState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn applies_events_to_view_model() {
    let mut s = AppState::new("build a todo app".into());
    s.apply(Event::JobDispatched { id: "it-1".into(), label: "scaffold".into(), tool: "codex".into(), model: "gpt-5".into() });
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
