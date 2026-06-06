use agentloop::cli;
use agentloop::requests;

fn tmp_ws() -> std::path::PathBuf {
    // pid + counter: nanos alone collide when parallel tests start in the same
    // tick (macOS quantizes SystemTime to 1µs); two tests sharing one workspace
    // dir fail with ENOENT when the faster test's cleanup removes it.
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "alrerun-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// macOS quantizes CLOCK_REALTIME to 1µs, so timestamp-only names collide
/// for tests spawned in the same instant (~25% of CI runs). Two tests then
/// share one workspace and the faster test's cleanup yanks the directory out
/// from under the slower one mid-bootstrap (ENOENT).
#[test]
fn tmp_ws_is_unique_across_simultaneous_threads() {
    for _ in 0..200 {
        let a = std::thread::spawn(tmp_ws);
        let b = std::thread::spawn(tmp_ws);
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        assert_ne!(a, b, "two threads must never share a workspace dir");
    }
}

#[test]
fn new_goal_text_is_appended_as_request_and_accumulated() {
    let ws = tmp_ws();
    cli::bootstrap_workspace(&ws, "build a todo app").unwrap();
    cli::fold_rerun_goal(&ws, "also add due dates").unwrap();

    let goal = std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap();
    assert!(goal.contains("build a todo app"), "original goal kept");
    assert!(goal.contains("also add due dates"), "new text accumulated");

    let pending = requests::pending(&ws).unwrap();
    assert_eq!(pending, vec!["also add due dates".to_string()]);

    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn identical_rerun_text_is_a_noop() {
    let ws = tmp_ws();
    cli::bootstrap_workspace(&ws, "build a todo app").unwrap();
    cli::fold_rerun_goal(&ws, "build a todo app").unwrap();

    let pending = requests::pending(&ws).unwrap();
    assert!(pending.is_empty(), "identical text adds no request");

    let _ = std::fs::remove_dir_all(&ws);
}
