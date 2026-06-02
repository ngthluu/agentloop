use agentloop::cli;
use agentloop::requests;

fn tmp_ws() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "alrerun-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn new_goal_text_is_appended_as_request_and_accumulated() {
    let ws = tmp_ws();
    cli::bootstrap_workspace(&ws, "build a todo app", None).unwrap();
    cli::fold_rerun_goal(&ws, "also add due dates").unwrap();

    let goal = std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap();
    assert!(goal.contains("build a todo app"), "original goal kept");
    assert!(goal.contains("also add due dates"), "new text accumulated");

    let pending = requests::pending(&ws).unwrap();
    assert_eq!(pending, vec!["also add due dates".to_string()]);

    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn identical_rerun_text_is_a_noop() {
    let ws = tmp_ws();
    cli::bootstrap_workspace(&ws, "build a todo app", None).unwrap();
    cli::fold_rerun_goal(&ws, "build a todo app").unwrap();

    let pending = requests::pending(&ws).unwrap();
    assert!(pending.is_empty(), "identical text adds no request");

    let _ = std::fs::remove_dir_all(&ws);
}
