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
    let contents: std::collections::HashSet<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert!(
        contents.contains("one") && contents.contains("two"),
        "both archived file contents preserved"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn archive_file_missing_source_is_noop() {
    let ws = tmp_ws("hist-arch-miss");
    history::archive_file(&ws.join("absent.json"), &ws.join("archive")).unwrap();
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn report_lists_bounced_failed_events_and_current_failures() {
    use serde_json::json;

    let ws = tmp_ws("hist-report");
    let st = ws.join(".agentloop/state");
    std::fs::create_dir_all(st.join("tasks/task-9")).unwrap();

    history::record(
        &ws,
        "status",
        "task-1-b1",
        "bounced",
        "needs_input: auto-answered",
    );
    history::record(&ws, "status", "task-9-b2", "failed", "did not report done");
    history::record(&ws, "task", "task-9", "failed", "redesign cap (3) reached");

    std::fs::write(
        st.join("backlog.json"),
        serde_json::to_vec(&json!({"items":[{
            "id":"task-9","title":"browse history","deps":[],"status":"failed",
            "attempts":0,"acceptance":"a","notes":"redesign cap (3) reached"
        }]}))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        st.join("tasks/task-9/builders.json"),
        serde_json::to_vec(&json!({"items":[{
            "id":"task-9-b2","title":"t","desc":"d","deps":[],"status":"failed",
            "attempts":3,"acceptance":"a","notes":"exceeded max_attempts (3)"
        }]}))
        .unwrap(),
    )
    .unwrap();

    let r = history::report(&ws);
    assert!(r.contains("BOUNCED events: 1"), "got:\n{r}");
    assert!(r.contains("task-1-b1"));
    assert!(r.contains("needs_input: auto-answered"));
    assert!(r.contains("FAILED events: 1"));
    assert!(r.contains("TASK redesign/failure events: 1"));
    assert!(r.contains("backlog items currently failed: 1"));
    assert!(r.contains("browse history"));
    assert!(r.contains("task-9/task-9-b2 (attempts 3)"));
    assert!(r.contains("exceeded max_attempts (3)"));

    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn recording_reporter_persists_dispatch_and_status() {
    use agentloop::events::{EventLineReporter, RecordingReporter, Reporter};
    use std::sync::Arc;

    let ws = tmp_ws("hist-reporter");
    let rep = RecordingReporter::new(ws.clone(), Arc::new(EventLineReporter));
    rep.dispatch("task-1-b1", "make file", "codex", "gpt-5", None);
    rep.status("task-1-b1", "bounced", "", "", "needs_input: auto-answered");

    let evs = history::read_events(&ws);
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0]["kind"], "dispatch");
    assert_eq!(evs[0]["status"], "running");
    assert_eq!(evs[1]["kind"], "status");
    assert_eq!(evs[1]["status"], "bounced");
    assert_eq!(evs[1]["reason"], "needs_input: auto-answered");

    let _ = std::fs::remove_dir_all(&ws);
}
