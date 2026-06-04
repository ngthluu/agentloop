use agentloop::history;
use std::path::PathBuf;

fn tmp_ws(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "{prefix}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn record_and_read_round_trip_with_single_line_reason() {
    let ws = tmp_ws("hist-rt");
    history::record(
        &ws,
        "status",
        "task-1-b1",
        "bounced",
        "needs_input: auto-answered\nsecond line dropped",
    );
    history::record(&ws, "status", "task-1-b2", "failed", "did not report done");

    let evs = history::read_events(&ws);
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0]["kind"], "status");
    assert_eq!(evs[0]["id"], "task-1-b1");
    assert_eq!(evs[0]["status"], "bounced");
    let reason = evs[0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("needs_input: auto-answered"));
    assert!(!reason.contains("second line"), "reason is one line");
    assert!(!evs[0]["ts"].as_str().unwrap().is_empty());

    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn read_events_on_missing_file_is_empty() {
    let ws = tmp_ws("hist-none");
    assert!(history::read_events(&ws).is_empty());
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn archive_file_moves_without_overwriting() {
    let ws = tmp_ws("hist-arch");
    let src = ws.join("r.json");
    let dir = ws.join("archive");

    std::fs::write(&src, "one").unwrap();
    history::archive_file(&src, &dir).unwrap();
    std::fs::write(&src, "two").unwrap();
    history::archive_file(&src, &dir).unwrap();

    assert!(!src.exists(), "source moved away");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        2,
        "second archive does not overwrite the first"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn archive_file_missing_source_is_noop() {
    let ws = tmp_ws("hist-arch-miss");
    history::archive_file(&ws.join("absent.json"), &ws.join("archive")).unwrap();
    let _ = std::fs::remove_dir_all(&ws);
}
