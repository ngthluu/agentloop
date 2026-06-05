mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, RecordingReporter, Reporter};
use agentloop::{history, orchestrator, worktree};
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

/// The manager owns .agentloop/verify.sh and rewrites it in the MAIN tree.
/// When that file is tracked, the rewrite used to leave the tree permanently
/// dirty — bouncing every finished merge with "workspace dirty" until the
/// redesign caps blew. Loop-owned tracked changes under .agentloop/ must be
/// auto-committed before integration so they can never block merges.
#[tokio::test]
async fn manager_rewritten_tracked_gate_is_autocommitted_not_bounced() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_tracked_gate_stub();
    set_env(&ws);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &recording(&ws))
        .await
        .unwrap();
    assert_eq!(merged, 1, "builder work merges despite the gate rewrite");

    let evs = history::read_events(&ws);
    assert!(
        !evs.iter().any(|e| e["status"] == "bounced"),
        "no dirty-workspace bounces: {evs:?}"
    );
    assert!(
        std::fs::read_to_string(ws.join("made.txt")).is_ok(),
        "builder work landed in the main tree"
    );
    assert!(
        !worktree::is_dirty(&ws),
        "the loop-owned gate rewrite was committed, not left dirty"
    );

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

/// USER uncommitted work must still block merges (never clobber or silently
/// commit it) — but the bounce must not burn the builder's attempt: the dirty
/// tree says nothing about the builder's work.
#[tokio::test]
async fn user_dirty_tree_bounces_without_burning_attempt_or_touching_user_work() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_stub();
    set_env(&ws);
    // The user edits a tracked file and walks away without committing.
    std::fs::write(ws.join("seed.txt"), "user work in progress").unwrap();

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &recording(&ws))
        .await
        .unwrap();
    assert_eq!(merged, 0, "nothing merges into a user-dirty tree");

    let evs = history::read_events(&ws);
    let bounce = evs
        .iter()
        .find(|e| e["status"] == "bounced" && e["id"] == "task-1-b1")
        .expect("dirty-tree bounce recorded");
    assert!(
        bounce["reason"].as_str().unwrap().contains("dirty"),
        "bounce reason names the dirty tree: {bounce}"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("seed.txt")).unwrap(),
        "user work in progress",
        "user's uncommitted edit is untouched"
    );
    assert!(
        worktree::is_dirty(&ws),
        "user's edit stays uncommitted — the loop never commits user work"
    );

    // The attempt was refunded: the builder is back to ready with the same
    // budget it had, so an unrelated dirty tree can't churn it into
    // max_attempts -> spurious redesign.
    let builders: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ws.join(".agentloop/state/tasks/task-1/builders.json")).unwrap(),
    )
    .unwrap();
    let item = builders["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "task-1-b1")
        .unwrap();
    assert_eq!(
        item["attempts"].as_u64().unwrap(),
        0,
        "dirty-tree bounce refunds the attempt: {item}"
    );

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}
