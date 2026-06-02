use agentloop::cli;

#[test]
fn bootstrap_creates_state_and_git() {
    let ws = std::env::temp_dir().join(format!("alboot-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    cli::bootstrap_workspace(&ws, "build a todo app", None).unwrap();

    assert!(ws.join(".git").exists(), "git repo initialized");
    assert!(ws.join(".agentloop/state/goal.md").exists());
    assert_eq!(std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap().trim(), "build a todo app");
    assert!(ws.join(".agentloop/state/backlog.json").exists());
    assert!(ws.join(".agentloop/state/master.md").exists());
    assert!(ws.join(".agentloop/config.yaml").exists());
    let gi = std::fs::read_to_string(ws.join(".gitignore")).unwrap();
    assert!(gi.contains(".agentloop/"));
    let _ = std::fs::remove_dir_all(&ws);
}
