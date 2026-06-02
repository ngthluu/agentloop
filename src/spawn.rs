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
    let flags = cfg.role_field(&rrole, "flags");

    let mut argv: Vec<String> = Vec::new();
    match tool.as_str() {
        "claude" => argv.extend(["claude".into(), "-p".into(), prompt.into()]),
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
    if let Some(f) = flags {
        for tok in f.split_whitespace() {
            argv.push(tok.to_string());
        }
    }
    Ok(argv)
}

/// Run argv with a wall-clock cap. Returns the exit code, or 124 on timeout.
/// The child is its own process group; on timeout the whole group is signalled
/// (SIGTERM, brief grace, SIGKILL) so descendant claude/codex processes die too.
pub async fn run_with_timeout(
    argv: &[String],
    cwd: &Path,
    log: &Path,
    t: Duration,
) -> Result<i32> {
    use command_group::AsyncCommandGroup;
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    let file =
        std::fs::File::create(log).with_context(|| format!("create log {}", log.display()))?;
    let err = file.try_clone()?;

    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(cwd).stdout(file).stderr(err);

    let mut child = cmd.group_spawn().context("spawn agent process group")?;
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
    if std::env::var("FAKE_AGENT").as_deref() == Ok("1") {
        let bin = std::env::var("FAKE_AGENT_BIN")
            .context("FAKE_AGENT=1 but FAKE_AGENT_BIN unset")?;
        argv.insert(0, bin);
    }
    run_with_timeout(&argv, cwd, log, t).await
}
