mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::{orchestrator, requests, state};
use std::sync::Arc;

fn cfg() -> Config {
    serde_yaml::from_str(r#"
caps: { max_iterations: 8, max_parallel: 1, item_timeout_sec: 30, total_budget_sec: 300, max_attempts: 5 }
routing:
  planner: { tool: claude, model: opus, effort: high, flags: "" }
  build:   { tool: codex,  model: gpt-5, effort: high, flags: "" }
defaults: { role: build }
"#).unwrap()
}

#[tokio::test]
async fn add_task_after_done_builds_new_item() {
    let ws = common::init_ws_with_request_stub();
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);
    let bk = ws.join(".agentloop/state/backlog.json");
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    // Build it-1, gate passes (no pending request yet).
    orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap(); // planner seeds it-1
    orchestrator::iterate(&cfg(), &ws, 2, &rep).await.unwrap(); // worker builds it-1
    orchestrator::iterate(&cfg(), &ws, 3, &rep).await.unwrap(); // planner marks it-1 done
    assert_eq!(state::open_count(&bk).unwrap(), 0, "all done before add-task");

    // Simulate AddTask in standby: append a request, require it-2 in the gate, re-engage.
    requests::append(&ws, "also build it-2").unwrap();
    std::env::set_var("WANT2", "1");

    orchestrator::iterate(&cfg(), &ws, 4, &rep).await.unwrap(); // planner folds request -> it-2
    let v = state::read(&bk).unwrap();
    assert!(state::item(&v, "it-2").is_some(), "planner added it-2 from the request");
    assert!(requests::pending(&ws).unwrap().is_empty(), "request consumed");

    orchestrator::iterate(&cfg(), &ws, 5, &rep).await.unwrap(); // worker builds it-2
    orchestrator::iterate(&cfg(), &ws, 6, &rep).await.unwrap(); // planner marks it-2 done
    assert!(ws.join("it-2.txt").exists(), "it-2 built and merged");
    assert_eq!(state::open_count(&bk).unwrap(), 0);

    for k in ["FAKE_AGENT","FAKE_AGENT_BIN","WS","WANT2"] { std::env::remove_var(k); }
    let _ = std::fs::remove_dir_all(&ws);
}
