use agentloop::state;
use std::io::Write;
use std::path::PathBuf;

fn tmp_backlog(body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("alstate-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("backlog.json");
    std::fs::File::create(&p).unwrap().write_all(body.as_bytes()).unwrap();
    p
}
fn rand_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
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
    assert_eq!(state::ready_items(&p, 10).unwrap(), vec!["it-2", "it-4", "it-5"]);
    assert_eq!(state::ready_items(&p, 1).unwrap(), vec!["it-2"]);
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
    let it2 = v["items"].as_array().unwrap().iter().find(|i| i["id"] == "it-2").unwrap();
    assert_eq!(it2["status"], "done");
    assert_eq!(it2["notes"], "merged ok");
    state::set_status(&p, "it-2", "done", "").unwrap();
    let v = state::read(&p).unwrap();
    let it2 = v["items"].as_array().unwrap().iter().find(|i| i["id"] == "it-2").unwrap();
    assert_eq!(it2["notes"], "merged ok");
}

#[test]
fn increment_attempts() {
    let p = tmp_backlog(BK);
    state::increment_attempts(&p, "it-3").unwrap();
    let v = state::read(&p).unwrap();
    let it3 = v["items"].as_array().unwrap().iter().find(|i| i["id"] == "it-3").unwrap();
    assert_eq!(it3["attempts"], 1);
}
