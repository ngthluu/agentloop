use agentloop::cli::resolve_goal_text;

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
