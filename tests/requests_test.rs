use agentloop::requests;
use std::path::PathBuf;

fn tmp_ws() -> PathBuf {
    let ws = std::env::temp_dir().join(format!(
        "alreq-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
    ws
}

#[test]
fn append_list_consume() {
    let ws = tmp_ws();
    assert!(requests::pending(&ws).unwrap().is_empty());

    requests::append(&ws, "add a --due flag").unwrap();
    requests::append(&ws, "show overdue in red").unwrap();
    let p = requests::pending(&ws).unwrap();
    assert_eq!(
        p,
        vec!["add a --due flag".to_string(), "show overdue in red".to_string()]
    );

    let block = requests::prompt_block(&ws).unwrap();
    assert!(block.contains("PENDING USER REQUESTS"));
    assert!(block.contains("add a --due flag"));

    requests::mark_all_consumed(&ws).unwrap();
    assert!(requests::pending(&ws).unwrap().is_empty());
    assert_eq!(requests::prompt_block(&ws).unwrap(), "");

    let _ = std::fs::remove_dir_all(&ws);
}
