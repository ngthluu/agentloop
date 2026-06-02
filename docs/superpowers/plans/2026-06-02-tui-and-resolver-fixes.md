# TUI text/layout fixes, optional goal, conflict resolver, total time — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop git output from corrupting the TUI, stack Jobs/Inbox vertically, make the goal arg optional, resolve merge conflicts with an unbounded agent instead of bouncing, and show total run time.

**Architecture:** `agentloop` is a Rust/tokio app. An orchestrator loop plans, dispatches workers into git worktrees, and integrates their branches. A ratatui TUI (alt-screen) renders state from orchestrator events. Changes are localized to `worktree.rs` (git I/O + conflict surface), `orchestrator.rs` (resolver flow), `worker.rs` (resolver prompt), `cli.rs` (optional goal + total time), `tui.rs` (layout + total time), and `templates/config.yaml` (resolver role).

**Tech Stack:** Rust 2021, tokio, ratatui 0.29, crossterm 0.28, serde_json, anyhow. Tests are integration tests under `tests/` (run with `cargo test`), using a `FAKE_AGENT` shell stub to stand in for claude/codex.

---

## File Structure

- `src/worktree.rs` — Modify: capture git stdout/stderr to `run.log`; add `MergeOutcome`, `merge_or_conflict`, `has_unmerged`, `merge_in_progress`, `commit_merge`, `abort_merge`.
- `src/worker.rs` — Modify: add `resolver_prompt`.
- `src/orchestrator.rs` — Modify: replace abort-and-bounce with the resolver flow; add `resolve_conflict` helper.
- `src/cli.rs` — Modify: `goal: Option<String>`, add `resolve_goal_text`, record/print total run time (headless).
- `src/tui.rs` — Modify: vertical Jobs/Inbox layout; add `started` to `AppState` + `total_elapsed`; show `⏱` in status bar.
- `templates/config.yaml` — Modify: add `resolver` role.
- `README.md` — Modify: document optional goal + resolver behavior.
- `tests/worktree_test.rs` — Modify: tests for git-output capture and conflict helpers.
- `tests/cli_goal_test.rs` — Create: tests for `resolve_goal_text`.
- `tests/tui_render_test.rs` — Create: TestBackend tests for vertical layout + total-time readout.
- `tests/worker_prompt_test.rs` — Create: test for `resolver_prompt`.
- `tests/loop_resolver_test.rs` — Create: end-to-end conflict→resolver integration test.

---

## Task 1: Capture git output (fix "broken text")

**Files:**
- Modify: `src/worktree.rs:5-8` (the `git()` helper)
- Test: `tests/worktree_test.rs`

- [ ] **Step 1: Write the failing test**

Add to `tests/worktree_test.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test worktree_test git_output_is_captured_to_run_log`
Expected: FAIL — `run.log` does not exist (helper currently uses `.status()`, writes nothing).

- [ ] **Step 3: Implement git-output capture**

Replace the top of `src/worktree.rs` (lines 1-8) with:

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test worktree_test`
Expected: PASS (new test plus the existing `create_merge_remove_roundtrip`).

- [ ] **Step 5: Commit**

```bash
git add src/worktree.rs tests/worktree_test.rs
git commit -m "fix(tui): capture git output to run.log so it never leaks onto the alt-screen"
```

---

## Task 2: Conflict-aware merge helpers in worktree

**Files:**
- Modify: `src/worktree.rs` (add `MergeOutcome` + helpers; keep existing `merge`)
- Test: `tests/worktree_test.rs`

- [ ] **Step 1: Write the failing tests**

Add to `tests/worktree_test.rs`:

```rust
use agentloop::worktree::MergeOutcome;

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
    git(repo, &["worktree", "add", "-q", wt.to_str().unwrap(), "item/c"]);
    std::fs::write(wt.join("shared.txt"), "branch\n").unwrap();
    git(&wt, &["add", "-A"]);
    git(&wt, &["commit", "-qm", "branch side"]);
    git(repo, &["worktree", "remove", "--force", wt.to_str().unwrap()]);
}

#[test]
fn merge_or_conflict_reports_conflict_without_aborting() {
    let repo = init_repo();
    make_conflict(&repo);

    let outcome = worktree::merge_or_conflict(&repo, "item/c").unwrap();
    assert!(matches!(outcome, MergeOutcome::Conflict));
    // The merge must be left in progress (NOT aborted) for the resolver to fix.
    assert!(worktree::merge_in_progress(&repo), "merge still in progress");
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
    assert!(matches!(worktree::merge_or_conflict(&repo, "item/c").unwrap(), MergeOutcome::Conflict));

    // Simulate a resolver: pick a resolution and stage it.
    std::fs::write(repo.join("shared.txt"), "resolved\n").unwrap();
    git(&repo, &["add", "shared.txt"]);
    assert!(!worktree::has_unmerged(&repo), "no unmerged paths after staging");

    assert!(worktree::commit_merge(&repo), "commit completes the merge");
    assert!(!worktree::merge_in_progress(&repo), "merge no longer in progress");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test worktree_test merge_or_conflict`
Expected: FAIL to compile — `MergeOutcome`, `merge_or_conflict`, `merge_in_progress`, `has_unmerged`, `abort_merge`, `commit_merge` don't exist.

- [ ] **Step 3: Implement the helpers**

Append to `src/worktree.rs` (after the existing `merge` fn; keep `merge` as-is):

```rust
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
        .arg("-C").arg(repo)
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test worktree_test`
Expected: PASS (all worktree tests).

- [ ] **Step 5: Commit**

```bash
git add src/worktree.rs tests/worktree_test.rs
git commit -m "feat(worktree): conflict-aware merge that leaves conflicts in place for a resolver"
```

---

## Task 3: Resolver prompt

**Files:**
- Modify: `src/worker.rs` (add `resolver_prompt`)
- Test: `tests/worker_prompt_test.rs` (create)

- [ ] **Step 1: Write the failing test**

Create `tests/worker_prompt_test.rs`:

```rust
use agentloop::worker::resolver_prompt;
use serde_json::json;

#[test]
fn resolver_prompt_mentions_branch_title_and_commit() {
    let ws = std::path::Path::new("/tmp/ws");
    let item = json!({"id":"it-7","title":"add auth","desc":"wire login","role":"build"});
    let p = resolver_prompt(ws, &item);

    assert!(p.contains("RESOLVER"), "identifies the role");
    assert!(p.contains("item/it-7"), "names the branch");
    assert!(p.contains("add auth"), "includes the item title");
    assert!(p.contains("commit"), "instructs to commit the merge");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test worker_prompt_test`
Expected: FAIL to compile — `resolver_prompt` does not exist.

- [ ] **Step 3: Implement `resolver_prompt`**

Add to `src/worker.rs` (after `worker_prompt`):

```rust
/// Prompt for the conflict-resolver agent. It runs in the MAIN workspace, which is
/// mid-merge with conflicts from `item/<id>`.
pub fn resolver_prompt(ws: &Path, item: &Value) -> String {
    let id = item["id"].as_str().unwrap_or("");
    let title = item["title"].as_str().unwrap_or("");
    let desc = item["desc"].as_str().unwrap_or("");
    format!(r#"You are a MERGE-CONFLICT RESOLVER on an autonomous app build. The git repo at
{ws} is in the middle of merging branch item/{id} into the main branch and has conflicts.
Resolve every conflict so the result reflects the intent of BOTH sides for this item:

  id:    {id}
  title: {title}
  task:  {desc}

Steps:
- Inspect the conflicts (git status; git diff). Resolve all <<<<<<< ======= >>>>>>>
  markers, keeping a correct result that builds.
- `git add` the resolved files, then `git commit --no-edit` to complete the merge.
- Do not change unrelated files and do not start new work."#,
        ws = ws.display(), id = id, title = title, desc = desc)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test worker_prompt_test`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/worker.rs tests/worker_prompt_test.rs
git commit -m "feat(worker): resolver_prompt for the conflict-resolver agent"
```

---

## Task 4: Wire the resolver into the orchestrator

**Files:**
- Modify: `src/orchestrator.rs:1-10` (imports), `src/orchestrator.rs:161-178` (integration branch), add `resolve_conflict` helper
- Modify: `templates/config.yaml` (add `resolver` role)
- Test: `tests/loop_resolver_test.rs` (create)

- [ ] **Step 1: Add the `resolver` role to the config template**

Edit `templates/config.yaml`, inside `routing:` (after the `trivial:` line), add:

```yaml
  resolver:  { tool: claude, model: sonnet, effort: medium, flags: "--dangerously-skip-permissions" }
```

- [ ] **Step 2: Write the failing integration test**

Create `tests/loop_resolver_test.rs`:

```rust
mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::orchestrator;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn git(repo: &Path, args: &[&str]) {
    assert!(Command::new("git").arg("-C").arg(repo).args(args).status().unwrap().success());
}

/// Workspace whose two items both write `shared.txt`, forcing a conflict on the second
/// merge; a RESOLVER prompt resolves it by committing the in-progress merge.
fn init_ws_conflict_stub() -> PathBuf {
    let ws = std::env::temp_dir().join(format!(
        "alres-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let st = ws.join(".agentloop/state");
    std::fs::create_dir_all(&st).unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/results")).unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/logs")).unwrap();
    git(&ws, &["init", "-q"]);
    git(&ws, &["config", "user.email", "t@t"]);
    git(&ws, &["config", "user.name", "t"]);
    std::fs::write(ws.join("seed.txt"), "seed").unwrap();
    git(&ws, &["add", "-A"]);
    git(&ws, &["commit", "-qm", "init"]);
    std::fs::write(st.join("goal.md"), "make shared").unwrap();
    std::fs::write(st.join("master.md"), "# status").unwrap();
    std::fs::write(st.join("backlog.json"), r#"{"items":[]}"#).unwrap();

    let stub = r##"#!/bin/bash
tool="$1"; shift
ws="$WS"; st="$ws/.agentloop/state"; res="$ws/.agentloop/results"
prompt="$*"
case "$prompt" in
  *RESOLVER*)
    echo resolved > "$PWD/shared.txt"
    git add shared.txt
    git commit --no-edit -q
    ;;
  *PLANNER*)
    n=$(cat "$ws/.pn" 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > "$ws/.pn"
    if [ "$n" -eq 1 ]; then
      echo '{"items":[{"id":"it-1","title":"a","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"ok"},{"id":"it-2","title":"b","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"ok"}]}' > "$st/backlog.json"
      printf '#!/bin/bash\ntest -f "$PWD/shared.txt"\n' > "$ws/.agentloop/verify.sh"; chmod +x "$ws/.agentloop/verify.sh"
    fi
    echo "# m" > "$st/master.md"
    ;;
  *WORKER*)
    id=$(echo "$prompt" | grep -oE 'it-[0-9]+' | head -1)
    echo "$id" > "$PWD/shared.txt"
    git add -A; git commit -qm "w $id" >/dev/null 2>&1
    echo "{\"status\":\"done\",\"summary\":\"s\",\"files_changed\":[\"shared.txt\"]}" > "$res/$id.json"
    ;;
esac
exit 0
"##;
    let stub_path = ws.join("stub.sh");
    std::fs::write(&stub_path, stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    ws
}

#[tokio::test]
async fn merge_conflict_is_resolved_by_an_agent_not_bounced() {
    let ws = init_ws_conflict_stub();

    let cfg: Config = serde_yaml::from_str(
        r#"
caps: { max_iterations: 6, max_parallel: 2, item_timeout_sec: 30, total_budget_sec: 300, max_attempts: 3 }
routing:
  planner:  { tool: claude, model: opus,   effort: high,   flags: "" }
  build:    { tool: codex,  model: gpt-5,  effort: high,   flags: "" }
  resolver: { tool: claude, model: sonnet, effort: medium, flags: "" }
defaults: { role: build }
"#,
    )
    .unwrap();

    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);

    let reporter: Arc<dyn Reporter> = Arc::new(EventLineReporter);
    let rc = orchestrator::run(&cfg, &ws, reporter).await.unwrap();

    assert_eq!(rc, 0, "loop reaches DONE after resolving the conflict");
    assert!(ws.join("shared.txt").exists(), "shared.txt is present on main");
    assert_eq!(
        agentloop::state::open_count(&ws.join(".agentloop/state/backlog.json")).unwrap(),
        0,
        "no open items remain"
    );

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --test loop_resolver_test`
Expected: FAIL — with today's abort-and-bounce, the conflicting item never merges, so the loop stalls (`rc != 0`) and `open_count != 0`.

- [ ] **Step 4: Add the `resolve_conflict` helper to the orchestrator**

In `src/orchestrator.rs`, update the imports near the top. Change:

```rust
use crate::{planner, state, worker, worktree};
```

to:

```rust
use crate::worktree::MergeOutcome;
use crate::{planner, spawn, state, worker, worktree};
```

Then add this helper above `pub async fn run(` (anywhere at module scope):

```rust
/// An effectively-infinite timeout. The resolver is unbounded (no wall-clock cap) per
/// design, but is still registered in ACTIVE_PGIDS by the spawn layer, so quitting the
/// TUI / SIGINT / SIGTERM still kills it (no orphaned agent).
const NO_TIMEOUT: Duration = Duration::from_secs(100 * 365 * 24 * 3600);

/// Spawn an unbounded resolver agent in the main workspace to resolve an in-progress
/// merge conflict for `id`, then complete the merge. Returns true if the merge is
/// resolved and committed.
async fn resolve_conflict(
    cfg: &Config,
    ws: &Path,
    id: &str,
    label: &str,
    item: &Value,
    n: u32,
    reporter: &Arc<dyn Reporter>,
) -> Result<bool> {
    let rrole = cfg.resolve_role("resolver").unwrap_or_default();
    let tool = cfg.role_field(&rrole, "tool").unwrap_or_default();
    let model = cfg.role_field(&rrole, "model").unwrap_or_default();
    let rid = format!("resolve-{id}");
    let log = ws.join(format!(".agentloop/logs/iter-{n}/{rid}.log"));
    reporter.dispatch(&rid, &format!("resolve merge conflict — {label}"), &tool, &model, Some(&log));

    let prompt = worker::resolver_prompt(ws, item);
    // Unbounded: run in the main workspace with no effective timeout.
    let _ = spawn::agent_run(cfg, "resolver", &prompt, ws, &log, NO_TIMEOUT).await;

    // Resolved iff no unmerged paths remain. If the agent resolved+staged but didn't
    // commit, finish the merge ourselves.
    if worktree::has_unmerged(ws) {
        reporter.status(&rid, "failed", &tool, &model);
        return Ok(false);
    }
    if worktree::merge_in_progress(ws) && !worktree::commit_merge(ws) {
        reporter.status(&rid, "failed", &tool, &model);
        return Ok(false);
    }
    reporter.status(&rid, "merged", &tool, &model);
    Ok(true)
}
```

- [ ] **Step 5: Replace the abort-and-bounce branch**

In `src/orchestrator.rs`, find this block inside `iterate()`:

```rust
            } else if worktree::merge(ws, &branch)? {
                state::set_status(&bk, id, "done", "")?;
                reporter.status(id, "merged", "", "");
                merged += 1;
            } else {
                state::set_status(&bk, id, "ready", "merge conflict; replan")?;
                reporter.status(id, "bounced", "", "");
            }
```

Replace it with:

```rust
            } else {
                match worktree::merge_or_conflict(ws, &branch)? {
                    MergeOutcome::Merged => {
                        state::set_status(&bk, id, "done", "")?;
                        reporter.status(id, "merged", "", "");
                        merged += 1;
                    }
                    MergeOutcome::Conflict => {
                        let label = state::read(&bk)
                            .ok()
                            .as_ref()
                            .and_then(|v| state::item(v, id))
                            .and_then(|i| i["title"].as_str().map(String::from))
                            .unwrap_or_default();
                        let item_v = state::read(&bk)
                            .ok()
                            .and_then(|v| state::item(&v, id).cloned())
                            .unwrap_or(Value::Null);
                        if resolve_conflict(cfg, ws, id, &label, &item_v, n, reporter).await? {
                            state::set_status(&bk, id, "done", "")?;
                            reporter.status(id, "merged", "", "");
                            merged += 1;
                        } else {
                            worktree::abort_merge(ws);
                            state::set_status(&bk, id, "ready", "merge conflict; resolver failed")?;
                            reporter.status(id, "bounced", "", "");
                        }
                    }
                }
            }
```

Note: `iterate()` already has `n` (the iteration number) and `reporter` in scope.

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test --test loop_resolver_test`
Expected: PASS — the conflicting item is resolved by the resolver and the loop reaches DONE.

- [ ] **Step 7: Run the full suite to check for regressions**

Run: `cargo test`
Expected: PASS (existing `loop_test`, `worktree_test`, etc. still green).

- [ ] **Step 8: Commit**

```bash
git add src/orchestrator.rs templates/config.yaml tests/loop_resolver_test.rs
git commit -m "feat(loop): resolve merge conflicts with an unbounded resolver agent instead of bouncing"
```

---

## Task 5: Optional goal argument + total run time (CLI)

**Files:**
- Modify: `src/cli.rs` (`Args.goal`, add `resolve_goal_text`, total time in headless exit)
- Test: `tests/cli_goal_test.rs` (create)

- [ ] **Step 1: Write the failing test**

Create `tests/cli_goal_test.rs`:

```rust
use agentloop::cli::resolve_goal_text;

#[test]
fn goal_text_prefers_arg_then_goal_md_then_empty() {
    let ws = std::env::temp_dir().join(format!(
        "algoal-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();

    // No arg, no goal.md -> empty.
    assert_eq!(resolve_goal_text(None, &ws), "");

    // No arg, goal.md present -> file contents (trimmed).
    std::fs::write(ws.join(".agentloop/state/goal.md"), "build a todo app\n").unwrap();
    assert_eq!(resolve_goal_text(None, &ws), "build a todo app");

    // Arg present -> arg wins over goal.md.
    assert_eq!(resolve_goal_text(Some("new goal"), &ws), "new goal");

    // Blank arg -> falls back to goal.md.
    assert_eq!(resolve_goal_text(Some("   "), &ws), "build a todo app");

    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test cli_goal_test`
Expected: FAIL to compile — `resolve_goal_text` does not exist.

- [ ] **Step 3: Implement `resolve_goal_text` and make the arg optional**

In `src/cli.rs`, change the `goal` field in `Args`:

```rust
    /// The goal prompt (quote it). Optional: omit to resume an existing workspace.
    goal: Option<String>,
```

Add this public helper (next to `fold_rerun_goal`):

```rust
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
```

- [ ] **Step 4: Wire it into `run()` and add total time**

In `src/cli.rs`'s `run()`, replace the body from the start through the TUI/headless dispatch with the version below. Concretely, change `&args.goal` usages and the final `goal` passed to the TUI/orchestrator, and add the `started` timer:

```rust
pub async fn run() -> Result<()> {
    let started = std::time::Instant::now();
    let args = Args::parse();
    let ws = args.workspace.clone().unwrap_or(std::env::current_dir()?);
    if args.fresh {
        let _ = std::fs::remove_dir_all(ws.join(".agentloop"));
    }

    let goal_arg = args.goal.clone();
    let cfg_path = bootstrap_workspace(&ws, goal_arg.as_deref().unwrap_or(""), args.config.as_deref())?;
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
    if args.dry_run || !is_tty {
        install_kill_on_signal();
    }

    if args.dry_run {
        let log = ws.join(".agentloop/logs/dryrun-planner.log");
        let ok = crate::planner::planner_run(
            &cfg,
            &ws,
            &log,
            std::time::Duration::from_secs(cfg.item_timeout_sec()),
        )
        .await?;
        if !ok {
            bail!("dry-run: planner produced invalid backlog");
        }
        let bk = std::fs::read_to_string(ws.join(".agentloop/state/backlog.json"))?;
        println!("dry-run: planned backlog ->\n{bk}");
        return Ok(());
    }

    if is_tty {
        let rc = crate::app::run_tui(cfg, ws.clone(), goal_text).await?;
        std::process::exit(rc);
    } else {
        let rc = orchestrator::run(&cfg, &ws, Arc::new(EventLineReporter)).await?;
        eprintln!(
            "=== agentloop finished (rc={rc}) in {}. See {}/.agentloop/state/master.md ===",
            crate::tui::fmt_elapsed(started.elapsed()),
            ws.display()
        );
        std::process::exit(rc);
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test cli_goal_test && cargo build`
Expected: PASS and a clean build.

- [ ] **Step 6: Run the CLI-related suites for regressions**

Run: `cargo test --test cli_bootstrap_test --test cli_rerun_test`
Expected: PASS (bootstrap/rerun unaffected — they call `bootstrap_workspace`/`fold_rerun_goal` directly).

- [ ] **Step 7: Commit**

```bash
git add src/cli.rs tests/cli_goal_test.rs
git commit -m "feat(cli): optional goal arg (resume/standby) and total run time on headless exit"
```

---

## Task 6: TUI — vertical layout + total-time readout

**Files:**
- Modify: `src/tui.rs` (`AppState` gets `started` + `total_elapsed`; status bar shows `⏱`; main split becomes vertical)
- Test: `tests/tui_render_test.rs` (create)

- [ ] **Step 1: Write the failing test**

Create `tests/tui_render_test.rs`:

```rust
use agentloop::events::Event;
use agentloop::tui::{self, AppState};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Find the (row, col) of the first cell where `needle` starts in the rendered buffer.
fn find(term: &Terminal<TestBackend>, needle: &str) -> Option<(u16, u16)> {
    let buf = term.backend().buffer();
    let area = buf.area();
    for y in 0..area.height {
        let mut row = String::new();
        for x in 0..area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        if let Some(idx) = row.find(needle) {
            return Some((y, idx as u16));
        }
    }
    None
}

#[test]
fn jobs_render_above_inbox_full_width() {
    let mut s = AppState::new("goal".into());
    s.apply(Event::JobDispatched {
        id: "it-1".into(), label: "scaffold".into(), tool: "codex".into(),
        model: "gpt-5".into(), log_path: None,
    });
    s.apply(Event::QuestionRaised {
        item_id: "db".into(), label: "db".into(), text: "q?".into(), context: "".into(),
    });

    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();

    let jobs = find(&term, "Jobs").expect("Jobs pane rendered");
    let inbox = find(&term, "Inbox").expect("Inbox pane rendered");
    // Vertical stacking: Jobs title is on an earlier row than Inbox title, and they are
    // NOT side by side (different rows).
    assert!(jobs.0 < inbox.0, "Jobs ({jobs:?}) is above Inbox ({inbox:?})");
}

#[test]
fn status_bar_shows_total_time() {
    let s = AppState::new("goal".into());
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(find(&term, "\u{23f1}").is_some(), "status bar shows the ⏱ total-time glyph");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test tui_render_test`
Expected: FAIL — `jobs.0 < inbox.0` fails (currently side-by-side, same row) and the `⏱` glyph is absent.

- [ ] **Step 3: Add `started` + `total_elapsed` to `AppState`**

In `src/tui.rs`, add a field to the `AppState` struct (after `log_scroll: u16,`):

```rust
    started: std::time::Instant,
```

In `AppState::new`, add to the struct literal (after `log_scroll: 0,`):

```rust
            started: std::time::Instant::now(),
```

Add a method in the `impl AppState` block (e.g. after `in_job_detail`):

```rust
    /// Wall-clock time since the session (TUI) started.
    pub fn total_elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }
```

- [ ] **Step 4: Show the total in the status bar**

In `tui::render`, replace the `status_text` block:

```rust
    let status_text = if s.standby {
        format!(
            " ✓ DONE · standby  │  {}  │  iter {}  │  gate: {}  │  open: {}  │  ❓{}",
            s.goal, s.iter, s.gate, s.open, s.inbox.len()
        )
    } else {
        format!(
            " {}  │  iter {}  │  gate: {}  │  open: {}  │  ❓{}",
            s.goal, s.iter, s.gate, s.open, s.inbox.len()
        )
    };
```

with:

```rust
    let total = fmt_elapsed(s.total_elapsed());
    let status_text = if s.standby {
        format!(
            " ✓ DONE · standby  │  {}  │  iter {}  │  gate: {}  │  open: {}  │  ❓{}  │  ⏱ {}",
            s.goal, s.iter, s.gate, s.open, s.inbox.len(), total
        )
    } else {
        format!(
            " {}  │  iter {}  │  gate: {}  │  open: {}  │  ❓{}  │  ⏱ {}",
            s.goal, s.iter, s.gate, s.open, s.inbox.len(), total
        )
    };
```

- [ ] **Step 5: Make the main area vertical (Jobs over Inbox)**

In `tui::render`, find the non-detail branch:

```rust
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[1]);
```

Replace with:

```rust
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(chunks[1]);
```

(The rest is unchanged: `main_chunks[0]` is Jobs, `main_chunks[1]` is Inbox — now top/bottom instead of left/right.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --test tui_render_test`
Expected: PASS (both tests).

- [ ] **Step 7: Run the TUI suites for regressions**

Run: `cargo test --test tui_viewmodel_test --test tui_helpers_test`
Expected: PASS (the `started` field has a default in `new`; no existing test constructs `AppState` differently).

- [ ] **Step 8: Commit**

```bash
git add src/tui.rs tests/tui_render_test.rs
git commit -m "feat(tui): stack Jobs over Inbox (1 col, 2 rows) and show total run time"
```

---

## Task 7: Docs

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the README**

In `README.md`, update the usage/behavior text to cover the two user-facing changes. Add (in the usage section):

```markdown
## Running

    agentloop "build a todo app" --workspace=./proj   # first run, with a goal
    agentloop --workspace=./proj                       # resume an existing workspace
    agentloop --workspace=./new-dir                    # fresh dir: starts in standby; press [a] to add a task

The goal argument is optional. When omitted, agentloop reads the goal from
`<workspace>/.agentloop/state/goal.md` and resumes; if there is no prior goal it starts
in standby waiting for you to add a task.

## Merge conflicts

When a worker's branch conflicts on merge, agentloop spawns a dedicated **resolver**
agent (config role `resolver`) in the workspace to resolve the conflict and complete the
merge, instead of bouncing the item. The resolver is unbounded (no attempt cap, no
timeout) but is killed when you quit, so it never orphans. If it cannot resolve, the
merge is aborted and the item bounces as before.
```

- [ ] **Step 2: Verify the build and full suite once more**

Run: `cargo test`
Expected: PASS (all tests).

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: optional goal argument, resolver behavior, total run time"
```

---

## Self-Review notes (for the implementer)

- **Spec coverage:** Task 1 → spec §1 (text containment). Task 6 → spec §2 (layout) + §5 (total time, TUI). Task 5 → spec §3 (optional goal) + §5 (total time, headless). Tasks 2–4 → spec §4 (resolver). Spec's deferred notes-surfacing is intentionally not implemented (documented as out of scope).
- **Type consistency:** `MergeOutcome` (variants `Merged`/`Conflict`), `merge_or_conflict`, `merge_in_progress`, `has_unmerged`, `commit_merge`, `abort_merge` are defined in Task 2 and used unchanged in Task 4. `resolver_prompt(ws, item)` defined in Task 3, used in Task 4's `resolve_conflict`. `resolve_goal_text(Option<&str>, &Path)` defined and used in Task 5. `total_elapsed`/`started` defined and used in Task 6.
- **Ordering:** Tasks 1→2→3→4 are sequential (4 depends on 1–3). Tasks 5 and 6 are independent of each other and of 2–4 (they only need Task 1's `worktree.rs` to compile, which it does). Task 7 is last.
