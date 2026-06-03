mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::{orchestrator, state};
use std::sync::Arc;

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

#[tokio::test]
async fn customer_rejection_keeps_business_task_ready_with_feedback() {
    let ws = common::init_ws_with_stub();
    let stub = r##"#!/bin/bash
tool="$1"; shift
ws_state="$WS/.agentloop/state"; res="$WS/.agentloop/results"
prompt="$*"
case "$prompt" in
  *MANAGER*)
    echo '{"items":[{"id":"task-1","title":"f","desc":"d","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/backlog.json"
    printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$WS/.agentloop/verify.sh"; chmod +x "$WS/.agentloop/verify.sh"
    echo "# updated" > "$ws_state/master.md"
    ;;
  *ARCHITECT*)
    mkdir -p "$ws_state/tasks/task-1"
    echo "Make the file." > "$ws_state/tasks/task-1/design.md"
    echo '{"items":[{"id":"task-1-b1","title":"make file","desc":"write made.txt","deps":[],"status":"ready","attempts":0,"acceptance":"made.txt exists"}]}' > "$ws_state/tasks/task-1/builders.json"
    ;;
  *BUILDER*)
    echo made > "$PWD/made.txt"; git add -A; git commit -qm "builder" 2>/dev/null
    echo '{"status":"done","summary":"made file","files_changed":["made.txt"]}' > "$res/task-1-b1.json"
    ;;
  *"SILLY CUSTOMER"*)
    mkdir -p "$ws_state/tasks/task-1"
    echo '{"status":"rejected","summary":"not accepted","acceptance_notes":"missing visible confirmation"}' > "$ws_state/tasks/task-1/customer.json"
    echo '{"status":"rejected","summary":"not accepted"}' > "$res/task-1-customer.json"
    ;;
esac
exit 0
"##;
    std::fs::write(ws.join("stub.sh"), stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(ws.join("stub.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }

    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);

    let reporter: Arc<dyn Reporter> = Arc::new(EventLineReporter);
    let merged = orchestrator::iterate(&cfg(), &ws, 1, &reporter)
        .await
        .unwrap();
    assert_eq!(merged, 1, "builder work merged before customer review");

    let bk = ws.join(".agentloop/state/backlog.json");
    let v = state::read(&bk).unwrap();
    let task = state::item(&v, "task-1").unwrap();
    assert_eq!(task["status"], "ready");
    assert!(
        task["notes"]
            .as_str()
            .unwrap_or_default()
            .contains("missing visible confirmation"),
        "customer feedback is preserved in notes"
    );
    assert_eq!(state::open_count(&bk).unwrap(), 1);

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
    let _ = std::fs::remove_dir_all(&ws);
}
