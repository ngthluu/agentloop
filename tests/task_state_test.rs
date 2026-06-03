use agentloop::task_state;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn tmp_ws(prefix: &str) -> PathBuf {
    let ws =
        std::env::temp_dir().join(format!("{prefix}-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&ws).unwrap();
    ws
}

fn write_design(ws: &Path, task_id: &str) {
    let dir = task_state::ensure_task_dir(ws, task_id).unwrap();
    std::fs::write(dir.join("design.md"), "Use the existing task flow.").unwrap();
}

fn write_builders(ws: &Path, task_id: &str, items: Value) {
    task_state::write_builders(ws, task_id, &json!({ "items": items })).unwrap();
}

fn builder(id: &str, status: &str, deps: Value) -> Value {
    json!({
        "id": id,
        "title": format!("Build {id}"),
        "desc": format!("Implement {id}"),
        "deps": deps,
        "status": status,
        "attempts": 0,
        "acceptance": format!("{id} is accepted")
    })
}

fn write_question(ws: &Path, id: &str) {
    let qd = ws.join(".agentloop/questions");
    std::fs::create_dir_all(&qd).unwrap();
    std::fs::write(
        qd.join(format!("{id}.json")),
        r#"{"question":"need a decision","context":""}"#,
    )
    .unwrap();
}

#[test]
fn validates_builder_plan() {
    let ws = tmp_ws("task-state-valid");
    write_design(&ws, "task-1");
    write_builders(
        &ws,
        "task-1",
        json!([
            builder("task-1-b1", "ready", json!([])),
            builder("task-1-b2", "ready", json!([]))
        ]),
    );

    assert!(task_state::builder_plan_valid(&ws, "task-1"));
    assert_eq!(
        task_state::ready_builders(&ws, "task-1", 1).unwrap(),
        vec!["task-1-b1"]
    );
    let builders = task_state::read_builders(&ws, "task-1").unwrap();
    assert!(task_state::item(&builders, "task-1-b2").is_some());
}

#[test]
fn builder_deps_must_be_done() {
    let ws = tmp_ws("task-state-deps");
    write_design(&ws, "task-2");
    write_builders(
        &ws,
        "task-2",
        json!([
            builder("task-2-b1", "done", json!([])),
            builder("task-2-b2", "ready", json!(["task-2-b1"])),
            builder("task-2-b3", "ready", json!(["task-2-b2"])),
            builder("task-2-b4", "blocked", json!(["task-2-b1"])),
            builder("task-2-b5", "blocked", json!(["task-2-b1"]))
        ]),
    );
    write_question(&ws, "task-2-b5");

    assert_eq!(
        task_state::ready_builders(&ws, "task-2", 10).unwrap(),
        vec!["task-2-b2", "task-2-b4"]
    );
}

#[test]
fn invalid_builder_plan_rejects_empty_items_missing_fields_duplicates_and_bad_prefix() {
    let ws = tmp_ws("task-state-invalid");
    write_design(&ws, "task-3");

    let cases = [
        json!([]),
        json!([{
            "id": "task-3-b1",
            "title": "",
            "desc": "Implement it",
            "deps": [],
            "status": "ready",
            "attempts": 0,
            "acceptance": "accepted"
        }]),
        json!([
            builder("task-3-b1", "ready", json!([])),
            builder("task-3-b1", "ready", json!([]))
        ]),
        json!([builder("other-b1", "ready", json!([]))]),
        json!([{
            "id": "task-3-b1",
            "title": "Build it",
            "desc": "Implement it",
            "deps": "task-3-b0",
            "status": "ready",
            "attempts": 0,
            "acceptance": "accepted"
        }]),
        json!([{
            "id": "task-3-b1",
            "title": "Build it",
            "desc": "Implement it",
            "deps": [],
            "status": "waiting",
            "attempts": 0,
            "acceptance": "accepted"
        }]),
        json!([{
            "id": "task-3-b1",
            "title": "Build it",
            "desc": "Implement it",
            "deps": [],
            "status": "ready",
            "attempts": -1,
            "acceptance": "accepted"
        }]),
    ];

    for items in cases {
        write_builders(&ws, "task-3", items);
        assert!(!task_state::builder_plan_valid(&ws, "task-3"));
    }
}

#[test]
fn invalid_builder_plan_rejects_missing_or_empty_design() {
    let ws = tmp_ws("task-state-invalid-design");
    write_builders(
        &ws,
        "task-design",
        json!([builder("task-design-b1", "ready", json!([]))]),
    );

    assert!(!task_state::builder_plan_valid(&ws, "task-design"));

    let dir = task_state::ensure_task_dir(&ws, "task-design").unwrap();
    std::fs::write(dir.join("design.md"), "   \n\t").unwrap();

    assert!(!task_state::builder_plan_valid(&ws, "task-design"));
}

#[test]
fn invalid_builder_plan_rejects_missing_required_string_field() {
    let ws = tmp_ws("task-state-missing-string");
    write_design(&ws, "task-acceptance");
    write_builders(
        &ws,
        "task-acceptance",
        json!([{
            "id": "task-acceptance-b1",
            "title": "Build it",
            "desc": "Implement it",
            "deps": [],
            "status": "ready",
            "attempts": 0
        }]),
    );

    assert!(!task_state::builder_plan_valid(&ws, "task-acceptance"));
}

#[test]
fn mutates_builder_status_and_attempts() {
    let ws = tmp_ws("task-state-mutate");
    write_design(&ws, "task-4");
    write_builders(
        &ws,
        "task-4",
        json!([builder("task-4-b1", "ready", json!([]))]),
    );

    task_state::set_builder_status(&ws, "task-4", "task-4-b1", "in_progress", "started").unwrap();
    task_state::increment_builder_attempts(&ws, "task-4", "task-4-b1").unwrap();

    let builders = task_state::read_builders(&ws, "task-4").unwrap();
    let item = task_state::item(&builders, "task-4-b1").unwrap();
    assert_eq!(item["status"], "in_progress");
    assert_eq!(item["notes"], "started");
    assert_eq!(item["attempts"], 1);
}

#[test]
fn customer_approval_is_read_from_task_local_state() {
    let ws = tmp_ws("task-state-customer");

    task_state::write_customer(&ws, "task-5", &json!({"status":"approved"})).unwrap();
    assert!(task_state::customer_approved(&ws, "task-5"));

    task_state::write_customer(&ws, "task-5", &json!({"status":"rejected"})).unwrap();
    assert!(!task_state::customer_approved(&ws, "task-5"));
}

#[test]
fn all_builders_done_requires_non_empty_all_done() {
    let ws = tmp_ws("task-state-done");
    write_design(&ws, "task-6");
    write_builders(&ws, "task-6", json!([]));
    assert!(!task_state::all_builders_done(&ws, "task-6").unwrap());

    write_builders(
        &ws,
        "task-6",
        json!([
            builder("task-6-b1", "done", json!([])),
            builder("task-6-b2", "done", json!(["task-6-b1"]))
        ]),
    );
    assert!(task_state::all_builders_done(&ws, "task-6").unwrap());
}
