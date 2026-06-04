use agentloop::config::Config;
use agentloop::spawn;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::sync::Mutex;

/// Mutex to serialize tests that mutate process-global env vars (FAKE_AGENT etc.).
/// Without this, parallel test threads can observe each other's env mutations and
/// race on FAKE_SLEEP in particular. An async-aware Mutex is used because the guard
/// is held across `.await` points (the spawn reads the env it set).
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn cfg() -> Config {
    serde_json::from_str(
        r#"{
            "routing": {
                "manager": { "tool": "claude", "model": "opus", "effort": "high" },
                "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" }
            },
            "defaults": { "role": "builder" }
        }"#,
    )
    .unwrap()
}

#[test]
fn claude_argv_injects_skip_permissions() {
    let a = spawn::build_argv(&cfg(), "manager", "HELLO").unwrap();
    assert_eq!(
        a,
        vec![
            "claude",
            "-p",
            "HELLO",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "opus",
            "--effort",
            "high",
            "--dangerously-skip-permissions",
        ]
    );
}

// --- claude stream-json -> readable log formatting ---

#[test]
fn fmt_assistant_text() {
    let line =
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Replan complete."}]}}"#;
    assert_eq!(spawn::format_claude_event(line), vec!["Replan complete."]);
}

#[test]
fn fmt_tool_use_bash_shows_command() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo build"}}]}}"#;
    let out = spawn::format_claude_event(line);
    assert_eq!(out.len(), 1);
    assert!(out[0].contains("Bash"), "{out:?}");
    assert!(out[0].contains("cargo build"), "{out:?}");
}

#[test]
fn fmt_tool_use_edit_shows_path() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/spawn.rs"}}]}}"#;
    let out = spawn::format_claude_event(line);
    assert_eq!(out.len(), 1);
    assert!(
        out[0].contains("Edit") && out[0].contains("src/spawn.rs"),
        "{out:?}"
    );
}

#[test]
fn fmt_result_returns_final_text() {
    let line = r#"{"type":"result","subtype":"success","result":"All done.","is_error":false}"#;
    assert_eq!(spawn::format_claude_event(line), vec!["All done."]);
}

#[test]
fn fmt_tool_results_are_skipped() {
    let line =
        r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"huge output"}]}}"#;
    assert!(
        spawn::format_claude_event(line).is_empty(),
        "tool_result should be skipped"
    );
}

#[test]
fn fmt_invalid_json_passes_through() {
    assert_eq!(
        spawn::format_claude_event("not json at all"),
        vec!["not json at all"]
    );
}

#[test]
fn fmt_blank_line_yields_nothing() {
    assert!(spawn::format_claude_event("   ").is_empty());
}

#[test]
fn codex_argv_injects_yolo() {
    let a = spawn::build_argv(&cfg(), "builder", "DOIT").unwrap();
    assert_eq!(
        a,
        vec![
            "codex",
            "exec",
            "DOIT",
            "-m",
            "gpt-5",
            "-c",
            "model_reasoning_effort=high",
            "--yolo"
        ]
    );
}

#[test]
fn unknown_tool_errors() {
    let c: Config =
        serde_json::from_str(r#"{"routing":{"x":{"tool":"nope"}},"defaults":{}}"#).unwrap();
    assert!(spawn::build_argv(&c, "x", "p").is_err());
}

#[tokio::test]
async fn fake_agent_runs_and_logs_argv() {
    let bin = env!("CARGO_BIN_EXE_fake_agent");
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", bin);
    std::env::remove_var("FAKE_SLEEP");
    std::env::remove_var("FAKE_EXIT");

    let dir = std::env::temp_dir().join(format!(
        "alspawn-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let log: PathBuf = dir.join("agent.log");

    let rc = spawn::agent_run(
        &cfg(),
        "manager",
        "HELLO",
        &dir,
        &log,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(rc, 0);
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.contains("FAKE_ARGS:"), "log: {logged}");
    assert!(
        logged.contains("--model"),
        "real argv passed to fake: {logged}"
    );
    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
}

#[tokio::test]
async fn timeout_returns_124() {
    let bin = env!("CARGO_BIN_EXE_fake_agent");
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", bin);
    std::env::set_var("FAKE_SLEEP", "10");

    let dir = std::env::temp_dir().join(format!(
        "altimeout-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("agent.log");

    let rc = spawn::agent_run(&cfg(), "manager", "P", &dir, &log, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(rc, 124);
    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("FAKE_SLEEP");
}

#[tokio::test]
async fn kill_all_agents_terminates_in_flight() {
    let bin = env!("CARGO_BIN_EXE_fake_agent");
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", bin);
    std::env::set_var("FAKE_SLEEP", "30"); // would run ~30s if not killed

    let dir = std::env::temp_dir().join(format!(
        "alkill-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("agent.log");

    assert_eq!(spawn::active_agent_count(), 0, "registry starts empty");

    let dir2 = dir.clone();
    let handle = tokio::spawn(async move {
        spawn::agent_run(&cfg(), "manager", "P", &dir2, &log, Duration::from_secs(60)).await
    });

    // Wait for the agent to register its process group.
    for _ in 0..100 {
        if spawn::active_agent_count() >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(spawn::active_agent_count(), 1, "in-flight agent registered");

    // Kill it; the background agent_run must return promptly (not after the 30s sleep).
    spawn::kill_all_agents();
    let joined = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("agent_run did not return after kill_all_agents");
    let rc = joined.expect("join error").expect("agent_run errored");
    assert_ne!(rc, 124, "agent was killed, not timed out");
    assert_eq!(spawn::active_agent_count(), 0, "registry empty after exit");

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("FAKE_SLEEP");
    // kill_all_agents sets the global SHUTDOWN flag; reset it so subsequent tests
    // that rely on agent_run retry logic are not affected.
    spawn::reset_shutdown_for_tests();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn agent_run_waits_out_usage_limit_and_auto_continues() {
    let _guard = ENV_LOCK.lock().await;
    let dir = std::env::temp_dir().join(format!(
        "limitws-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mark = dir.join("first-done");
    // First call: print a usage-limit message (reset already in the past) and fail.
    // Second call: succeed.
    let stub = format!(
        "#!/bin/bash\nif [ ! -f \"{mark}\" ]; then\n  touch \"{mark}\"\n  echo \"Claude AI usage limit reached|1700000000\"\n  exit 1\nfi\necho \"FAKE_OK\"\nexit 0\n",
        mark = mark.display()
    );
    let stub_path = dir.join("stub.sh");
    std::fs::write(&stub_path, stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", &stub_path);
    std::env::set_var("AGENTLOOP_LIMIT_SLACK_SECS", "0");

    let log = dir.join("agent.log");
    let code = spawn::agent_run(
        &cfg(),
        "manager",
        "HELLO",
        &dir,
        &log,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("AGENTLOOP_LIMIT_SLACK_SECS");

    assert_eq!(code, 0, "second attempt after the limit wait succeeds");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains("usage limit reached"),
        "first attempt's limit message is kept in the log: {text}"
    );
    assert!(
        text.contains("auto-continuing"),
        "the wait note is logged: {text}"
    );
    assert!(
        text.contains("FAKE_OK"),
        "second attempt's output is appended"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
