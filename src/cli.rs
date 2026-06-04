use anyhow::{bail, Result};
use clap::Parser;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use crate::config::Config;
use crate::events::{EventLineReporter, RecordingReporter, Reporter};
use crate::orchestrator;

const TEMPLATE_MASTER: &str = include_str!("../templates/master.md");

#[derive(Parser, Debug)]
#[command(name = "agentloop", about = "Autonomous app builder")]
struct Args {
    /// The goal prompt (quote it). Optional: omit to resume an existing workspace.
    goal: Option<String>,
    /// Target dir (default: current dir)
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// config.json path (default: global agentloop config.json)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Wipe existing .agentloop state and start over
    #[arg(long)]
    fresh: bool,
    /// Override caps.max_iterations
    #[arg(long)]
    max_iterations: Option<u32>,
    /// Plan only; do not dispatch workers
    #[arg(long)]
    dry_run: bool,
}

fn git(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Create .agentloop scaffolding, init git, and seed state. Idempotent.
pub fn bootstrap_workspace(ws: &Path, goal: &str) -> Result<()> {
    std::fs::create_dir_all(ws)?;
    let ws = ws.canonicalize().unwrap_or_else(|_| ws.to_path_buf());
    let meta = ws.join(".agentloop");
    for sub in [
        "state",
        "results",
        "logs",
        "worktrees",
        "questions",
        "answers",
    ] {
        std::fs::create_dir_all(meta.join(sub))?;
    }

    if !ws.join(".git").exists() {
        git(&ws, &["init", "-q"]);
    }
    if !git(&ws, &["config", "user.email"]) {
        git(&ws, &["config", "user.email", "agentloop@local"]);
    }
    if !git(&ws, &["config", "user.name"]) {
        git(&ws, &["config", "user.name", "agentloop"]);
    }

    let gi = ws.join(".gitignore");
    let cur = std::fs::read_to_string(&gi).unwrap_or_default();
    if !cur.lines().any(|l| l == ".agentloop/") {
        std::fs::write(&gi, format!("{cur}.agentloop/\n"))?;
    }
    // Ensure at least one commit exists so `worktree add HEAD` works.
    if !git(&ws, &["rev-parse", "HEAD"]) {
        git(&ws, &["add", "-A"]);
        git(&ws, &["commit", "-qm", "agentloop: initial commit"]);
    }

    let master = meta.join("state/master.md");
    if !master.exists() {
        std::fs::write(&master, TEMPLATE_MASTER)?;
    }
    let goalf = meta.join("state/goal.md");
    if !goalf.exists() {
        std::fs::write(&goalf, format!("{goal}\n"))?;
    }
    let bk = meta.join("state/backlog.json");
    if !bk.exists() {
        std::fs::write(&bk, "{\"items\":[]}\n")?;
    }

    Ok(())
}

/// Additive re-run: any new goal text is treated as MORE context layered onto the
/// existing effort, never a different goal. If goal.md already contains the text,
/// this is a no-op (a plain resume). Otherwise the text is queued as a pending
/// request (so the manager folds it into the backlog) and appended to goal.md.
pub fn fold_rerun_goal(ws: &Path, goal: &str) -> Result<()> {
    let goalf = ws.join(".agentloop/state/goal.md");
    let existing = std::fs::read_to_string(&goalf).unwrap_or_default();
    let trimmed = goal.trim();
    if trimmed.is_empty() || existing.contains(trimmed) {
        return Ok(());
    }
    crate::requests::append(ws, trimmed)?;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let addition = format!("\n## Added {stamp}\n{trimmed}\n");
    std::fs::write(&goalf, format!("{existing}{addition}"))?;
    Ok(())
}

/// Commit the goal the user typed on the entry screen. On a fresh workspace (blank
/// goal.md) the text is written directly. Otherwise it is treated as additive context
/// via `fold_rerun_goal` (appended + queued as a pending request; identical text is a
/// no-op plain resume).
pub fn commit_goal(ws: &Path, goal: &str) -> Result<()> {
    let goalf = ws.join(".agentloop/state/goal.md");
    let existing = std::fs::read_to_string(&goalf).unwrap_or_default();
    if existing.trim().is_empty() {
        let trimmed = goal.trim();
        if !trimmed.is_empty() {
            std::fs::write(&goalf, format!("{trimmed}\n"))?;
        }
        Ok(())
    } else {
        fold_rerun_goal(ws, goal)
    }
}

/// The goal text to use: the CLI argument if non-blank, else the persisted
/// `.agentloop/state/goal.md`, else empty (a fresh workspace that will start in standby).
pub fn resolve_goal_text(arg: Option<&str>, ws: &Path) -> String {
    if let Some(g) = arg {
        if !g.trim().is_empty() {
            return g.trim().to_string();
        }
    }
    std::fs::read_to_string(ws.join(".agentloop/state/goal.md"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub async fn run() -> Result<()> {
    let started = std::time::Instant::now();
    let args = Args::parse();
    let ws = args.workspace.clone().unwrap_or(std::env::current_dir()?);
    if args.fresh {
        let _ = std::fs::remove_dir_all(ws.join(".agentloop"));
    }

    let goal_arg = args.goal.clone();
    let cfg_path = if let Some(path) = args.config.as_deref() {
        if !path.exists() {
            bail!("config path does not exist: {}", path.display());
        }
        path.to_path_buf()
    } else {
        Config::ensure_default_config(&Config::default_config_path())?
    };
    bootstrap_workspace(&ws, goal_arg.as_deref().unwrap_or(""))?;
    let ws = ws.canonicalize().unwrap_or(ws);
    if !args.fresh {
        if let Some(g) = goal_arg.as_deref() {
            if !g.trim().is_empty() {
                fold_rerun_goal(&ws, g)?;
            }
        }
    }
    let goal_text = resolve_goal_text(goal_arg.as_deref(), &ws);

    let mut cfg = Config::load(&cfg_path)?;
    if let Some(m) = args.max_iterations {
        cfg.caps.max_iterations = Some(m);
    }

    use std::io::IsTerminal;
    let is_tty = std::io::stdout().is_terminal();
    // Headless and dry-run runs: a SIGINT (Ctrl-C) or SIGTERM kills in-flight agents and
    // exits, so an interrupt never orphans claude/codex. The TUI path installs its own
    // handler (it must also restore the terminal), so skip it there.
    if args.dry_run || !is_tty {
        install_kill_on_signal();
    }

    if args.dry_run {
        let log = ws.join(".agentloop/logs/dryrun-manager.log");
        let ok = crate::manager::manager_run(
            &cfg,
            &ws,
            &log,
            std::time::Duration::from_secs(cfg.item_timeout_sec()),
        )
        .await?;
        if !ok {
            bail!("dry-run: manager produced invalid backlog");
        }
        let bk = std::fs::read_to_string(ws.join(".agentloop/state/backlog.json"))?;
        println!("dry-run: managed backlog ->\n{bk}");
        return Ok(());
    }

    if is_tty {
        let rc = crate::app::run_tui(cfg, ws.clone(), goal_text).await?;
        std::process::exit(rc);
    } else {
        let reporter: Arc<dyn Reporter> = Arc::new(RecordingReporter::new(
            ws.clone(),
            Arc::new(EventLineReporter),
        ));
        let rc = orchestrator::run(&cfg, &ws, reporter).await?;
        eprintln!(
            "=== agentloop finished (rc={rc}) in {}. See {}/.agentloop/state/master.md ===",
            crate::tui::fmt_elapsed(started.elapsed()),
            ws.display()
        );
        std::process::exit(rc);
    }
}

/// Spawn a task that, on Ctrl-C or SIGTERM, kills every in-flight agent process group
/// and exits. For headless and dry-run runs (no terminal state to restore).
fn install_kill_on_signal() {
    tokio::spawn(async move {
        let sigterm = async {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                match signal(SignalKind::terminate()) {
                    Ok(mut s) => {
                        s.recv().await;
                    }
                    Err(_) => std::future::pending::<()>().await,
                }
            }
            #[cfg(not(unix))]
            std::future::pending::<()>().await;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm => {}
        }
        eprintln!("\ninterrupted; stopping agents...");
        crate::spawn::kill_all_agents();
        std::process::exit(130);
    });
}
