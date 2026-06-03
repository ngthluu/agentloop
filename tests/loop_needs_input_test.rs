mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::{orchestrator, state, task_state};
use std::sync::Arc;

fn cfg() -> Config {
    serde_json::from_str(r#"{
  "caps": { "max_iterations": 6, "max_parallel": 1, "item_timeout_sec": 30, "total_budget_sec": 300, "max_attempts": 5 },
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
async fn item_goes_blocked_then_answer_completes() {
    let ws = common::init_ws_with_asking_stub();
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);
    let bk = ws.join(".agentloop/state/backlog.json");
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    // One iteration: builder asks -> builder subitem becomes blocked, not merged.
    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();
    assert_eq!(merged, 0);
    let builders = task_state::read_builders(&ws, "task-1").unwrap();
    assert_eq!(
        task_state::item(&builders, "task-1-b1").unwrap()["status"],
        "blocked"
    );
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "task-1").unwrap()["status"], "in_progress");

    // User answers -> builder subitem flips to ready.
    orchestrator::apply_answer(&ws, "task-1-b1", "yes").unwrap();
    let builders = task_state::read_builders(&ws, "task-1").unwrap();
    assert_eq!(
        task_state::item(&builders, "task-1-b1").unwrap()["status"],
        "ready"
    );

    // Next iteration completes (stub now sees an answer file).
    let merged2 = orchestrator::iterate(&cfg(), &ws, 2, &rep).await.unwrap();
    assert_eq!(merged2, 1);
    assert_eq!(
        std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(),
        "made"
    );

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
    let _ = std::fs::remove_dir_all(&ws);
}
