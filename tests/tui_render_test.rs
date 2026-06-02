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

#[test]
fn jobs_render_above_inbox_full_width() {
    let mut s = AppState::new("goal".into());
    s.apply(Event::JobDispatched {
        id: "it-1".into(), label: "scaffold".into(), tool: "codex".into(),
        model: "gpt-5".into(), log_path: None,
    });
    s.apply(Event::QuestionRaised {
        item_id: "db".into(), label: "db".into(), text: "q?".into(), context: "".into(),
    });

    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();

    let jobs = find(&term, "Jobs").expect("Jobs pane rendered");
    let inbox = find(&term, "Inbox").expect("Inbox pane rendered");
    // Vertical stacking: Jobs title is on an earlier row than Inbox title.
    assert!(jobs.0 < inbox.0, "Jobs ({jobs:?}) is above Inbox ({inbox:?})");
}

#[test]
fn status_bar_shows_total_time() {
    let s = AppState::new("goal".into());
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(find(&term, "\u{23f1}").is_some(), "status bar shows the ⏱ total-time glyph");
}
