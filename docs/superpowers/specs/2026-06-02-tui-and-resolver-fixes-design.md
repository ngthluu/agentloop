# TUI text containment, no-goal run, and conflict-resolver agent

Date: 2026-06-02
Status: Approved (design)

## Summary

Three focused fixes to `agentloop`:

1. Stop git subprocess output from leaking onto the TUI alternate screen ("broken text").
2. Change the main TUI layout from two side-by-side columns to one column, two rows (Jobs on top, Inbox below).
3. Make the `goal` CLI argument optional so `./agentloop --workspace=...` resumes an existing run (or starts in standby for a fresh workspace).
4. On a merge conflict, spawn a dedicated, unbounded `resolver` agent to resolve the conflict and complete the merge, instead of aborting and bouncing the item.
5. Show the total running time of the session, live in the TUI and printed on exit.

These are independent and can be implemented and reviewed separately.

## 1. Fix the broken text (git output leaking onto the TUI)

### Problem

The TUI runs on the crossterm **alternate screen**. `src/app.rs` redirects the
process's **stderr** (fd 2) to `.agentloop/logs/run.log` via `StderrRedirect` so the
orchestrator's `eprintln!` diagnostics don't scroll over the frame. However, the git
helpers in `src/worktree.rs` run subprocesses with `Command::...status()`, which
**inherits stdout (fd 1)**. fd 1 is *not* redirected, so during integration the git
commands (`worktree add`, `merge`, `worktree remove`, `branch -D`) print messages such
as `Auto-merging <file>`, `CONFLICT (content): ...`, `Automatic merge failed`, and
`Preparing worktree ...` directly onto the alt-screen, on top of the TUI frame. This is
the "free-floating text" observed especially when a merge conflicts.

`gate()` and `worktree::has_commits_ahead()` already use `.output()` (captured), which
is why those paths do not leak — confirming the diagnosis.

### Fix

Make every git helper invoked while the TUI is live capture **both** stdout and stderr
instead of inheriting them.

- In `src/worktree.rs`:
  - Change the `git()` helper from `.status()` to `.output()`. Preserve the existing
    boolean success semantics (`Ok(status.success())`). Append the captured
    stdout+stderr to `.agentloop/logs/run.log` (diagnosable) rather than discarding it.
    Because `create`, `merge`, and `remove` all go through `git()`, they are covered.
  - `has_commits_ahead` already uses `.output()`; leave it (optionally tee its stderr to
    the same log for consistency).
- `src/cli.rs`'s `git()` runs during `bootstrap_workspace`, **before** the TUI starts, so
  it cannot leak onto the alt-screen. Leave it as-is (changing it is optional and out of
  scope).

Note: `run.log` lives under `.agentloop/logs/`, which `app.rs` creates and the
`StderrRedirect` also appends to; concurrent appends from short-lived git `.output()`
writes and the redirected fd are acceptable for a diagnostic log.

### Acceptance

- Triggering a worktree add/remove and a merge (including a conflicting merge) while the
  TUI is up does not print any text outside the rendered frame.
- The captured git output is present in `.agentloop/logs/run.log`.

## 2. Layout: two columns -> one column, two rows

### Problem

In `tui::render()`, the main content area uses a `Horizontal` split of
`[Percentage(50), Percentage(50)]` — Jobs on the left, Inbox on the right. The narrow
50%-width columns truncate long job/inbox lines.

### Fix

Change the main-area split (the non-detail branch) from `Direction::Horizontal` to
`Direction::Vertical`, with **Jobs on top** and **Inbox below**, each using the full
terminal width. Sizing: `[Percentage(60), Percentage(40)]` (Jobs typically outnumber
inbox items).

Everything else is unchanged:
- The top status bar and the footer keep their current vertical layout
  (`Length(1)` / `Min(0)` / footer).
- Tab focus toggling, up/down navigation, border colors, the `❓ {label} — {text}` inbox
  line, the job line format, and the job-detail view (`render_job_detail`) are untouched.

### Acceptance

- The main area renders Jobs as a full-width pane on top and Inbox as a full-width pane
  below.
- Tab still toggles focus between the two panes; up/down still navigates the focused pane;
  Enter still opens a job detail / starts answering an inbox item.

## 3. Make the goal argument optional

### Problem

`src/cli.rs` declares `goal: String` as a required positional argument, so the tool
cannot be run as just `./agentloop --workspace=...`. The goal already persists in
`.agentloop/state/goal.md` after the first run.

### Fix

Change `goal` to `Option<String>` in the `Args` struct and adjust `run()`:

- **Goal provided** (`Some`): unchanged behavior — `bootstrap_workspace(..., &goal, ...)`
  then `fold_rerun_goal(&ws, &goal)` (when not `--fresh`).
- **Goal omitted** (`None`):
  - If `.agentloop/state/goal.md` exists and is non-empty: **resume**. Bootstrap with an
    empty goal string (bootstrap only writes `goal.md` if absent, so the existing goal is
    preserved), skip `fold_rerun_goal`, and read the goal text from `goal.md` to pass into
    the TUI header.
  - If `goal.md` does not exist (fresh workspace): bootstrap with an empty goal (writes an
    empty `goal.md` and an empty `backlog.json`), and start the TUI. With an empty backlog
    the interactive orchestrator reaches standby quickly, so the user can press `a` to add
    the first task.
- `--fresh` combined with an omitted goal wipes `.agentloop` and then proceeds via the
  fresh-workspace path (empty goal, standby).

The headless (non-TTY) and `--dry-run` paths similarly read the goal from `goal.md` when
the argument is omitted; with no goal and an empty backlog, the planner has nothing to do
and the run exits normally.

Implementation detail: introduce a resolved `goal_text: String` early in `run()`
(argument if present, else contents of `goal.md`, else empty) and use it for the TUI
header and any place that currently uses `args.goal`.

### Acceptance

- `./agentloop --workspace=<existing>` with no goal argument resumes the existing run and
  shows the prior goal in the header.
- `./agentloop --workspace=<empty-dir>` with no goal argument starts the TUI in standby;
  pressing `a` and submitting a task begins work.
- `./agentloop "<goal>" --workspace=<dir>` behaves exactly as before.

## 4. Merge conflict -> spawn an unbounded `resolver` agent

### Problem

In `orchestrator::iterate()` integration, when `worktree::merge` returns `false`
(conflict), `worktree::merge` runs `git merge --abort` and the item is set back to
`ready` with note "merge conflict; replan" and reported as `bounced` (the ↺ glyph the
user called a "bounded agent result"). The conflict is never actually resolved; the item
just retries from scratch, often re-conflicting.

### Fix

Replace the abort-and-bounce path for conflicts with a resolver-agent flow.

#### 4a. worktree: surface a conflict without aborting

Add a merge outcome type and a non-aborting merge to `src/worktree.rs`:

```rust
pub enum MergeOutcome { Merged, Conflict }

/// Merge `branch` into the repo's current branch. On success returns Merged. On
/// conflict, leaves the working tree in the conflicted (mid-merge) state and returns
/// Conflict (does NOT abort).
pub fn merge_or_conflict(repo: &Path, branch: &str) -> Result<MergeOutcome>;
```

Implementation: `git merge --no-edit -q <branch>`; success -> `Merged`; failure ->
`Conflict` (no `--abort`). Keep the existing `merge()` (abort-on-conflict) available for
the fallback path, or add a small `abort_merge(repo)` helper. Capture output via the
`git()` helper from fix #1 so nothing prints to the terminal.

#### 4b. config: add a `resolver` role

Add to `templates/config.yaml` under `routing`:

```yaml
resolver: { tool: claude, model: sonnet, flags: "--dangerously-skip-permissions" }
```

Existing workspaces with a `config.yaml` that lacks `resolver` fall back to
`defaults.role` (today `build`) via `Config::resolve_role`, so the feature still works
without a config edit; the template addition is the recommended default.

#### 4c. orchestrator: spawn the resolver on conflict

In the `result_done` branch of `iterate()` integration, when the merge yields a
conflict:

1. Report the resolver as a job so the TUI shows its live log:
   `reporter.dispatch("resolve-<id>", "resolve merge conflict for <id>", tool, model, Some(&log))`
   where `log` is `.agentloop/logs/iter-<n>/resolve-<id>.log` and `tool`/`model` come from
   the resolved `resolver` role.
2. Spawn the resolver agent **in the main workspace** (`ws`), where the conflicted merge
   is in progress, via `spawn::agent_run(cfg, "resolver", &prompt, ws, &log, timeout)`.
3. **Unbounded:** the resolver does not consume the item's `max_attempts` and a conflict
   never marks the item `bounced`/`failed` up front. It runs with no effective wall-clock
   timeout — pass an effectively-infinite duration (e.g. `Duration::from_secs(u64::MAX)`,
   or a sufficiently large value such as `100 * 365 * 24 * 3600` to avoid any overflow in
   `tokio::time::timeout`). The agent is **still registered in `ACTIVE_PGIDS`** by
   `run_with_timeout`, so quitting the TUI / Ctrl-C / SIGTERM still kills it and never
   orphans a running agent. (Tradeoff, accepted by the user: a stuck resolver only stops
   on quit.)
4. After the agent returns, verify resolution in `ws`:
   - No remaining conflicts: `git diff --name-only --diff-filter=U` is empty.
   - The merge was committed: no `.git/MERGE_HEAD` (i.e., `git rev-parse -q --verify
     MERGE_HEAD` fails), or `HEAD` advanced. If conflicts are resolved but the agent left
     the merge staged-but-uncommitted, the orchestrator commits it
     (`git commit --no-edit`).
   - On success: `state::set_status(&bk, id, "done", "")`,
     `reporter.status("resolve-<id>", "merged", ...)` and `reporter.status(id, "merged",
     ...)`, `merged += 1`.
5. On failure (conflicts remain after the agent returns and cannot be committed):
   `abort_merge(ws)`, set the item `ready` with note "merge conflict; resolver failed",
   and `reporter.status(id, "bounced", ...)` — the existing fallback, so a broken resolver
   can never wedge the repo.

#### Resolver prompt (sketch)

```
You are a MERGE-CONFLICT RESOLVER on an autonomous app build. The repository at
<ws> is in the middle of merging branch item/<id> into the main branch and has
conflicts. Resolve every conflict so the merge reflects the intent of BOTH sides:

- title: <item title>
- task:  <item desc>

Steps:
- Inspect conflicted files (git status; git diff). Resolve all <<<<<<< ======= >>>>>>>
  markers, keeping a correct, building result.
- `git add` the resolved files and `git commit --no-edit` to complete the merge.
- Do not change unrelated files. Do not start new work.
```

### Acceptance

- A merge that conflicts spawns a `resolver` job (visible in the TUI with a live log) and
  does not abort the merge up front.
- When the resolver resolves and commits, the item is marked `done`/`merged` and counts
  toward `merged`.
- When the resolver fails, the merge is aborted and the item bounces to `ready` (existing
  behavior), and the repo is left clean (no lingering MERGE_HEAD/conflict markers).
- Quitting the TUI while a resolver is running kills the resolver process (no orphan).

## 5. Total running time

### Problem

The TUI shows per-job working-time but no total elapsed time for the whole session, and
the headless run prints no total either.

### Fix

The TUI re-renders every ~80 ms tick, so the total is computed purely in the UI with no
new events or reporter plumbing.

- In `src/tui.rs`, add a `started: std::time::Instant` field to `AppState`, set to
  `Instant::now()` in `AppState::new()`.
- In `tui::render()`, append the live total to the top status bar using the existing
  `fmt_elapsed`, e.g. `... │ ⏱ {total}` where `total = fmt_elapsed(s.started.elapsed())`.
  This appears in both the normal and standby status-bar strings.

Definition: total = wall-clock time since the TUI launched, counted continuously
(including standby/idle periods and across re-engagements). This is the simplest, least
surprising meaning of "total time of running."

- Headless / `--dry-run` path: in `src/cli.rs`, record an `Instant` at the start of
  `run()` and include the total in the final `eprintln!` line
  (`=== agentloop finished ... in <total> ===`), formatted with `tui::fmt_elapsed`.

### Acceptance

- The TUI status bar shows a live, increasing total-time readout that keeps ticking while
  in standby.
- A headless run prints the total elapsed time on exit.

## Non-goals / why other bounces remain

The remaining `bounced`/`failed` outcomes are legitimate signals, not bugs, and are
unchanged by this work:

- `worker reported done but made no commits` — the branch has no commits ahead of HEAD.
- `worker did not report done` — missing/failed result file (timeout, crash, or
  `status: failed`).
- `needs_input without a question file` — malformed needs_input result.

The bounce reason is already persisted by `state::set_status` in `backlog.json` (`notes`)
and, after fix #1, the git output that explains a merge conflict lands in
`.agentloop/logs/run.log`. **Deferred (out of scope for this plan):** surfacing the
`notes` string directly in the TUI job line/detail. It requires threading a note through
the `Reporter::status` trait and the `Event::JobStatus` variant, which would change
existing public signatures and tests; it is a follow-up enhancement, not part of this
iteration.
