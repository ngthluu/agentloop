use agentloop::worktree;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(repo).args(args).status().unwrap().success();
    assert!(ok, "git {:?} failed", args);
}

fn init_repo() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("alwt-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
    assert!(repo.join("made.txt").exists(), "merged file present on main");

    worktree::remove(&repo, &wt, "item/it-1");
    assert!(!wt.exists());
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
    assert!(log.contains("git worktree add"), "git invocation logged: {log}");
}
