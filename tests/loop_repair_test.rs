mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::{orchestrator, state};
use std::sync::Arc;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cfg() -> Config {
    serde_json::from_str(r#"{
  "caps": { "max_iterations": 3, "max_parallel": 1, "item_timeout_sec": 30, "total_budget_sec": 300, "max_attempts": 3 },
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

fn write_backlog(ws: &std::path::Path, json: &str) {
    std::fs::write(ws.join(".agentloop/state/backlog.json"), json).unwrap();
}

/// Zombie mode (a) from test-chat-app: in_progress with no local plan was never
/// re-architected (only ready items reach the architect).
#[tokio::test]
async fn in_progress_task_without_plan_is_rearchitected_and_completes() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_keep_backlog_stub();
    write_backlog(
        &ws,
        r#"{"items":[{"id":"task-1","title":"f","desc":"d","deps":[],"status":"in_progress","attempts":0,"acceptance":"file exists"}]}"#,
    );
    set_env(&ws);
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();

    assert_eq!(
        merged, 1,
        "repaired task is architected and built this round"
    );
    let bk = ws.join(".agentloop/state/backlog.json");
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "task-1").unwrap()["status"], "done");
    assert_eq!(
        std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(),
        "made"
    );
    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

/// Zombie mode (b): ready item depending on an id that is not in the backlog
/// (e.g. a leaked sub-item id) could never dispatch.
#[tokio::test]
async fn ready_task_with_unknown_dep_is_unstuck_and_completes() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_keep_backlog_stub();
    write_backlog(
        &ws,
        r#"{"items":[{"id":"task-1","title":"f","desc":"d","deps":["task-ghost"],"status":"ready","attempts":0,"acceptance":"file exists"}]}"#,
    );
    set_env(&ws);
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();

    assert_eq!(
        merged, 1,
        "unknown dep stripped; item dispatches this round"
    );
    let bk = ws.join(".agentloop/state/backlog.json");
    let v = state::read(&bk).unwrap();
    let task = state::item(&v, "task-1").unwrap();
    assert_eq!(task["status"], "done");
    assert_eq!(task["deps"], serde_json::json!([]));
    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

/// Zombie mode (c): valid plan whose remaining ready builders dep on failed
/// builders deadlocked the parent forever.
#[tokio::test]
async fn deadlocked_builder_plan_is_redesigned_and_completes() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_keep_backlog_stub();
    write_backlog(
        &ws,
        r#"{"items":[{"id":"task-1","title":"f","desc":"d","deps":[],"status":"in_progress","attempts":0,"acceptance":"file exists"}]}"#,
    );
    let tdir = ws.join(".agentloop/state/tasks/task-1");
    std::fs::create_dir_all(&tdir).unwrap();
    std::fs::write(tdir.join("design.md"), "old design").unwrap();
    std::fs::write(
        tdir.join("builders.json"),
        r#"{"items":[
            {"id":"task-1-b1","title":"t","desc":"d","deps":[],"status":"failed","attempts":3,"acceptance":"a"},
            {"id":"task-1-b2","title":"t","desc":"d","deps":["task-1-b1"],"status":"ready","attempts":0,"acceptance":"a"}
        ]}"#,
    )
    .unwrap();
    set_env(&ws);
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();

    assert_eq!(
        merged, 1,
        "deadlocked plan is redesigned and rebuilt this round"
    );
    let bk = ws.join(".agentloop/state/backlog.json");
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "task-1").unwrap()["status"], "done");
    assert!(
        !ws.join(".agentloop/state/tasks/task-1/redesign.json")
            .exists(),
        "redesign counter resets on completion"
    );
    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}
