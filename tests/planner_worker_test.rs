use agentloop::{planner, worker};
use serde_json::json;
use std::path::PathBuf;

fn ws_with_state() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("alpw-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let st = dir.join(".agentloop/state");
    std::fs::create_dir_all(&st).unwrap();
    std::fs::write(st.join("goal.md"), "build a thing").unwrap();
    std::fs::write(st.join("master.md"), "# status").unwrap();
    std::fs::write(st.join("backlog.json"), r#"{"items":[]}"#).unwrap();
    dir
}

#[test]
fn planner_prompt_has_contract() {
    let ws = ws_with_state();
    let p = planner::planner_prompt(&ws, 3);
    assert!(p.contains("You are the PLANNER"));
    assert!(p.contains("build a thing"));
    assert!(p.contains("backlog.json"));
    assert!(p.contains("max_attempts"));
}

#[test]
fn worker_prompt_has_contract() {
    let item = json!({
        "id": "it-9", "title": "T", "desc": "D", "role": "build", "acceptance": "A"
    });
    let p = worker::worker_prompt(std::path::Path::new("/ws"), &item);
    assert!(p.contains("You are a WORKER"));
    assert!(p.contains("it-9"));
    assert!(p.contains("A"));
    assert!(p.contains(".agentloop/results/it-9.json"));
}
