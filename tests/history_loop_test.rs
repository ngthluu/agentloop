mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, RecordingReporter, Reporter};
use agentloop::{history, orchestrator};
use std::sync::Arc;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

fn set_env(ws: &std::path::Path) {
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", ws);
}

fn clear_env() {
    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
}

fn recording(ws: &std::path::Path) -> Arc<dyn Reporter> {
    Arc::new(RecordingReporter::new(
        ws.to_path_buf(),
        Arc::new(EventLineReporter),
    ))
}

#[tokio::test]
async fn happy_iteration_records_terminal_events() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_stub();
    set_env(&ws);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &recording(&ws))
        .await
        .unwrap();
    assert_eq!(merged, 1);

    let evs = history::read_events(&ws);
    let has = |status: &str, id: &str| {
        evs.iter()
            .any(|e| e["kind"] == "status" && e["status"] == status && e["id"] == id)
    };
    assert!(has("done", "manager"), "manager done recorded");
    assert!(has("done", "architect-task-1"), "architect done recorded");
    assert!(has("merged", "task-1-b1"), "builder merge recorded");
    assert!(has("approved", "task-1-customer"), "customer approval recorded");

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn needs_input_bounce_is_recorded_with_reason() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_asking_stub();
    set_env(&ws);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &recording(&ws))
        .await
        .unwrap();
    assert_eq!(merged, 0);

    let evs = history::read_events(&ws);
    let bounce = evs
        .iter()
        .find(|e| e["status"] == "bounced" && e["id"] == "task-1-b1")
        .expect("bounce event recorded");
    assert!(
        bounce["reason"].as_str().unwrap().contains("needs_input"),
        "bounce reason names the cause: {bounce}"
    );

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}
