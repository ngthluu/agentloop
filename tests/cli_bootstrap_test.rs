use agentloop::cli;
use std::process::Command;

#[test]
fn bootstrap_creates_state_and_git() {
    let ws = std::env::temp_dir().join(format!(
        "alboot-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    cli::bootstrap_workspace(&ws, "build a todo app").unwrap();

    assert!(ws.join(".git").exists(), "git repo initialized");
    assert!(ws.join(".agentloop/state/goal.md").exists());
    assert_eq!(
        std::fs::read_to_string(ws.join(".agentloop/state/goal.md"))
            .unwrap()
            .trim(),
        "build a todo app"
    );
    assert!(ws.join(".agentloop/state/backlog.json").exists());
    assert!(ws.join(".agentloop/state/master.md").exists());
    assert!(!ws.join(".agentloop/config.yaml").exists());
    assert!(!ws.join(".agentloop/config.json").exists());
    let gi = std::fs::read_to_string(ws.join(".gitignore")).unwrap();
    assert!(gi.contains(".agentloop/"));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn missing_explicit_config_does_not_bootstrap_workspace() {
    let ws = std::env::temp_dir().join(format!(
        "alboot-missing-config-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let cfg = ws.join("missing-config.json");

    let out = Command::new(env!("CARGO_BIN_EXE_agentloop"))
        .arg("build a todo app")
        .arg("--workspace")
        .arg(&ws)
        .arg("--config")
        .arg(&cfg)
        .arg("--dry-run")
        .output()
        .unwrap();

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("config path does not exist"),
        "unexpected stderr: {stderr}"
    );
    assert!(!ws.join(".agentloop").exists());
    assert!(!ws.join(".git").exists());
    let _ = std::fs::remove_dir_all(&ws);
}
