use agentloop::cli::commit_goal;
use agentloop::cli::resolve_goal_text;

#[test]
fn commit_goal_writes_when_fresh_and_folds_when_existing() {
    let ws = std::env::temp_dir().join(format!(
        "alcommit-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
    std::fs::write(ws.join(".agentloop/state/goal.md"), "").unwrap();

    // Fresh + blank goal: no-op, goal.md stays empty.
    commit_goal(&ws, "   ").unwrap();
    assert!(std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap().trim().is_empty());

    // Fresh (empty goal.md): commit writes the goal directly.
    commit_goal(&ws, "build a todo app").unwrap();
    let g = std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap();
    assert!(g.contains("build a todo app"));

    // Existing goal + new text: folded as an addition (appended).
    commit_goal(&ws, "also add a --due flag").unwrap();
    let g2 = std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap();
    assert!(g2.contains("build a todo app"));
    assert!(g2.contains("also add a --due flag"));

    // Re-committing identical existing text is a no-op (no duplicate).
    commit_goal(&ws, "build a todo app").unwrap();
    let g3 = std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap();
    assert_eq!(g3.matches("build a todo app").count(), 1);

    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn goal_text_prefers_arg_then_goal_md_then_empty() {
    let ws = std::env::temp_dir().join(format!(
        "algoal-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();

    // No arg, no goal.md -> empty.
    assert_eq!(resolve_goal_text(None, &ws), "");

    // No arg, goal.md present -> file contents (trimmed).
    std::fs::write(ws.join(".agentloop/state/goal.md"), "build a todo app\n").unwrap();
    assert_eq!(resolve_goal_text(None, &ws), "build a todo app");

    // Arg present -> arg wins over goal.md.
    assert_eq!(resolve_goal_text(Some("new goal"), &ws), "new goal");

    // Blank arg -> falls back to goal.md.
    assert_eq!(resolve_goal_text(Some("   "), &ws), "build a todo app");

    let _ = std::fs::remove_dir_all(&ws);
}
