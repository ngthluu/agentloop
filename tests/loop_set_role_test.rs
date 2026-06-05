use agentloop::config::Config;
use agentloop::events::{Command, EventLineReporter, Reporter};
use agentloop::orchestrator;
use std::sync::Arc;

#[tokio::test]
async fn set_role_before_start_is_consumed_without_starting_work() {
    let ws = std::env::temp_dir().join(format!(
        "setrole-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&ws).unwrap();
    let cfg: Config = serde_json::from_str(
        r#"{ "routing": { "builder": { "tool": "codex", "effort": "high" } },
             "defaults": { "role": "builder" } }"#,
    )
    .unwrap();
    let (ctx, mut crx) = tokio::sync::mpsc::unbounded_channel::<Command>();
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    // SetRole then Quit: the pre-goal wait loop must apply the routing edit and
    // keep waiting (not treat it as a run start), then exit cleanly on Quit.
    ctx.send(Command::SetRole {
        role: "builder".into(),
        tool: "claude".into(),
        model: "opus".into(),
        effort: String::new(),
    })
    .unwrap();
    ctx.send(Command::Quit).unwrap();

    let rc = orchestrator::run_interactive(&cfg, &ws, rep, &mut crx)
        .await
        .unwrap();
    assert_eq!(rc, 0);
    assert!(
        !ws.join(".agentloop/logs/iter-1").exists(),
        "no iteration ran"
    );
    assert!(
        !ws.join(".agentloop/state/goal.md").exists(),
        "SetRole must not be treated as a run start (commit_goal never called)"
    );
    let _ = std::fs::remove_dir_all(&ws);
}
