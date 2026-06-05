use agentloop::worktree;
use agentloop::worktree::MergeOutcome;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {:?} failed", args);
}

fn init_repo() -> std::path::PathBuf {
    // pid + counter: nanos alone collide when parallel tests start in the same
    // tick, and two tests sharing one repo dir fail confusingly.
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "alwt-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("seed.txt"), "seed").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "init"]);
    dir
}

#[test]
fn create_merge_remove_roundtrip() {
    let repo = init_repo();
    let wt = repo.join(".agentloop/worktrees/it-1");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();

    worktree::create(&repo, "item/it-1", &wt).unwrap();
    assert!(wt.join("seed.txt").exists());

    std::fs::write(wt.join("made.txt"), "x").unwrap();
    git(&wt, &["add", "-A"]);
    git(&wt, &["commit", "-qm", "work"]);

    assert!(worktree::merge(&repo, "item/it-1").unwrap());
    assert!(
        repo.join("made.txt").exists(),
        "merged file present on main"
    );

    worktree::remove(&repo, &wt, "item/it-1");
    assert!(!wt.exists());
}

/// Create a branch that conflicts with main on `shared.txt`.
fn make_conflict(repo: &std::path::Path) {
    // main writes shared.txt = "main"
    std::fs::write(repo.join("shared.txt"), "main\n").unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", "main side"]);
    // branch off the PRIOR commit and write a different shared.txt
    git(repo, &["branch", "item/c", "HEAD~1"]);
    let wt = repo.join(".agentloop/worktrees/c");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    git(
        repo,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "item/c"],
    );
    std::fs::write(wt.join("shared.txt"), "branch\n").unwrap();
    git(&wt, &["add", "-A"]);
    git(&wt, &["commit", "-qm", "branch side"]);
    git(
        repo,
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
    );
}

#[test]
fn merge_or_conflict_reports_conflict_without_aborting() {
    let repo = init_repo();
    make_conflict(&repo);

    let outcome = worktree::merge_or_conflict(&repo, "item/c").unwrap();
    assert!(matches!(outcome, MergeOutcome::Conflict));
    // The merge must be left in progress (NOT aborted) for the resolver to fix.
    assert!(
        worktree::merge_in_progress(&repo),
        "merge still in progress"
    );
    assert!(worktree::has_unmerged(&repo), "unmerged paths present");

    // Cleanup so the test leaves a clean repo.
    worktree::abort_merge(&repo);
    assert!(!worktree::merge_in_progress(&repo));
}

#[test]
fn merge_or_conflict_clean_merge_reports_merged() {
    let repo = init_repo();
    let wt = repo.join(".agentloop/worktrees/ok");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    worktree::create(&repo, "item/ok", &wt).unwrap();
    std::fs::write(wt.join("new.txt"), "x").unwrap();
    git(&wt, &["add", "-A"]);
    git(&wt, &["commit", "-qm", "ok"]);

    let outcome = worktree::merge_or_conflict(&repo, "item/ok").unwrap();
    assert!(matches!(outcome, MergeOutcome::Merged));
    assert!(!worktree::merge_in_progress(&repo));
    assert!(repo.join("new.txt").exists());
}

#[test]
fn commit_merge_completes_a_resolved_merge() {
    let repo = init_repo();
    make_conflict(&repo);
    assert!(matches!(
        worktree::merge_or_conflict(&repo, "item/c").unwrap(),
        MergeOutcome::Conflict
    ));

    // Simulate a resolver: pick a resolution and stage it.
    std::fs::write(repo.join("shared.txt"), "resolved\n").unwrap();
    git(&repo, &["add", "shared.txt"]);
    assert!(
        !worktree::has_unmerged(&repo),
        "no unmerged paths after staging"
    );

    assert!(worktree::commit_merge(&repo), "commit completes the merge");
    assert!(
        !worktree::merge_in_progress(&repo),
        "merge no longer in progress"
    );
}

#[test]
fn git_output_is_captured_to_run_log() {
    let repo = init_repo();
    // app.rs creates this dir at runtime; create it so the helper logs there.
    std::fs::create_dir_all(repo.join(".agentloop/logs")).unwrap();

    // A successful worktree op should still work and leave nothing on our stdout.
    let wt = repo.join(".agentloop/worktrees/it-1");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    worktree::create(&repo, "item/it-1", &wt).unwrap();

    let log = std::fs::read_to_string(repo.join(".agentloop/logs/run.log")).unwrap();
    assert!(
        log.contains("git worktree add"),
        "git invocation logged: {log}"
    );
}

#[test]
fn is_dirty_detects_uncommitted_tracked_changes() {
    let repo = init_repo();
    assert!(!worktree::is_dirty(&repo), "clean tree");

    // Untracked files alone are fine — merges don't touch them.
    std::fs::write(repo.join("untracked.txt"), "x").unwrap();
    assert!(!worktree::is_dirty(&repo), "untracked-only stays clean");

    // Modified tracked file = dirty (a merge could clobber it).
    std::fs::write(repo.join("seed.txt"), "modified").unwrap();
    assert!(worktree::is_dirty(&repo), "unstaged change is dirty");

    git(&repo, &["add", "seed.txt"]);
    assert!(worktree::is_dirty(&repo), "staged change is dirty");

    git(&repo, &["commit", "-qm", "commit it"]);
    assert!(!worktree::is_dirty(&repo), "clean again after commit");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn remove_recovers_even_when_dir_was_rmrfd_first() {
    // A crashed run can leave .git/worktrees metadata with the dir already
    // gone; remove() must prune so the next create() for the same id works.
    let repo = init_repo();
    let wt = repo.join(".agentloop/worktrees/it-9");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    worktree::create(&repo, "item/it-9", &wt).unwrap();

    // Simulate the crash artifact: dir removed behind git's back.
    std::fs::remove_dir_all(&wt).unwrap();
    worktree::remove(&repo, &wt, "item/it-9");

    worktree::create(&repo, "item/it-9", &wt).expect("re-create after stale metadata must succeed");
    worktree::remove(&repo, &wt, "item/it-9");
    let _ = std::fs::remove_dir_all(&repo);
}
