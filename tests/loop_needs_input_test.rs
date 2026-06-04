mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::{inbox, orchestrator, state, task_state};
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

#[tokio::test]
async fn question_is_auto_answered_and_item_completes() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_asking_stub();
    set_env(&ws);
    let bk = ws.join(".agentloop/state/backlog.json");
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    // Iteration 1: builder asks -> question is auto-answered, item flips back to
    // ready (not blocked), nothing waits on a human.
    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();
    assert_eq!(merged, 0);
    let builders = task_state::read_builders(&ws, "task-1").unwrap();
    assert_eq!(
        task_state::item(&builders, "task-1-b1").unwrap()["status"],
        "ready",
        "asking item is re-queued, not parked"
    );
    let a = inbox::read_answer(&ws, "task-1-b1").unwrap();
    assert_eq!(a.question, "make the file?");
    assert_eq!(a.answer, orchestrator::AUTO_ANSWER);
    assert!(
        !ws.join(".agentloop/questions/task-1-b1.json").exists(),
        "question file is consumed"
    );

    // Iteration 2: the stub sees the answer file and completes; customer approves.
    let merged2 = orchestrator::iterate(&cfg(), &ws, 2, &rep).await.unwrap();
    assert_eq!(merged2, 1);
    assert_eq!(
        std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(),
        "made"
    );
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "task-1").unwrap()["status"], "done");

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn stale_question_from_prior_run_is_swept() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_asking_stub();
    // Simulate an interrupted earlier run: task in progress, builder parked
    // blocked on a question that nobody answered.
    let st = ws.join(".agentloop/state");
    std::fs::write(
        st.join("backlog.json"),
        r#"{"items":[{"id":"task-1","title":"f","desc":"d","deps":[],"status":"in_progress","attempts":0,"acceptance":"file exists"}]}"#,
    )
    .unwrap();
    let tdir = st.join("tasks/task-1");
    std::fs::create_dir_all(&tdir).unwrap();
    std::fs::write(tdir.join("design.md"), "Make the file.").unwrap();
    std::fs::write(
        tdir.join("builders.json"),
        r#"{"items":[{"id":"task-1-b1","title":"make file","desc":"write made.txt","deps":[],"status":"blocked","attempts":1,"acceptance":"made.txt exists"}]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/questions")).unwrap();
    std::fs::write(
        ws.join(".agentloop/questions/task-1-b1.json"),
        r#"{"question":"make the file?","context":"need confirm"}"#,
    )
    .unwrap();
    set_env(&ws);
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    // One iteration: the sweep auto-answers, the builder re-dispatches (stub sees
    // the answer file), the work merges and the customer approves.
    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();
    assert_eq!(merged, 1, "swept question lets the parked builder finish");
    let a = inbox::read_answer(&ws, "task-1-b1").unwrap();
    assert_eq!(a.answer, orchestrator::AUTO_ANSWER);
    let bk = ws.join(".agentloop/state/backlog.json");
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "task-1").unwrap()["status"], "done");

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn headless_run_auto_continues_past_questions() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_asking_stub();
    set_env(&ws);
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    let rc = orchestrator::run(&cfg(), &ws, rep).await.unwrap();
    assert_eq!(rc, 0, "headless run no longer halts on builder questions");
    assert_eq!(
        std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(),
        "made"
    );

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}
