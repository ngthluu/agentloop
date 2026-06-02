use anyhow::{bail, Context, Result};
use std::path::Path;
use std::time::Duration;

use crate::config::Config;

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
    let pgid = child.id().map(|p| Pid::from_raw(p as i32));

    match tokio::time::timeout(t, child.wait()).await {
        Ok(status) => Ok(status?.code().unwrap_or(-1)),
        Err(_elapsed) => {
            if let Some(pg) = pgid {
                let _ = killpg(pg, Signal::SIGTERM);
                tokio::time::sleep(Duration::from_secs(1)).await;
                let _ = killpg(pg, Signal::SIGKILL);
            }
            let _ = child.wait().await;
            Ok(124)
        }
    }
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
