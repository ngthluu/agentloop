use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use crate::config::Config;

/// Process-group ids of every agent currently running. The signal handler and the
/// TUI-exit path use this to kill in-flight claude/codex so an interrupt never leaves
/// agents orphaned (burning credits). Each agent is its own process group.
static ACTIVE_PGIDS: LazyLock<Mutex<HashSet<i32>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn register_pgid(pgid: i32) {
    ACTIVE_PGIDS.lock().unwrap().insert(pgid);
}

fn unregister_pgid(pgid: i32) {
    ACTIVE_PGIDS.lock().unwrap().remove(&pgid);
}

/// Number of agent process groups currently registered (for tests/diagnostics).
pub fn active_agent_count() -> usize {
    ACTIVE_PGIDS.lock().unwrap().len()
}

/// Kill every in-flight agent process group (SIGTERM, brief grace, SIGKILL). Safe to
/// call from a signal handler or on TUI exit; idempotent and never panics.
pub fn kill_all_agents() {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;
    let pgids: Vec<i32> = ACTIVE_PGIDS.lock().unwrap().iter().copied().collect();
    for pg in &pgids {
        let _ = killpg(Pid::from_raw(*pg), Signal::SIGTERM);
    }
    if !pgids.is_empty() {
        std::thread::sleep(Duration::from_millis(300));
        for pg in &pgids {
            let _ = killpg(Pid::from_raw(*pg), Signal::SIGKILL);
        }
    }
}

/// Build the real claude/codex argv for a role (prompt included), mirroring lib/spawn.sh.
pub fn build_argv(cfg: &Config, role: &str, prompt: &str) -> Result<Vec<String>> {
    let rrole = cfg.resolve_role(role).context("no resolvable role")?;
    let tool = cfg.role_field(&rrole, "tool").context("role has no tool")?;
    let model = cfg.role_field(&rrole, "model");
    let effort = cfg.role_field(&rrole, "effort");

    let mut argv: Vec<String> = Vec::new();
    match tool.as_str() {
        // stream-json (+ --verbose, required for --print streaming) makes claude emit
        // events as it works instead of buffering a single final blob; run_with_timeout
        // formats those events into readable log lines. codex already streams plain text.
        "claude" => argv.extend([
            "claude".into(),
            "-p".into(),
            prompt.into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
        ]),
        "codex" => argv.extend(["codex".into(), "exec".into(), prompt.into()]),
        other => bail!("agent_run: unknown tool [{other}]"),
    }
    if let Some(m) = model {
        if tool == "codex" {
            argv.push("-m".into());
            argv.push(m);
        } else {
            argv.push("--model".into());
            argv.push(m);
        }
    }
    if let Some(e) = effort {
        if tool == "codex" {
            argv.push("-c".into());
            argv.push(format!("model_reasoning_effort={e}"));
        } else {
            argv.push("--effort".into());
            argv.push(e);
        }
    }
    match tool.as_str() {
        "claude" => argv.push("--dangerously-skip-permissions".into()),
        "codex" => argv.push("--yolo".into()),
        _ => {}
    }
    Ok(argv)
}

/// Turn one line of claude `--output-format stream-json` into zero or more
/// human-readable log lines. A single assistant message may carry several content
/// blocks (text + tool calls), so this returns a Vec. Anything that isn't valid
/// JSON is passed through verbatim (e.g. plain-text warnings claude prints), and
/// noisy event types (tool results) are dropped to keep the log skimmable.
pub fn format_claude_event(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let v: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return vec![trimmed.to_string()],
    };
    match v.get("type").and_then(|t| t.as_str()) {
        Some("system") => {
            if v.get("subtype").and_then(|s| s.as_str()) == Some("init") {
                match v.get("model").and_then(|m| m.as_str()) {
                    Some(m) => vec![format!("● session started · {m}")],
                    None => vec!["● session started".to_string()],
                }
            } else {
                Vec::new()
            }
        }
        Some("assistant") => {
            let mut out = Vec::new();
            let blocks = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array());
            for block in blocks.into_iter().flatten() {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                            let t = t.trim();
                            if !t.is_empty() {
                                out.push(t.to_string());
                            }
                        }
                    }
                    Some("tool_use") => {
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                        let brief = tool_brief(name, block.get("input"));
                        if brief.is_empty() {
                            out.push(format!("● {name}"));
                        } else {
                            out.push(format!("● {name}({brief})"));
                        }
                    }
                    _ => {} // thinking blocks etc. — skip
                }
            }
            out
        }
        Some("result") => v
            .get("result")
            .and_then(|r| r.as_str())
            .map(|r| vec![r.trim().to_string()])
            .unwrap_or_default(),
        _ => Vec::new(), // "user" (tool results) and unknown event types
    }
}

/// A short, single-line summary of a tool call's input for the log.
fn tool_brief(name: &str, input: Option<&serde_json::Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let pick = |k: &str| input.get(k).and_then(|v| v.as_str());
    let raw = match name {
        "Bash" => pick("command").map(|c| format!("$ {c}")),
        _ => pick("file_path")
            .or_else(|| pick("path"))
            .or_else(|| pick("pattern"))
            .or_else(|| pick("url"))
            .or_else(|| pick("description"))
            .map(|s| s.to_string()),
    };
    let mut s = raw.unwrap_or_default();
    if let Some(i) = s.find('\n') {
        s.truncate(i);
    }
    if s.chars().count() > 80 {
        s = s.chars().take(77).collect::<String>() + "...";
    }
    s
}

/// Run argv with a wall-clock cap. Returns the exit code, or 124 on timeout.
/// The child is its own process group; on timeout the whole group is signalled
/// (SIGTERM, brief grace, SIGKILL) so descendant claude/codex processes die too.
///
/// When `stream_claude` is set the child's stdout is claude `stream-json`; it is
/// piped, parsed line-by-line via [`format_claude_event`], and the readable result
/// appended to the log as it arrives (so the TUI shows live progress instead of a
/// blank "(no output yet)" until the agent finishes). Otherwise stdout/stderr are
/// written to the log directly (codex already streams plain text).
pub async fn run_with_timeout(
    argv: &[String],
    cwd: &Path,
    log: &Path,
    t: Duration,
    stream_claude: bool,
) -> Result<i32> {
    use command_group::AsyncCommandGroup;
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    let file =
        std::fs::File::create(log).with_context(|| format!("create log {}", log.display()))?;

    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(cwd);

    // Background tasks draining the child's piped output into the log (streaming mode).
    let mut pumps: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    let mut child = if stream_claude {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, BufReader};
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.group_spawn().context("spawn agent process group")?;

        // A single writer task owns the log file; the stdout/stderr readers feed it
        // ordered lines over a channel (avoids interleaved writes to one fd).
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut wfile = tokio::fs::File::from_std(file);
        pumps.push(tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            while let Some(line) = rx.recv().await {
                let _ = wfile.write_all(line.as_bytes()).await;
                let _ = wfile.write_all(b"\n").await;
                let _ = wfile.flush().await;
            }
        }));

        if let Some(out) = child.inner().stdout.take() {
            let tx = tx.clone();
            pumps.push(tokio::spawn(async move {
                let mut lines = BufReader::new(out).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    for formatted in format_claude_event(&line) {
                        let _ = tx.send(formatted);
                    }
                }
            }));
        }
        if let Some(errout) = child.inner().stderr.take() {
            let tx = tx.clone();
            pumps.push(tokio::spawn(async move {
                let mut lines = BufReader::new(errout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(line);
                }
            }));
        }
        // Drop the original sender so the writer ends once both readers hit EOF.
        drop(tx);
        child
    } else {
        let err = file.try_clone()?;
        cmd.stdout(file).stderr(err);
        cmd.group_spawn().context("spawn agent process group")?
    };

    // id() returns the group leader's PID, which equals the PGID because
    // group_spawn() puts the child in a fresh process group with itself as leader.
    let raw_pgid = child.id().map(|p| p as i32);
    if let Some(p) = raw_pgid {
        register_pgid(p);
    }
    let pgid = raw_pgid.map(Pid::from_raw);

    // Compute the outcome without an early `?`-return so the pgid is always
    // unregistered (a leaked registry entry would make kill_all_agents target a
    // pid that may have been reused).
    let outcome = match tokio::time::timeout(t, child.wait()).await {
        Ok(Ok(status)) => status.code().unwrap_or(-1),
        Ok(Err(_io_err)) => -1,
        Err(_elapsed) => {
            if let Some(pg) = pgid {
                let _ = killpg(pg, Signal::SIGTERM);
                tokio::time::sleep(Duration::from_secs(1)).await;
                let _ = killpg(pg, Signal::SIGKILL);
            }
            let _ = child.wait().await;
            124
        }
    };
    if let Some(p) = raw_pgid {
        unregister_pgid(p);
    }
    // Drain remaining piped output and flush it to the log before returning.
    for pump in pumps {
        let _ = pump.await;
    }
    Ok(outcome)
}

/// Resolve a role and run the matching CLI (or fake) in cwd, capped by timeout.
pub async fn agent_run(
    cfg: &Config,
    role: &str,
    prompt: &str,
    cwd: &Path,
    log: &Path,
    t: Duration,
) -> Result<i32> {
    let mut argv = build_argv(cfg, role, prompt)?;
    // claude emits stream-json (see build_argv) which run_with_timeout formats live.
    let rrole = cfg.resolve_role(role).context("no resolvable role")?;
    let stream_claude = cfg.role_field(&rrole, "tool").as_deref() == Some("claude");
    if std::env::var("FAKE_AGENT").as_deref() == Ok("1") {
        let bin =
            std::env::var("FAKE_AGENT_BIN").context("FAKE_AGENT=1 but FAKE_AGENT_BIN unset")?;
        argv.insert(0, bin);
    }
    run_with_timeout(&argv, cwd, log, t, stream_claude).await
}
