mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::{orchestrator, requests, state};
use std::sync::Arc;

fn cfg() -> Config {
    serde_json::from_str(r#"{
  "caps": { "max_iterations": 8, "max_parallel": 1, "item_timeout_sec": 30, "total_budget_sec": 300, "max_attempts": 5 },
  "routing": {
    "manager": { "tool": "claude", "model": "opus", "effort": "high" },
    "architect": { "tool": "claude", "model": "opus", "effort": "high" },
    "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" },
    "customer": { "tool": "claude", "model": "sonnet", "effort": "medium" },
    "resolver": { "tool": "claude", "model": "sonnet", "effort": "medium" }
  },
  "defaults": { "role": "builder" }
}"#).unwrap()
}

#[tokio::test]
async fn add_task_after_done_builds_new_item() {
    let ws = common::init_ws_with_request_stub();
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);
    let bk = ws.join(".agentloop/state/backlog.json");
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    // Build task-1, gate passes (no pending request yet).
    orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();
    assert_eq!(
        state::open_count(&bk).unwrap(),
        0,
        "all done before add-task"
    );

    // Simulate AddTask in standby: append a request, require task-2 in the gate, re-engage.
    requests::append(&ws, "also build task-2").unwrap();
    std::env::set_var("WANT2", "1");

    orchestrator::iterate(&cfg(), &ws, 2, &rep).await.unwrap();
    let v = state::read(&bk).unwrap();
    assert!(
        state::item(&v, "task-2").is_some(),
        "manager added task-2 from the request"
    );
    assert!(
        requests::pending(&ws).unwrap().is_empty(),
        "request consumed"
    );

    assert!(ws.join("task-2.txt").exists(), "task-2 built and merged");
    assert_eq!(state::open_count(&bk).unwrap(), 0);

    for k in ["FAKE_AGENT", "FAKE_AGENT_BIN", "WS", "WANT2"] {
        std::env::remove_var(k);
    }
    let _ = std::fs::remove_dir_all(&ws);
}
