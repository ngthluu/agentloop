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

#[test]
fn worker_prompt_documents_needs_input_and_prior_qa() {
    let ws = std::env::temp_dir().join(format!("alwq-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(ws.join(".agentloop/answers")).unwrap();
    // pre-existing answer should be injected into the prompt
    agentloop::inbox::record_answer(&ws, "it-9", "DB?", "SQLite").unwrap();

    let item = serde_json::json!({"id":"it-9","title":"T","desc":"D","role":"build","acceptance":"A"});
    let p = agentloop::worker::worker_prompt(&ws, &item);
    assert!(p.contains("needs_input"), "documents the needs_input escape hatch");
    assert!(p.contains("questions/it-9.json"));
    assert!(p.contains("SQLite"), "prior answer injected");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn planner_prompt_includes_pending_requests() {
    let ws = std::env::temp_dir().join(format!("alpr-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
    std::fs::write(ws.join(".agentloop/state/goal.md"), "g").unwrap();
    std::fs::write(ws.join(".agentloop/state/master.md"), "m").unwrap();
    std::fs::write(ws.join(".agentloop/state/backlog.json"), r#"{"items":[]}"#).unwrap();
    agentloop::requests::append(&ws, "add a --due flag").unwrap();

    let p = agentloop::planner::planner_prompt(&ws, 3);
    assert!(p.contains("PENDING USER REQUESTS"));
    assert!(p.contains("add a --due flag"));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn planner_prompt_maintains_design_and_build_graph() {
    let ws = ws_with_state();
    let p = planner::planner_prompt(&ws, 3);
    // Planner owns the technical design now.
    assert!(p.contains("design.md"), "planner is told to maintain design.md");
    // Work items are all role=build; no architect/fix/trivial.
    assert!(p.contains(r#"role="build""#), "items are tagged build");
    assert!(!p.contains("architect"), "architect role removed");
    // Dependency-aware decomposition is requested.
    assert!(p.contains("dependency-aware"), "asks for a dependency-aware task graph");
}
