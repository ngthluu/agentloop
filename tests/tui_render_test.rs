use agentloop::events::Event;
use agentloop::tui::{self, AppState};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Find the (row, col) of the first cell where `needle` starts in the rendered buffer.
fn find(term: &Terminal<TestBackend>, needle: &str) -> Option<(u16, u16)> {
    let buf = term.backend().buffer();
    let area = buf.area();
    for y in 0..area.height {
        let mut row = String::new();
        for x in 0..area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        if let Some(idx) = row.find(needle) {
            return Some((y, idx as u16));
        }
    }
    None
}

fn started(goal: &str) -> AppState {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = AppState::new(goal.into());
    let _ = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)); // leave goal-entry
    s
}

#[test]
fn jobs_render_above_inbox_full_width() {
    let mut s = started("goal");
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: None,
    });
    s.apply(Event::QuestionRaised {
        item_id: "db".into(),
        label: "db".into(),
        text: "q?".into(),
        context: "".into(),
    });

    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();

    let jobs = find(&term, "Jobs").expect("Jobs pane rendered");
    let inbox = find(&term, "Inbox").expect("Inbox pane rendered");
    assert!(
        jobs.0 < inbox.0,
        "Jobs ({jobs:?}) is above Inbox ({inbox:?})"
    );
}

#[test]
fn status_bar_shows_total_time() {
    let s = started("goal");
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "\u{23f1}").is_some(),
        "status bar shows the ⏱ total-time glyph"
    );
}

#[test]
fn goal_entry_screen_shows_prompt_and_continue() {
    let s = AppState::new(String::new()); // starts on the goal-entry view
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "Continue").is_some(),
        "Continue button rendered"
    );
    assert!(
        find(&term, "build").is_some(),
        "entry prompt mentions what to build"
    );
}

#[test]
fn jobs_pane_scrolls_to_keep_selection_visible() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = started("goal");
    for i in 0..40 {
        s.apply(Event::JobDispatched {
            id: format!("it-{i}"),
            label: format!("jobLABEL{i}"),
            tool: "codex".into(),
            model: "gpt".into(),
            log_path: None,
        });
    }
    // Focus Jobs, then move the selection to the last job.
    s.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    for _ in 0..39 {
        s.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let backend = TestBackend::new(80, 20);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "jobLABEL39").is_some(),
        "selected (last) job scrolled into view"
    );
    assert!(
        find(&term, "jobLABEL0 ").is_none(),
        "first job scrolled out of view"
    );
}

#[test]
fn list_view_shows_input_target_label() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = AppState::new("g".into());
    let _ = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "Add task").is_some(),
        "bottom input shows the Add task target"
    );
}
