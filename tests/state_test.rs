use agentloop::state;
use std::io::Write;
use std::path::{Path, PathBuf};

fn tmp_backlog(body: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("alstate-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("backlog.json");
    std::fs::File::create(&p)
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    p
}
fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Build a real workspace layout (.agentloop/state/backlog.json) so question files
/// can be placed alongside it. Returns the ws root.
fn tmp_ws(backlog: &str) -> PathBuf {
    let ws = std::env::temp_dir().join(format!("alws-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
    std::fs::write(ws.join(".agentloop/state/backlog.json"), backlog).unwrap();
    ws
}
fn bk_path(ws: &Path) -> PathBuf {
    ws.join(".agentloop/state/backlog.json")
}
fn write_question(ws: &Path, id: &str) {
    let qd = ws.join(".agentloop/questions");
    std::fs::create_dir_all(&qd).unwrap();
    std::fs::write(
        qd.join(format!("{id}.json")),
        r#"{"question":"need a decision?","context":""}"#,
    )
    .unwrap();
}

const BK: &str = r#"{ "items": [
  {"id":"it-1","status":"done","deps":[]},
  {"id":"it-2","status":"ready","deps":["it-1"]},
  {"id":"it-3","status":"ready","deps":["it-2"]},
  {"id":"it-4","status":"ready","deps":[]},
  {"id":"it-5","status":"ready"}
]}"#;

#[test]
fn valid_and_invalid() {
    let p = tmp_backlog(BK);
    assert!(state::backlog_valid(&p));
    let bad = tmp_backlog("not json");
    assert!(!state::backlog_valid(&bad));
}

#[test]
fn ready_respects_deps_and_parallel() {
    let p = tmp_backlog(BK);
    let ws = p.parent().unwrap();
    assert_eq!(
        state::ready_items(&p, ws, 10).unwrap(),
        vec!["it-2", "it-4", "it-5"]
    );
    assert_eq!(state::ready_items(&p, ws, 1).unwrap(), vec!["it-2"]);
}

// A manager-emitted dependency "blocked" (no pending question) is dispatchable
// once its deps are done — this is what makes the loop fully autonomous instead
// of locking up. A "blocked" item that has a real user question is NOT dispatched.
const BK_BLOCKED: &str = r#"{ "items": [
  {"id":"d1","status":"done","deps":[]},
  {"id":"b-dep","status":"blocked","deps":["d1"]},
  {"id":"b-user","status":"blocked","deps":["d1"]},
  {"id":"b-waiting","status":"blocked","deps":["b-dep"]}
]}"#;

#[test]
fn ready_dispatches_dep_blocked_but_not_user_blocked() {
    let ws = tmp_ws(BK_BLOCKED);
    write_question(&ws, "b-user"); // only b-user is truly waiting on the user
    let bk = bk_path(&ws);
    // b-dep: blocked, deps[d1] done, no question -> dispatchable
    // b-user: blocked with a question -> excluded
    // b-waiting: blocked, dep b-dep not done -> excluded
    assert_eq!(state::ready_items(&bk, &ws, 10).unwrap(), vec!["b-dep"]);
}

#[test]
fn open_count_counts_open_states() {
    let p = tmp_backlog(BK);
    assert_eq!(state::open_count(&p).unwrap(), 4);
}

#[test]
fn set_status_and_notes() {
    let p = tmp_backlog(BK);
    state::set_status(&p, "it-2", "done", "merged ok").unwrap();
    let v = state::read(&p).unwrap();
    let it2 = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "it-2")
        .unwrap();
    assert_eq!(it2["status"], "done");
    assert_eq!(it2["notes"], "merged ok");
    state::set_status(&p, "it-2", "done", "").unwrap();
    let v = state::read(&p).unwrap();
    let it2 = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "it-2")
        .unwrap();
    assert_eq!(it2["notes"], "merged ok");
}

#[test]
fn increment_attempts() {
    let p = tmp_backlog(BK);
    state::increment_attempts(&p, "it-3").unwrap();
    let v = state::read(&p).unwrap();
    let it3 = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "it-3")
        .unwrap();
    assert_eq!(it3["attempts"], 1);
}

#[test]
fn strip_unknown_deps_removes_only_unsatisfiable_deps() {
    let dir = std::env::temp_dir().join(format!(
        "strip-deps-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let bk = dir.join("backlog.json");
    std::fs::write(
        &bk,
        r#"{"items":[
            {"id":"task-1","title":"a","desc":"d","deps":[],"status":"done","attempts":0,"acceptance":"x"},
            {"id":"task-2","title":"b","desc":"d","deps":["task-1","task-ghost"],"status":"ready","attempts":0,"acceptance":"x"},
            {"id":"task-3","title":"c","desc":"d","deps":["task-2"],"status":"ready","attempts":0,"acceptance":"x"}
        ]}"#,
    )
    .unwrap();

    let removed = state::strip_unknown_deps(&bk).unwrap();
    assert_eq!(
        removed,
        vec![("task-2".to_string(), "task-ghost".to_string())]
    );

    let v = state::read(&bk).unwrap();
    assert_eq!(
        state::item(&v, "task-2").unwrap()["deps"],
        serde_json::json!(["task-1"]),
        "only the unknown dep is removed"
    );
    assert!(state::item(&v, "task-2").unwrap()["notes"]
        .as_str()
        .unwrap()
        .contains("unknown"));
    assert_eq!(
        state::item(&v, "task-3").unwrap()["deps"],
        serde_json::json!(["task-2"]),
        "valid deps are untouched"
    );
    // Second pass is a no-op.
    assert!(state::strip_unknown_deps(&bk).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_dep_report_lists_open_items_behind_failed_items() {
    let dir = std::env::temp_dir().join(format!(
        "failed-deps-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let bk = dir.join("backlog.json");
    std::fs::write(
        &bk,
        r#"{"items":[
            {"id":"task-1","title":"a","desc":"d","deps":[],"status":"failed","attempts":3,"acceptance":"x"},
            {"id":"task-2","title":"b","desc":"d","deps":["task-1"],"status":"ready","attempts":0,"acceptance":"x"},
            {"id":"task-3","title":"c","desc":"d","deps":[],"status":"ready","attempts":0,"acceptance":"x"}
        ]}"#,
    )
    .unwrap();

    let report = state::failed_dep_report(&bk).unwrap();
    assert!(
        report.contains("task-2 depends on failed task-1"),
        "{report}"
    );
    assert!(!report.contains("task-3"), "healthy items are not reported");
    let _ = std::fs::remove_dir_all(&dir);
}
