mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::{orchestrator, state};
use std::sync::Arc;

fn cfg() -> Config {
    serde_yaml::from_str(r#"
caps: { max_iterations: 6, max_parallel: 1, item_timeout_sec: 30, total_budget_sec: 300, max_attempts: 5 }
routing:
  planner: { tool: claude, model: opus, effort: high, flags: "" }
  build:   { tool: codex,  model: gpt-5, effort: high, flags: "" }
defaults: { role: build }
"#).unwrap()
}

#[tokio::test]
async fn item_goes_blocked_then_answer_completes() {
    let ws = common::init_ws_with_asking_stub();
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);
    let bk = ws.join(".agentloop/state/backlog.json");
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    // One iteration: worker asks -> item becomes blocked, not merged.
    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();
    assert_eq!(merged, 0);
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "it-1").unwrap()["status"], "blocked");

    // User answers -> item flips to ready.
    orchestrator::apply_answer(&ws, "it-1", "yes").unwrap();
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "it-1").unwrap()["status"], "ready");

    // Next iteration completes (stub now sees an answer file).
    let merged2 = orchestrator::iterate(&cfg(), &ws, 2, &rep).await.unwrap();
    assert_eq!(merged2, 1);
    assert_eq!(std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(), "made");

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
    let _ = std::fs::remove_dir_all(&ws);
}
