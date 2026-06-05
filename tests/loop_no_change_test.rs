mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, RecordingReporter, Reporter};
use agentloop::{history, orchestrator};
use std::sync::Arc;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cfg() -> Config {
    serde_json::from_str(r#"{
  "caps": { "max_iterations": 6, "max_parallel": 1, "item_timeout_sec": 30, "total_budget_sec": 300, "max_attempts": 3 },
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

/// A builder that verifies its slice, finds the acceptance criteria already
/// hold, commits nothing, and reports done with `"no_changes": true` must be
/// accepted as done — not bounced as "reported done but made no commits".
#[tokio::test]
async fn verified_no_change_done_is_accepted_not_bounced() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_no_change_stub();
    set_env(&ws);

    orchestrator::iterate(&cfg(), &ws, 1, &recording(&ws))
        .await
        .unwrap();

    let evs = history::read_events(&ws);
    assert!(
        !evs.iter().any(|e| e["status"] == "bounced"),
        "no bounce events for a verified no-change done: {evs:?}"
    );
    let done = evs
        .iter()
        .find(|e| e["kind"] == "status" && e["status"] == "done" && e["id"] == "task-1-b1")
        .expect("builder accepted as done");
    assert!(
        done["reason"].as_str().unwrap_or("").contains("no changes"),
        "done note says no changes were needed: {done}"
    );
    assert!(
        evs.iter()
            .any(|e| e["status"] == "approved" && e["id"] == "task-1-customer"),
        "task ran the gate and customer approved: {evs:?}"
    );

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

/// A done with no commits and NO no_changes declaration is still a lazy
/// builder: the anti-laziness bounce must stay.
#[tokio::test]
async fn plain_done_without_commits_still_bounces() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_no_change_stub();
    // Same stub, but strip the no_changes flag from the result.
    let stub = std::fs::read_to_string(ws.join("stub.sh")).unwrap();
    std::fs::write(
        ws.join("stub.sh"),
        stub.replace(r#","no_changes":true"#, ""),
    )
    .unwrap();
    set_env(&ws);

    orchestrator::iterate(&cfg(), &ws, 1, &recording(&ws))
        .await
        .unwrap();

    let evs = history::read_events(&ws);
    let bounce = evs
        .iter()
        .find(|e| e["status"] == "bounced" && e["id"] == "task-1-b1")
        .expect("undeclared no-commit done still bounces");
    assert!(
        bounce["reason"].as_str().unwrap().contains("no commits"),
        "bounce reason names the cause: {bounce}"
    );

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}
