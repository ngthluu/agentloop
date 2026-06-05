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
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
    {
        let _ = writeln!(f, "$ git {}", args.join(" "));
        let _ = f.write_all(&out.stdout);
        let _ = f.write_all(&out.stderr);
    }
}

/// Run git, capturing stdout+stderr (never inheriting them onto the TUI). Returns
/// whether the command succeeded.
fn git(repo: &Path, args: &[&str]) -> Result<bool> {
    Ok(git_out(repo, args)?.status.success())
}

/// Like [`git`] but returns the full captured output, so callers can surface
/// git's stderr in failure notes instead of burying it in run.log.
fn git_out(repo: &Path, args: &[&str]) -> Result<std::process::Output> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
    log_git(repo, args, &out);
    Ok(out)
}

/// First non-empty stderr line of a git invocation, for failure notes.
fn stderr_line(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string()
}

pub fn create(repo: &Path, branch: &str, path: &Path) -> Result<()> {
    // Paths are passed as &str through the shared arg slice; a non-UTF-8
    // workspace path is rejected with an error instead of a panic.
    let Some(p) = path.to_str() else {
        bail!("worktree path is not valid UTF-8: {}", path.display());
    };
    let out = git_out(repo, &["worktree", "add", "-q", "-b", branch, p, "HEAD"])?;
    if out.status.success() {
        Ok(())
    } else {
        bail!("worktree add failed for {branch}: {}", stderr_line(&out))
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

/// Best-effort worktree cleanup. Order matters: `git worktree remove` must run
/// while the directory still exists — rm-ing the dir first orphans the
/// `.git/worktrees/<name>` metadata and the next `worktree add` for a reused id
/// fails with "missing but locked". `prune` then clears any metadata a crashed
/// prior run left behind.
pub fn remove(repo: &Path, path: &Path, branch: &str) {
    if let Some(p) = path.to_str() {
        let _ = git(repo, &["worktree", "remove", "--force", p]);
    }
    if path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
    let _ = git(repo, &["worktree", "prune"]);
    let _ = git(repo, &["branch", "-D", branch]);
}

/// Whether the working tree has uncommitted (staged or unstaged) changes to
/// tracked files. Untracked files are allowed — merges don't touch them unless
/// they collide, and git refuses that on its own. Errors read as dirty: never
/// merge on an unverifiable tree.
pub fn is_dirty(repo: &Path) -> bool {
    let unstaged = git(repo, &["diff", "--quiet"]).unwrap_or(false);
    let staged = git(repo, &["diff", "--cached", "--quiet"]).unwrap_or(false);
    !(unstaged && staged)
}

/// Commit loop-owned tracked changes under `.agentloop/`. The manager rewrites
/// `.agentloop/verify.sh` in the MAIN tree and never commits; in workspaces
/// where that file is tracked, the rewrite left the tree permanently dirty and
/// `is_dirty` bounced every finished merge until the redesign caps blew.
///
/// Pathspec commit: only tracked-file changes under `.agentloop/` are taken
/// from the working tree. The user's staged files elsewhere stay staged, and
/// untracked `.agentloop` runtime files (results, logs, state) stay untracked.
/// No-op on a clean path or mid-merge. Best-effort: a failure here just leaves
/// the old bounce behavior.
pub fn commit_agentloop_changes(repo: &Path) {
    if merge_in_progress(repo) {
        return;
    }
    let Ok(out) = git_out(repo, &["status", "--porcelain", "--", ".agentloop"]) else {
        return;
    };
    let tracked_change = String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| !l.starts_with("??"));
    if !tracked_change {
        return;
    }
    let _ = git(
        repo,
        &[
            "commit",
            "-q",
            "-m",
            "agentloop: persist loop-owned state (verify gate, task state)",
            "--",
            ".agentloop",
        ],
    );
}

/// Outcome of attempting to merge a worker branch into main.
pub enum MergeOutcome {
    Merged,
    Conflict,
}

/// Merge `branch` into the repo's current branch. On success returns `Merged`. On
/// conflict, leaves the working tree in the conflicted (mid-merge) state — does NOT
/// abort — and returns `Conflict`, so a resolver agent can fix it in place.
pub fn merge_or_conflict(repo: &Path, branch: &str) -> Result<MergeOutcome> {
    if git(repo, &["merge", "--no-edit", "-q", branch])? {
        Ok(MergeOutcome::Merged)
    } else {
        Ok(MergeOutcome::Conflict)
    }
}

/// Whether a merge is currently in progress (MERGE_HEAD exists).
pub fn merge_in_progress(repo: &Path) -> bool {
    git(repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).unwrap_or(false)
}

/// Whether the index has unmerged (conflicted) paths.
pub fn has_unmerged(repo: &Path) -> bool {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output();
    match out {
        Ok(o) => !String::from_utf8_lossy(&o.stdout).trim().is_empty(),
        Err(_) => false,
    }
}

/// Commit an in-progress merge whose conflicts have been resolved+staged. Returns success.
pub fn commit_merge(repo: &Path) -> bool {
    git(repo, &["commit", "--no-edit"]).unwrap_or(false)
}

/// Abort an in-progress merge, restoring the pre-merge state. Best-effort.
pub fn abort_merge(repo: &Path) {
    let _ = git(repo, &["merge", "--abort"]);
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
