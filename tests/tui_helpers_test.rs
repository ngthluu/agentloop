use agentloop::tui::fmt_elapsed;
use std::time::Duration;

#[test]
fn formats_seconds_minutes_hours() {
    assert_eq!(fmt_elapsed(Duration::from_secs(0)), "0s");
    assert_eq!(fmt_elapsed(Duration::from_secs(7)), "7s");
    assert_eq!(fmt_elapsed(Duration::from_secs(59)), "59s");
    assert_eq!(fmt_elapsed(Duration::from_secs(60)), "1m00s");
    assert_eq!(fmt_elapsed(Duration::from_secs(192)), "3m12s");
    assert_eq!(fmt_elapsed(Duration::from_secs(3599)), "59m59s");
    assert_eq!(fmt_elapsed(Duration::from_secs(3600)), "1h00m");
    assert_eq!(fmt_elapsed(Duration::from_secs(3600 + 5 * 60)), "1h05m");
}

use agentloop::tui::tail_file;

#[test]
fn tail_file_returns_last_lines_or_placeholder() {
    // Missing file -> placeholder.
    let missing = std::env::temp_dir().join("altail-does-not-exist.log");
    let _ = std::fs::remove_file(&missing);
    assert_eq!(
        tail_file(&missing, 10, 4096),
        vec!["(no output yet)".to_string()]
    );

    // File with more lines than the cap -> only the last `max_lines`.
    let p = std::env::temp_dir().join(format!(
        "altail-{}.log",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&p, &body).unwrap();
    let last = tail_file(&p, 3, 4096);
    assert_eq!(
        last,
        vec![
            "line 18".to_string(),
            "line 19".to_string(),
            "line 20".to_string()
        ]
    );
    let _ = std::fs::remove_file(&p);
}
