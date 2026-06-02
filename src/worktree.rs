use anyhow::{bail, Result};
use std::io::Write;
use std::path::Path;
use std::process::Command;

/// Append a git invocation and its captured output to `<repo>/.agentloop/logs/run.log`
/// when that log dir exists (it does during a real run; app.rs creates it). This keeps
/// git's stdout/stderr off the TUI alternate screen while preserving it for diagnostics.
fn log_git(repo: &Path, args: &[&str], out: &std::process::Output) {
    let log = repo.join(".agentloop/logs/run.log");
    match log.parent() {
        Some(dir) if dir.exists() => {}
        _ => return,
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
        let _ = writeln!(f, "$ git {}", args.join(" "));
        let _ = f.write_all(&out.stdout);
        let _ = f.write_all(&out.stderr);
    }
}

/// Run git, capturing stdout+stderr (never inheriting them onto the TUI). Returns
/// whether the command succeeded.
fn git(repo: &Path, args: &[&str]) -> Result<bool> {
    let out = Command::new("git").arg("-C").arg(repo).args(args).output()?;
    log_git(repo, args, &out);
    Ok(out.status.success())
}

pub fn create(repo: &Path, branch: &str, path: &Path) -> Result<()> {
    let p = path.to_str().unwrap();
    if git(repo, &["worktree", "add", "-q", "-b", branch, p, "HEAD"])? {
        Ok(())
    } else {
        bail!("worktree add failed for {branch}")
    }
}

/// Merge branch into repo's current branch. On conflict, abort and return false.
pub fn merge(repo: &Path, branch: &str) -> Result<bool> {
    if git(repo, &["merge", "--no-edit", "-q", branch])? {
        Ok(true)
    } else {
        let _ = git(repo, &["merge", "--abort"]);
        Ok(false)
    }
}

pub fn remove(repo: &Path, path: &Path, branch: &str) {
    let p = path.to_str().unwrap_or("");
    let _ = git(repo, &["worktree", "remove", "--force", p]);
    let _ = git(repo, &["branch", "-D", branch]);
}

/// Whether `branch` has commits ahead of HEAD (used to detect "claimed done but no commits").
pub fn has_commits_ahead(repo: &Path, branch: &str) -> bool {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "--oneline", &format!("HEAD..{branch}")])
        .output();
    match out {
        Ok(o) => !String::from_utf8_lossy(&o.stdout).trim().is_empty(),
        Err(_) => false,
    }
}
