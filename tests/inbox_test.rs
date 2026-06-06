use agentloop::inbox;
use std::path::PathBuf;

fn tmp_ws() -> PathBuf {
    // pid + counter: nanos alone collide when parallel tests start in the same
    // tick (macOS quantizes SystemTime to 1µs); two tests sharing one workspace
    // dir tread on each other's questions/answers.
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let ws = std::env::temp_dir().join(format!(
        "alinbox-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(ws.join(".agentloop/questions")).unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/answers")).unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/logs")).unwrap();
    ws
}

#[test]
fn read_question_and_record_answer() {
    let ws = tmp_ws();
    std::fs::write(
        ws.join(".agentloop/questions/it-1.json"),
        r#"{"question":"SQLite or Postgres?","context":"storage layer"}"#,
    )
    .unwrap();

    let q = inbox::read_question(&ws, "it-1").unwrap();
    assert_eq!(q.question, "SQLite or Postgres?");
    assert_eq!(q.context, "storage layer");

    inbox::record_answer(&ws, "it-1", "SQLite or Postgres?", "SQLite").unwrap();
    let a = inbox::read_answer(&ws, "it-1").unwrap();
    assert_eq!(a.answer, "SQLite");

    let block = inbox::prior_qa_block(&ws, "it-1").unwrap();
    assert!(block.contains("SQLite or Postgres?"));
    assert!(block.contains("SQLite"));

    inbox::consume_question(&ws, "it-1").unwrap();
    assert!(!ws.join(".agentloop/questions/it-1.json").exists());
}

#[test]
fn missing_question_is_none() {
    let ws = tmp_ws();
    assert!(inbox::read_question(&ws, "nope").is_err());
    assert!(inbox::prior_qa_block(&ws, "nope").unwrap().is_empty());
}
