use agentloop::{architect, customer, manager, worker};
use serde_json::json;
use std::path::PathBuf;

fn tmp_ws(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "{prefix}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let st = dir.join(".agentloop/state");
    std::fs::create_dir_all(&st).unwrap();
    std::fs::write(st.join("goal.md"), "ship useful software").unwrap();
    std::fs::write(st.join("master.md"), "# status").unwrap();
    std::fs::write(st.join("backlog.json"), r#"{"items":[]}"#).unwrap();
    dir
}

#[test]
fn manager_prompt_is_business_only() {
    let ws = tmp_ws("almgr");
    let p = manager::manager_prompt(&ws, 3);

    assert!(p.contains("You are the MANAGER"));
    assert!(p.contains("business tasks only"));
    assert!(p.contains("backlog.json"));
    assert!(p.contains("master.md"));
    assert!(!p.contains("design.md"));
    assert!(!p.contains("builders.json"));
    assert!(!p.contains("role"));
    assert!(!p.contains("architect"));
    assert!(!p.contains("builder"));
    assert!(!p.contains("builders"));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn manager_prompt_includes_pending_requests() {
    let ws = tmp_ws("almgrreq");
    agentloop::requests::append(&ws, "add CSV export").unwrap();

    let p = manager::manager_prompt(&ws, 3);

    assert!(p.contains("PENDING USER REQUESTS"));
    assert!(p.contains("add CSV export"));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn architect_prompt_writes_task_plan() {
    let ws = tmp_ws("alarch");
    let task = json!({
        "id": "task-1",
        "title": "Import contacts",
        "desc": "Allow CSV imports",
        "acceptance": "contacts appear in the list"
    });

    let p = architect::architect_prompt(&ws, &task);

    assert!(p.contains("You are the ARCHITECT"));
    assert!(p.contains("task-1"));
    assert!(p.contains(".agentloop/state/tasks/task-1/design.md"));
    assert!(p.contains(".agentloop/state/tasks/task-1/builders.json"));
    assert!(p.contains("Do not edit application source code"));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn builder_prompt_uses_parent_task_and_design() {
    let ws = tmp_ws("albuilder");
    let task_dir = ws.join(".agentloop/state/tasks/task-1");
    std::fs::create_dir_all(&task_dir).unwrap();
    std::fs::write(
        task_dir.join("design.md"),
        "Build importer with streaming parse.",
    )
    .unwrap();
    let parent = json!({
        "id": "task-1",
        "title": "Import contacts",
        "desc": "Allow CSV imports",
        "acceptance": "contacts appear in the list"
    });
    let item = json!({
        "id": "builder-1",
        "title": "CSV parser",
        "desc": "Parse uploaded CSV rows",
        "acceptance": "invalid rows report useful errors"
    });

    let p = worker::builder_prompt(&ws, &parent, &item);

    assert!(p.contains("You are a BUILDER"));
    assert!(p.contains("BUSINESS TASK"));
    assert!(p.contains("Import contacts"));
    assert!(p.contains("TECHNICAL DESIGN"));
    assert!(p.contains("Build importer with streaming parse."));
    assert!(p.contains(".agentloop/results/builder-1.json"));
    assert!(
        p.contains("Open decisions are yours"),
        "builders are told to decide autonomously"
    );
    assert!(
        p.contains("An automatic reply will tell you to decide for yourself"),
        "builders know questions are auto-answered"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn customer_prompt_is_ac_only() {
    let ws = tmp_ws("alcust");
    let task = json!({
        "id": "task-1",
        "title": "Import contacts",
        "desc": "Allow CSV imports",
        "acceptance": "contacts appear in the list"
    });

    let p = customer::customer_prompt(&ws, &task);

    assert!(p.contains("You are the SILLY CUSTOMER"));
    assert!(p.contains("acceptance criteria"));
    assert!(p.contains("contacts appear in the list"));
    assert!(p.contains(".agentloop/state/tasks/task-1/customer.json"));
    assert!(p.contains(".agentloop/results/task-1-customer.json"));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn manager_prompt_hardens_backlog_ownership() {
    let ws = std::env::temp_dir().join(format!(
        "mgr-rules-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
    let p = manager::manager_prompt(&ws, 3);
    assert!(p.contains("NEVER write \"in_progress\""));
    assert!(p.contains("must be the id of another item in this backlog.json"));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn manager_prompt_reports_items_stuck_on_failed_deps() {
    let ws = std::env::temp_dir().join(format!(
        "mgr-stuck-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let st = ws.join(".agentloop/state");
    std::fs::create_dir_all(&st).unwrap();
    std::fs::write(
        st.join("backlog.json"),
        r#"{"items":[
            {"id":"task-1","title":"a","desc":"d","deps":[],"status":"failed","attempts":3,"acceptance":"x"},
            {"id":"task-2","title":"b","desc":"d","deps":["task-1"],"status":"ready","attempts":0,"acceptance":"x"}
        ]}"#,
    )
    .unwrap();
    let p = manager::manager_prompt(&ws, 3);
    assert!(p.contains("STUCK ITEMS"));
    assert!(p.contains("task-2 depends on failed task-1"));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn manager_prompt_surfaces_failed_leaf_tasks() {
    // A failed task with NO dependents holds the run open (DONE requires zero
    // failed) but is not dispatchable; if the manager never hears about it the
    // run can never finish. Every failed item must reach the prompt.
    let ws = tmp_ws("almgrfailed");
    let sdir = ws.join(".agentloop/state");
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::write(
        sdir.join("backlog.json"),
        r#"{"items":[{"id":"task-9","title":"orphan","desc":"d","deps":[],"status":"failed","attempts":3,"acceptance":"a","notes":"redesign cap (3) reached"}]}"#,
    )
    .unwrap();

    let p = manager::manager_prompt(&ws, 3);

    assert!(p.contains("FAILED ITEMS"), "failed section present");
    assert!(p.contains("task-9"), "failed leaf id listed");
    assert!(p.contains("redesign cap (3) reached"), "failure note shown");
    let _ = std::fs::remove_dir_all(&ws);
}
