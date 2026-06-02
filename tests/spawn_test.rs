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
    serde_yaml::from_str(r#"
routing:
  planner: { tool: claude, model: opus, effort: high, flags: "--flag-a --flag-b" }
  build:   { tool: codex,  model: gpt-5, effort: high, flags: "" }
defaults: { role: build }
"#).unwrap()
}

#[test]
fn claude_argv() {
    let a = spawn::build_argv(&cfg(), "planner", "HELLO").unwrap();
    assert_eq!(a, vec![
        "claude","-p","HELLO","--model","opus","--effort","high","--flag-a","--flag-b"
    ]);
}

#[test]
fn codex_argv() {
    let a = spawn::build_argv(&cfg(), "build", "DOIT").unwrap();
    assert_eq!(a, vec![
        "codex","exec","DOIT","-m","gpt-5","-c","model_reasoning_effort=high"
    ]);
}

#[test]
fn unknown_tool_errors() {
    let c: Config = serde_yaml::from_str("routing: { x: { tool: nope } }\ndefaults: {}\n").unwrap();
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

    let dir = std::env::temp_dir().join(format!("alspawn-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let log: PathBuf = dir.join("agent.log");

    let rc = spawn::agent_run(&cfg(), "planner", "HELLO", &dir, &log, Duration::from_secs(10)).await.unwrap();
    assert_eq!(rc, 0);
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.contains("FAKE_ARGS:"), "log: {logged}");
    assert!(logged.contains("--model"), "real argv passed to fake: {logged}");
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

    let dir = std::env::temp_dir().join(format!("altimeout-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("agent.log");

    let rc = spawn::agent_run(&cfg(), "planner", "P", &dir, &log, Duration::from_secs(1)).await.unwrap();
    assert_eq!(rc, 124);
    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("FAKE_SLEEP");
}
