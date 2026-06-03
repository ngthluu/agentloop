mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::orchestrator;
use std::sync::Arc;

#[tokio::test]
async fn loop_runs_to_done() {
    let ws = common::init_ws_with_stub();

    let cfg: Config = serde_json::from_str(
        r#"{
  "caps": { "max_iterations": 5, "max_parallel": 2, "item_timeout_sec": 30, "total_budget_sec": 300, "max_attempts": 3 },
  "routing": {
    "manager": { "tool": "claude", "model": "opus", "effort": "high" },
    "architect": { "tool": "claude", "model": "opus", "effort": "high" },
    "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" },
    "customer": { "tool": "claude", "model": "sonnet", "effort": "medium" },
    "resolver": { "tool": "claude", "model": "sonnet", "effort": "medium" }
  },
  "defaults": { "role": "builder" }
}"#,
    )
    .unwrap();

    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);

    let reporter: Arc<dyn Reporter> = Arc::new(EventLineReporter);
    let rc = orchestrator::run(&cfg, &ws, reporter).await.unwrap();
    assert_eq!(rc, 0, "loop reports DONE");
    assert_eq!(
        std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(),
        "made"
    );
    assert_eq!(
        agentloop::state::open_count(&ws.join(".agentloop/state/backlog.json")).unwrap(),
        0
    );

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
    let _ = std::fs::remove_dir_all(&ws);
}
