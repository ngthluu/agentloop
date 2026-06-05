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
fn jobs_pane_renders_without_inbox() {
    let mut s = started("goal");
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: None,
    });

    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();

    assert!(find(&term, "Jobs").is_some(), "Jobs pane rendered");
    assert!(find(&term, "Inbox").is_none(), "Inbox pane removed");
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
    // Move the selection to the last job.
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

#[test]
fn customer_review_statuses_render_glyphs_not_question_mark() {
    let mut s = started("goal");
    s.apply(Event::JobDispatched {
        id: "task-1-customer".into(),
        label: "customer review".into(),
        tool: "claude".into(),
        model: "sonnet".into(),
        log_path: None,
    });
    s.apply(Event::JobStatus {
        id: "task-1-customer".into(),
        status: "approved".into(),
    });
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "✓ customer review").is_some(),
        "approved renders ✓"
    );
    assert!(
        find(&term, "? customer review").is_none(),
        "no ? for approved"
    );

    s.apply(Event::JobStatus {
        id: "task-1-customer".into(),
        status: "rejected".into(),
    });
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "✗ customer review").is_some(),
        "rejected renders ✗"
    );
}

#[test]
fn long_goal_is_ellipsized_and_counters_stay_visible() {
    let long_goal = "Implement a production-ready chat app, has 2 part: FE is a swift mac app, \
                     BE is rust-based. This chat app supports DM chat and group chat, support \
                     emoji picker, attach file. This chat app is secured, e2e encryption everything.";
    let s = started(long_goal);
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();

    assert!(find(&term, "\u{23f1}").is_some(), "⏱ running time visible");
    assert!(find(&term, "open:").is_some(), "open counter visible");
    assert!(find(&term, "iter").is_some(), "iteration counter visible");
    assert!(find(&term, "…").is_some(), "goal is ellipsized");
}

fn routed(goal: &str) -> AppState {
    use agentloop::tui::RoleEntry;
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
            model: String::new(),
            effort: "high".into(),
        },
    ]);
    s
}

#[test]
fn model_config_panel_renders_roles_values_and_defaults() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = routed("");
    s.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));

    let backend = TestBackend::new(100, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();

    assert!(find(&term, "Model routing").is_some(), "panel title rendered");
    assert!(find(&term, "architect").is_some());
    assert!(find(&term, "opus").is_some());
    assert!(
        find(&term, "(default)").is_some(),
        "unpinned model shows (default)"
    );
    assert!(find(&term, "[esc] close").is_some(), "close hint shown");
}

#[test]
fn footer_hints_advertise_ctrl_o() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    // Goal-entry screen.
    let s = routed("");
    let backend = TestBackend::new(100, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "ctrl-o").is_some(),
        "goal entry advertises the model picker"
    );

    // List view footer.
    let mut s = routed("g");
    s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let backend = TestBackend::new(120, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "[ctrl-o] models").is_some(),
        "list footer advertises the model picker"
    );
}
