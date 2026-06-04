# Troubleshooting Persistence + Bounce/Failure Reporting + UX Fixes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist every bounce/failure/status transition and every loop artifact (no deletes), add an `agentloop --report` command that lists all bounced and failed cases, fix the TUI header (ellipsized goal + always-visible running time), stop git probes from printing credentials/sha junk to the terminal, handle Ctrl-C cleanly in the TUI path, and add a startup preflight that errors when a configured agent CLI (claude/codex) is not installed.

**Architecture:** A new `src/history.rs` module owns an append-only `.agentloop/state/events.jsonl` plus a generic timestamped `archive_file` helper. The `Reporter` trait gains a `note` parameter on `status()`, and a new `RecordingReporter` decorator (in `src/events.rs`) persists every dispatch/status/iteration through `history::record` before forwarding to the existing stderr/TUI reporters — so no call site can forget to record. Every `remove_file` of a loop artifact becomes an archive move. A new `src/preflight.rs` checks configured tools at startup.

**Tech Stack:** Rust (tokio, serde_json, chrono, ratatui/crossterm, clap). Tests follow the existing offline integration style (`FAKE_AGENT=1` + scripted `stub.sh`, `tests/common/mod.rs`).

---

## Background: the bounce/failure taxonomy (current code)

These are all the places the orchestrator reports `bounced` / `failed` today. The reasons exist only in transient `notes` fields (overwritten on the next status change) and on the TUI (lost at exit) — in TUI mode they never reach `run.log` because `ChannelReporter` sends them to the TUI channel only. That is the gap this plan closes.

**Bounced** (builder re-queued, `reporter.status(id, "bounced", ...)`):
1. Builder reported `needs_input` and had a question file → auto-answered, re-dispatched (`src/orchestrator.rs:519-542`)
2. Builder reported `needs_input` without a question file (malformed) (`src/orchestrator.rs:530-537`)
3. Builder reported `done` but made no commits (`src/orchestrator.rs:547-555`)
4. Merge conflict and the resolver failed (`src/orchestrator.rs:572-586`)

**Failed** (`reporter.status(..., "failed", ...)` or backlog/builder `status:"failed"`):
1. Resolver left unmerged paths (`src/orchestrator.rs:112-114`)
2. Resolver could not commit the merge (`src/orchestrator.rs:116-119`)
3. Resolver aborted — branch commits not contained in HEAD (`src/orchestrator.rs:122-125`)
4. Architect produced an invalid task plan (`src/orchestrator.rs:402-404`)
5. Builder did not report done — missing/invalid result file or `status != done` (`src/orchestrator.rs:591-600`)
6. Builder exceeded `max_attempts` → builder failed + parent redesign (`src/orchestrator.rs:424-444`) — **never reported to the reporter today**
7. Worktree create failed before dispatch (`src/orchestrator.rs:452-470`) — **never reported to the reporter today**
8. Task hit the redesign cap → backlog task failed (`src/orchestrator.rs:239-255`) — **never reported to the reporter today**
9. Customer rejected (reported as `rejected`, `src/orchestrator.rs:634-638`)
10. Gate (`verify.sh`) failure → parent redesign (`src/orchestrator.rs:615-618`)

---

## File map

- Create: `src/history.rs` — events.jsonl record/read, `archive_file`, `report`
- Create: `src/preflight.rs` — required-tool check
- Create: `tests/history_test.rs`, `tests/history_loop_test.rs`, `tests/preflight_test.rs`
- Modify: `src/lib.rs` (two `pub mod` lines)
- Modify: `src/events.rs` (Reporter `note` param, `RecordingReporter`)
- Modify: `src/orchestrator.rs` (status reasons, new failure events, archive-not-delete, gate.log)
- Modify: `src/customer.rs` (archive prior reviews), `src/task_state.rs` (archive redesign.json), `src/inbox.rs` (timestamped answered files)
- Modify: `src/cli.rs` (`--report`, silent git probes, preflight wiring, RecordingReporter wiring)
- Modify: `src/app.rs` (RecordingReporter wiring, SIGINT handler)
- Modify: `src/tui.rs` (`ellipsize`, status-bar layout)
- Modify: `tests/tui_render_test.rs`, `tests/tui_helpers_test.rs`, `tests/cli_bootstrap_test.rs`, `README.md`

---

### Task 1: `src/history.rs` — append-only event history + archive helper

**Files:**
- Create: `src/history.rs`
- Modify: `src/lib.rs`
- Test: `tests/history_test.rs`

- [ ] **Step 1: Write the failing tests**

Create `tests/history_test.rs`:

```rust
use agentloop::history;
use std::path::PathBuf;

fn tmp_ws(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "{prefix}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn record_and_read_round_trip_with_single_line_reason() {
    let ws = tmp_ws("hist-rt");
    history::record(
        &ws,
        "status",
        "task-1-b1",
        "bounced",
        "needs_input: auto-answered\nsecond line dropped",
    );
    history::record(&ws, "status", "task-1-b2", "failed", "did not report done");

    let evs = history::read_events(&ws);
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0]["kind"], "status");
    assert_eq!(evs[0]["id"], "task-1-b1");
    assert_eq!(evs[0]["status"], "bounced");
    let reason = evs[0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("needs_input: auto-answered"));
    assert!(!reason.contains("second line"), "reason is one line");
    assert!(!evs[0]["ts"].as_str().unwrap().is_empty());

    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn read_events_on_missing_file_is_empty() {
    let ws = tmp_ws("hist-none");
    assert!(history::read_events(&ws).is_empty());
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn archive_file_moves_without_overwriting() {
    let ws = tmp_ws("hist-arch");
    let src = ws.join("r.json");
    let dir = ws.join("archive");

    std::fs::write(&src, "one").unwrap();
    history::archive_file(&src, &dir).unwrap();
    std::fs::write(&src, "two").unwrap();
    history::archive_file(&src, &dir).unwrap();

    assert!(!src.exists(), "source moved away");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        2,
        "second archive does not overwrite the first"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn archive_file_missing_source_is_noop() {
    let ws = tmp_ws("hist-arch-miss");
    history::archive_file(&ws.join("absent.json"), &ws.join("archive")).unwrap();
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test history_test`
Expected: FAIL to compile — `agentloop::history` does not exist.

- [ ] **Step 3: Write the implementation**

Create `src/history.rs`:

```rust
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Append-only status-transition history. One JSON object per line; never
/// truncated or rewritten, so bounces/failures survive after the TUI exits.
pub fn events_path(ws: &Path) -> PathBuf {
    ws.join(".agentloop/state/events.jsonl")
}

/// Append one event. Best-effort: history must never break the loop.
pub fn record(ws: &Path, kind: &str, id: &str, status: &str, reason: &str) {
    if let Err(e) = try_record(ws, kind, id, status, reason) {
        eprintln!("history: record failed for {id}: {e:#}");
    }
}

/// First line of `reason`, capped at `max` chars (full output lives in the job
/// logs / gate.log; events.jsonl stays one skimmable line per event).
fn one_line(reason: &str, max: usize) -> String {
    let first = reason.lines().next().unwrap_or("");
    let mut s: String = first.chars().take(max).collect();
    if first.chars().count() > max || reason.lines().count() > 1 {
        s.push('…');
    }
    s
}

fn try_record(ws: &Path, kind: &str, id: &str, status: &str, reason: &str) -> Result<()> {
    let path = events_path(ws);
    let dir = path.parent().context("events path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let ev = json!({
        "ts": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "kind": kind,
        "id": id,
        "status": status,
        "reason": one_line(reason, 500),
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{ev}")?;
    Ok(())
}

/// All recorded events, oldest first. Missing file -> empty; unparseable lines skipped.
pub fn read_events(ws: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(events_path(ws)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Move `path` into `dir` with a timestamp prefix so repeats never overwrite.
/// Missing source is a no-op. This is how loop artifacts are retired: archived
/// for troubleshooting, never deleted.
pub fn archive_file(path: &Path, dir: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("archive: source has no file name")?
        .to_string();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut dest = dir.join(format!("{stamp}-{name}"));
    let mut n = 1u32;
    while dest.exists() {
        dest = dir.join(format!("{stamp}-{n}-{name}"));
        n += 1;
    }
    std::fs::rename(path, &dest).or_else(|_| {
        std::fs::copy(path, &dest)
            .map(|_| ())
            .and_then(|_| std::fs::remove_file(path))
    })?;
    Ok(())
}
```

In `src/lib.rs`, add (alphabetical order):

```rust
pub mod history;
```

between `pub mod events;` and `pub mod inbox;`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test history_test`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add src/history.rs src/lib.rs tests/history_test.rs
git commit -m "feat(history): append-only events.jsonl + timestamped archive helper"
```

---

### Task 2: Record every status transition — Reporter `note` + `RecordingReporter`

The `Reporter::status` signature gains a `note: &str` so failure/bounce reasons travel with the status. A `RecordingReporter` decorator persists every dispatch/status/iteration to events.jsonl, then forwards. The TUI and headless paths are wrapped with it.

**Files:**
- Modify: `src/events.rs`
- Modify: `src/orchestrator.rs` (every `reporter.status(...)` call + two new failure reports)
- Modify: `src/app.rs:115`, `src/cli.rs:211` (wrap reporters)
- Test: `tests/history_test.rs` (decorator unit test), `tests/history_loop_test.rs` (loop integration)

- [ ] **Step 1: Write the failing tests**

Append to `tests/history_test.rs`:

```rust
#[test]
fn recording_reporter_persists_dispatch_and_status() {
    use agentloop::events::{EventLineReporter, RecordingReporter, Reporter};
    use std::sync::Arc;

    let ws = tmp_ws("hist-reporter");
    let rep = RecordingReporter::new(ws.clone(), Arc::new(EventLineReporter));
    rep.dispatch("task-1-b1", "make file", "codex", "gpt-5", None);
    rep.status("task-1-b1", "bounced", "", "", "needs_input: auto-answered");

    let evs = history::read_events(&ws);
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0]["kind"], "dispatch");
    assert_eq!(evs[0]["status"], "running");
    assert_eq!(evs[1]["kind"], "status");
    assert_eq!(evs[1]["status"], "bounced");
    assert_eq!(evs[1]["reason"], "needs_input: auto-answered");

    let _ = std::fs::remove_dir_all(&ws);
}
```

Create `tests/history_loop_test.rs`:

```rust
mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, RecordingReporter, Reporter};
use agentloop::{history, orchestrator};
use std::sync::Arc;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cfg() -> Config {
    serde_json::from_str(r#"{
  "caps": { "max_iterations": 6, "max_parallel": 1, "item_timeout_sec": 30, "total_budget_sec": 300, "max_attempts": 5 },
  "routing": {
    "manager": { "tool": "claude", "model": "opus", "effort": "high" },
    "architect": { "tool": "claude", "model": "opus", "effort": "high" },
    "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" },
    "customer": { "tool": "claude", "model": "sonnet", "effort": "medium" },
    "resolver": { "tool": "claude", "model": "sonnet", "effort": "medium" }
  },
  "defaults": { "role": "builder" }
}"#).unwrap()
}

fn set_env(ws: &std::path::Path) {
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", ws);
}

fn clear_env() {
    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
}

fn recording(ws: &std::path::Path) -> Arc<dyn Reporter> {
    Arc::new(RecordingReporter::new(
        ws.to_path_buf(),
        Arc::new(EventLineReporter),
    ))
}

#[tokio::test]
async fn happy_iteration_records_terminal_events() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_stub();
    set_env(&ws);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &recording(&ws))
        .await
        .unwrap();
    assert_eq!(merged, 1);

    let evs = history::read_events(&ws);
    let has = |status: &str, id: &str| {
        evs.iter()
            .any(|e| e["kind"] == "status" && e["status"] == status && e["id"] == id)
    };
    assert!(has("done", "manager"), "manager done recorded");
    assert!(has("done", "architect-task-1"), "architect done recorded");
    assert!(has("merged", "task-1-b1"), "builder merge recorded");
    assert!(has("approved", "task-1-customer"), "customer approval recorded");

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn needs_input_bounce_is_recorded_with_reason() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_asking_stub();
    set_env(&ws);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &recording(&ws))
        .await
        .unwrap();
    assert_eq!(merged, 0);

    let evs = history::read_events(&ws);
    let bounce = evs
        .iter()
        .find(|e| e["status"] == "bounced" && e["id"] == "task-1-b1")
        .expect("bounce event recorded");
    assert!(
        bounce["reason"].as_str().unwrap().contains("needs_input"),
        "bounce reason names the cause: {bounce}"
    );

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test history_test --test history_loop_test`
Expected: FAIL to compile — `RecordingReporter` does not exist.

- [ ] **Step 3: Change the Reporter trait + reporters in `src/events.rs`**

Replace the trait method and both impls; add the decorator. The trait doc comment line 10-11 becomes:

```rust
    /// A job changed status (done/failed/merged/bounced/...). `note` is the
    /// human-readable reason for failures/bounces ("" when there is none).
    fn status(&self, id: &str, status: &str, tool: &str, model: &str, note: &str);
```

`EventLineReporter::status` becomes:

```rust
    fn status(&self, id: &str, status: &str, tool: &str, model: &str, note: &str) {
        if note.is_empty() {
            eprintln!("{}  {:<9} {:<10} {}/{}", hms(), status, id, tool, model);
        } else {
            eprintln!(
                "{}  {:<9} {:<10} {}/{}  {}",
                hms(),
                status,
                id,
                tool,
                model,
                note
            );
        }
    }
```

`ChannelReporter::status` becomes (Event enum unchanged — the TUI does not display notes):

```rust
    fn status(&self, id: &str, status: &str, _tool: &str, _model: &str, _note: &str) {
        let _ = self.tx.send(Event::JobStatus {
            id: id.into(),
            status: status.into(),
        });
    }
```

Add at the top of `src/events.rs`: `use std::sync::Arc;` — then append at the end of the file:

```rust
/// Decorator that persists every dispatch/status/iteration to
/// `.agentloop/state/events.jsonl` (via crate::history) before forwarding, so
/// bounces and failures survive after the TUI exits and are queryable with
/// `agentloop --report`.
pub struct RecordingReporter {
    ws: PathBuf,
    inner: Arc<dyn Reporter>,
}

impl RecordingReporter {
    pub fn new(ws: PathBuf, inner: Arc<dyn Reporter>) -> Self {
        Self { ws, inner }
    }
}

impl Reporter for RecordingReporter {
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str, log: Option<&Path>) {
        crate::history::record(&self.ws, "dispatch", id, "running", label);
        self.inner.dispatch(id, label, tool, model, log);
    }
    fn status(&self, id: &str, status: &str, tool: &str, model: &str, note: &str) {
        crate::history::record(&self.ws, "status", id, status, note);
        self.inner.status(id, status, tool, model, note);
    }
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64) {
        crate::history::record(
            &self.ws,
            "iteration",
            &format!("iter-{n}"),
            gate,
            &format!("merged={merged} open={open}"),
        );
        self.inner.iteration(n, merged, gate, open);
    }
    fn standby(&self) {
        self.inner.standby();
    }
}
```

- [ ] **Step 4: Update every `reporter.status` call in `src/orchestrator.rs` with a reason**

In `resolve_conflict` (lines 112-127), the four calls become:

```rust
    if worktree::has_unmerged(ws) {
        reporter.status(&rid, "failed", &tool, &model, "resolver left unmerged paths");
        return Ok(false);
    }
    if worktree::merge_in_progress(ws) && !worktree::commit_merge(ws) {
        reporter.status(&rid, "failed", &tool, &model, "resolver could not commit the merge");
        return Ok(false);
    }
    // Guard against a resolver that aborted instead of completing the merge: the
    // branch's commits must now be contained in HEAD.
    if worktree::has_commits_ahead(ws, branch) {
        reporter.status(
            &rid,
            "failed",
            &tool,
            &model,
            "resolver did not complete the merge (branch commits not in HEAD)",
        );
        return Ok(false);
    }
    reporter.status(&rid, "merged", &tool, &model, "");
```

In `iterate`:

- Line 376: `reporter.status("manager", "done", &mtool, &mmodel, "");`
- Line 400: `reporter.status(&aid, "done", &atool, &amodel, "");`
- Line 403: `reporter.status(&aid, "failed", &atool, &amodel, "architect produced invalid task plan");`

In the `att >= maxatt` block (after the `task_state::set_builder_status(..., "failed", ...)` call at lines 428-434), add one line so this failure is reported/recorded:

```rust
                reporter.status(&id, "failed", "", "", &format!("exceeded max_attempts ({maxatt})"));
```

In the worktree-create-failed block (after `task_state::set_builder_status(..., "failed", "worktree create failed")` at lines 453-460), add:

```rust
                reporter.status(&id, "failed", "", "", "worktree create failed before dispatch");
```

Replace the whole `needs_input` block (lines 519-543) so the bounce reason is captured in one place:

```rust
        if status == "needs_input" {
            // Autonomous mode: never park the item on a human. Persist the canned
            // "you decide" answer and re-dispatch with the prior Q&A appended to
            // the builder prompt. Re-dispatch consumes an attempt, so a builder
            // that asks forever still hits max_attempts -> redesign.
            let note = if crate::inbox::has_question(ws, id) {
                if let Err(e) = auto_answer(ws, id) {
                    eprintln!("auto-answer failed for {id}: {e:#}");
                    task_state::set_builder_status(ws, task_id, id, "ready", "auto-answer failed")?;
                    "needs_input: auto-answer failed; re-dispatching"
                } else {
                    "needs_input: auto-answered; re-dispatching"
                }
            } else {
                // Malformed/missing question file: treat as a normal non-done bounce.
                task_state::set_builder_status(
                    ws,
                    task_id,
                    id,
                    "ready",
                    "needs_input without a question file",
                )?;
                "needs_input without a question file"
            };
            reporter.status(id, "bounced", "", "", note);
            worktree::remove(ws, &ws.join(format!(".agentloop/worktrees/{id}")), &branch);
            let _ = std::fs::remove_file(&rfile);
            continue;
        }
```

Remaining `iterate` calls:

- Line 555: `reporter.status(id, "bounced", "", "", "reported done but made no commits");`
- Line 560: `reporter.status(id, "merged", "", "", "");`
- Line 570: `reporter.status(id, "merged", "", "", "");`
- Line 586: `reporter.status(id, "bounced", "", "", "merge conflict; resolver failed");`
- Line 599: `reporter.status(id, "failed", "", "", "did not report done (missing/invalid result file or status != done)");`
- Line 633: `reporter.status(&cid, "approved", &ctool, &cmodel, "");`
- Line 637: `reporter.status(&cid, "rejected", &ctool, &cmodel, &feedback);`

And in `reopen_parent_for_redesign` (lines 232-255), record the task-level outcome (no reporter in scope here; record directly):

```rust
fn reopen_parent_for_redesign(
    bk: &Path,
    ws: &Path,
    task_id: &str,
    note: &str,
    max_redesigns: u32,
) -> Result<()> {
    let count = task_state::bump_redesign(ws, task_id, note)?;
    if count >= max_redesigns {
        // The task keeps failing after its builders complete. Stop the redesign loop:
        // mark it failed (so it leaves the open set) and keep the plan for inspection.
        let reason = format!("redesign cap ({max_redesigns}) reached; last failure: {note}");
        state::set_status(bk, task_id, "failed", &reason)?;
        crate::history::record(ws, "task", task_id, "failed", &reason);
    } else {
        // Under the cap: reopen for a fresh, feedback-informed architect pass.
        state::set_status(bk, task_id, "ready", note)?;
        crate::history::record(ws, "task", task_id, "redesign", note);
        invalidate_task_plan(ws, task_id);
    }
    Ok(())
}
```

- [ ] **Step 5: Wrap the reporters in `src/app.rs` and `src/cli.rs`**

`src/app.rs` — change the import on line 18 and the reporter on line 115:

```rust
use crate::events::{ChannelReporter, Command, Event, RecordingReporter, Reporter};
```

```rust
    let reporter: Arc<dyn Reporter> = Arc::new(RecordingReporter::new(
        ws.clone(),
        Arc::new(ChannelReporter::new(etx)),
    ));
```

`src/cli.rs` — change the import on line 8 and the headless call on line 211:

```rust
use crate::events::{EventLineReporter, RecordingReporter, Reporter};
```

```rust
        let reporter: Arc<dyn Reporter> = Arc::new(RecordingReporter::new(
            ws.clone(),
            Arc::new(EventLineReporter),
        ));
        let rc = orchestrator::run(&cfg, &ws, reporter).await?;
```

- [ ] **Step 6: Build + run the full suite (the trait change ripples)**

Run: `cargo test`
Expected: all green, including the two new test files. If any other file fails to compile on the `status` arity, update it the same way (there are no other `Reporter` impls — only `src/events.rs` has them).

- [ ] **Step 7: Commit**

```bash
git add src/events.rs src/orchestrator.rs src/app.rs src/cli.rs tests/history_test.rs tests/history_loop_test.rs
git commit -m "feat(history): record every job status transition with its reason"
```

---

### Task 3: `agentloop --report` — list all bounced and failed cases

**Files:**
- Modify: `src/history.rs` (add `report`)
- Modify: `src/cli.rs` (flag + early exit)
- Test: `tests/history_test.rs`

- [ ] **Step 1: Write the failing test**

Append to `tests/history_test.rs`:

```rust
#[test]
fn report_lists_bounced_failed_events_and_current_failures() {
    use serde_json::json;

    let ws = tmp_ws("hist-report");
    let st = ws.join(".agentloop/state");
    std::fs::create_dir_all(st.join("tasks/task-9")).unwrap();

    history::record(&ws, "status", "task-1-b1", "bounced", "needs_input: auto-answered");
    history::record(&ws, "status", "task-9-b2", "failed", "did not report done");
    history::record(&ws, "task", "task-9", "failed", "redesign cap (3) reached");

    std::fs::write(
        st.join("backlog.json"),
        serde_json::to_vec(&json!({"items":[{
            "id":"task-9","title":"browse history","deps":[],"status":"failed",
            "attempts":0,"acceptance":"a","notes":"redesign cap (3) reached"
        }]}))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        st.join("tasks/task-9/builders.json"),
        serde_json::to_vec(&json!({"items":[{
            "id":"task-9-b2","title":"t","desc":"d","deps":[],"status":"failed",
            "attempts":3,"acceptance":"a","notes":"exceeded max_attempts (3)"
        }]}))
        .unwrap(),
    )
    .unwrap();

    let r = history::report(&ws);
    assert!(r.contains("BOUNCED events: 1"), "got:\n{r}");
    assert!(r.contains("task-1-b1"));
    assert!(r.contains("needs_input: auto-answered"));
    assert!(r.contains("FAILED events: 1"));
    assert!(r.contains("TASK redesign/failure events: 1"));
    assert!(r.contains("backlog items currently failed: 1"));
    assert!(r.contains("browse history"));
    assert!(r.contains("task-9/task-9-b2 (attempts 3)"));
    assert!(r.contains("exceeded max_attempts (3)"));

    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test history_test report_lists`
Expected: FAIL to compile — `history::report` does not exist.

- [ ] **Step 3: Implement `report` in `src/history.rs`**

Append:

```rust
/// Human-readable troubleshooting report: every bounced/failed/rejected event
/// ever recorded, plus what is failed right now in the backlog and in each
/// task's builder plan.
pub fn report(ws: &Path) -> String {
    let mut out = String::new();
    let events = read_events(ws);
    out.push_str(&format!("=== agentloop report — {} ===\n", ws.display()));
    if events.is_empty() {
        out.push_str("(no events recorded yet — events.jsonl is written by runs from this version on)\n");
    }

    let pick = |status: &str| -> Vec<&Value> {
        events
            .iter()
            .filter(|e| e["kind"] == "status" && e["status"] == status)
            .collect()
    };
    for (title, status) in [
        ("BOUNCED", "bounced"),
        ("FAILED", "failed"),
        ("REJECTED (customer)", "rejected"),
    ] {
        let evs = pick(status);
        out.push_str(&format!("\n{title} events: {}\n", evs.len()));
        for e in evs {
            out.push_str(&format!(
                "  {}  {:<24} {}\n",
                e["ts"].as_str().unwrap_or(""),
                e["id"].as_str().unwrap_or(""),
                e["reason"].as_str().unwrap_or("")
            ));
        }
    }

    let redesigns: Vec<&Value> = events.iter().filter(|e| e["kind"] == "task").collect();
    out.push_str(&format!(
        "\nTASK redesign/failure events: {}\n",
        redesigns.len()
    ));
    for e in redesigns {
        out.push_str(&format!(
            "  {}  {:<24} {:<8} {}\n",
            e["ts"].as_str().unwrap_or(""),
            e["id"].as_str().unwrap_or(""),
            e["status"].as_str().unwrap_or(""),
            e["reason"].as_str().unwrap_or("")
        ));
    }

    let bk = ws.join(".agentloop/state/backlog.json");
    if let Ok(v) = crate::state::read(&bk) {
        let empty = vec![];
        let failed: Vec<&Value> = v["items"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter(|i| i["status"] == "failed")
            .collect();
        out.push_str(&format!(
            "\nbacklog items currently failed: {}\n",
            failed.len()
        ));
        for i in failed {
            out.push_str(&format!(
                "  {}  {}\n    note: {}\n",
                i["id"].as_str().unwrap_or(""),
                i["title"].as_str().unwrap_or(""),
                i["notes"].as_str().unwrap_or("").lines().next().unwrap_or("")
            ));
        }
    }

    out.push_str("\nbuilders currently failed:\n");
    let mut none = true;
    if let Ok(entries) = std::fs::read_dir(ws.join(".agentloop/state/tasks")) {
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        for task_id in names {
            let Ok(b) = crate::task_state::read_builders(ws, &task_id) else {
                continue;
            };
            let empty = vec![];
            for i in b["items"].as_array().unwrap_or(&empty) {
                if i["status"] == "failed" {
                    none = false;
                    out.push_str(&format!(
                        "  {}/{} (attempts {})  {}\n",
                        task_id,
                        i["id"].as_str().unwrap_or(""),
                        i["attempts"].as_u64().unwrap_or(0),
                        i["notes"].as_str().unwrap_or("").lines().next().unwrap_or("")
                    ));
                }
            }
        }
    }
    if none {
        out.push_str("  (none)\n");
    }
    out
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test history_test report_lists`
Expected: PASS.

- [ ] **Step 5: Wire the `--report` flag in `src/cli.rs`**

Add to `Args` (after the `dry_run` field):

```rust
    /// Print the bounce/failure troubleshooting report for the workspace and exit
    #[arg(long)]
    report: bool,
```

In `run()`, right after `let ws = args.workspace.clone().unwrap_or(std::env::current_dir()?);` and **before** the `--fresh` wipe (the report must never modify state):

```rust
    if args.report {
        let ws = ws.canonicalize().unwrap_or(ws);
        print!("{}", crate::history::report(&ws));
        return Ok(());
    }
```

- [ ] **Step 6: Verify the binary path**

Run: `cargo run -- --report --workspace /tmp` (any dir)
Expected: prints a report with zero events and "(none)" sections; exits 0; creates no files in /tmp.

- [ ] **Step 7: Commit**

```bash
git add src/history.rs src/cli.rs tests/history_test.rs
git commit -m "feat(cli): agentloop --report lists all bounced and failed cases"
```

---

### Task 4: Archive instead of delete — results, plans, customer reviews, redesign counters, questions

**Files:**
- Modify: `src/orchestrator.rs` (2 result deletions, `clear_customer_review`, `invalidate_task_plan`)
- Modify: `src/customer.rs:57-68`, `src/task_state.rs:65-68` (`reset_redesign`), `src/inbox.rs:65-73` (`consume_question`)
- Test: `tests/history_loop_test.rs`, `src/orchestrator.rs` module tests

- [ ] **Step 1: Write the failing tests**

Append to `tests/history_loop_test.rs`:

```rust
#[tokio::test]
async fn builder_results_are_archived_into_the_iter_log_dir_not_deleted() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_stub();
    set_env(&ws);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &recording(&ws))
        .await
        .unwrap();
    assert_eq!(merged, 1);

    assert!(
        !ws.join(".agentloop/results/task-1-b1.json").exists(),
        "live results dir is still cleared for the next round"
    );
    let archived = std::fs::read_dir(ws.join(".agentloop/logs/iter-1"))
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().ends_with("-task-1-b1.json"));
    assert!(archived, "builder result archived into .agentloop/logs/iter-1/");

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}
```

In `src/orchestrator.rs`, extend the existing module test `redesign_under_cap_reopens_invalidates_and_records_feedback` — after the existing `!builders_path.exists()` assertion, add:

```rust
        let archive = ws.join(".agentloop/state/tasks/task-1/archive");
        let archived_plan = std::fs::read_dir(&archive)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with("-builders.json"));
        assert!(archived_plan, "invalidated plan is archived, not deleted");
        let events = crate::history::read_events(&ws);
        assert!(
            events
                .iter()
                .any(|e| e["kind"] == "task" && e["id"] == "task-1" && e["status"] == "redesign"),
            "redesign recorded in events.jsonl"
        );
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test history_loop_test builder_results && cargo test --lib redesign_under_cap`
Expected: both FAIL (result file deleted; archive dir missing).

- [ ] **Step 3: Implement the archive moves**

`src/orchestrator.rs` — replace both result-file deletions (in the `needs_input` block and at the end of the integration loop) with archive moves into the iteration's log dir (`ldir` is in scope in `iterate`):

```rust
            let _ = crate::history::archive_file(&rfile, &ldir);
```

(replaces `let _ = std::fs::remove_file(&rfile);` at both sites.)

`src/orchestrator.rs` — replace `clear_customer_review` and `invalidate_task_plan`:

```rust
/// Retire a stale customer review into the task's archive dir (never deleted;
/// rejected reviews are the troubleshooting trail for redesigns).
fn clear_customer_review(ws: &Path, task_id: &str) {
    let dir = task_state::task_dir(ws, task_id).join("archive");
    let _ = crate::history::archive_file(&task_state::customer_path(ws, task_id), &dir);
    let _ = crate::history::archive_file(
        &ws.join(".agentloop/results")
            .join(format!("{task_id}-customer.json")),
        &dir,
    );
}

fn invalidate_task_plan(ws: &Path, task_id: &str) {
    let dir = task_state::task_dir(ws, task_id).join("archive");
    // The next architect pass overwrites design.md in place; keep a copy of the
    // failed design alongside the failed plan.
    let design = task_state::task_dir(ws, task_id).join("design.md");
    if design.exists() {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::copy(&design, dir.join(format!("{stamp}-design.md")));
    }
    let _ = crate::history::archive_file(&task_state::builders_path(ws, task_id), &dir);
    clear_customer_review(ws, task_id);
}
```

`src/customer.rs` — replace the two `remove_file` calls in `customer_run` (lines 58-68):

```rust
    if !id.is_empty() {
        // Archive (never delete) the previous round's review before re-running.
        let dir = crate::task_state::task_dir(ws, id).join("archive");
        let _ = crate::history::archive_file(
            &ws.join(".agentloop/state/tasks").join(id).join("customer.json"),
            &dir,
        );
        let _ = crate::history::archive_file(
            &ws.join(".agentloop/results").join(format!("{id}-customer.json")),
            &dir,
        );
    }
```

`src/task_state.rs` — replace `reset_redesign`:

```rust
/// Retire the redesign counter into the task's archive (called when the task is
/// genuinely completed). Reads as (0, "") afterwards.
pub fn reset_redesign(ws: &Path, task_id: &str) {
    let dir = task_dir(ws, task_id).join("archive");
    let _ = crate::history::archive_file(&redesign_path(ws, task_id), &dir);
}
```

`src/inbox.rs` — replace `consume_question` so repeat questions never overwrite the archived copy:

```rust
/// Archive the question file under logs/ (timestamped, never overwritten) so it
/// isn't re-raised.
pub fn consume_question(ws: &Path, id: &str) -> Result<()> {
    let q = qpath(ws, id);
    if q.exists() {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let dest = ws.join(format!(".agentloop/logs/answered-{id}-{stamp}.json"));
        std::fs::rename(&q, &dest).or_else(|_| std::fs::remove_file(&q))?;
    }
    Ok(())
}
```

- [ ] **Step 4: Run the full suite**

Run: `cargo test`
Expected: all green (existing tests assert absence of the live files — moves keep those passing).

- [ ] **Step 5: Commit**

```bash
git add src/orchestrator.rs src/customer.rs src/task_state.rs src/inbox.rs tests/history_loop_test.rs
git commit -m "feat(persistence): archive results/plans/reviews/questions instead of deleting"
```

---

### Task 5: Persist every gate (verify.sh) run to `gate.log`

`last_gate.txt` keeps only the latest run; append every run (timestamp + rc + full output) to `.agentloop/logs/gate.log`.

**Files:**
- Modify: `src/orchestrator.rs` (`gate`, lines 130-155)
- Test: `src/orchestrator.rs` module tests

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/orchestrator.rs`:

```rust
    #[test]
    fn gate_appends_every_run_to_gate_log() {
        let ws = tmp_ws("orch-gatelog");
        std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();

        assert_eq!(gate(&ws), 1); // no verify.sh yet
        std::fs::write(ws.join(".agentloop/verify.sh"), "#!/bin/bash\nexit 0\n").unwrap();
        assert_eq!(gate(&ws), 0);

        let log = std::fs::read_to_string(ws.join(".agentloop/logs/gate.log")).unwrap();
        assert_eq!(log.matches("=== ").count(), 2, "both runs recorded");
        assert!(log.contains("rc=1") && log.contains("rc=0"));
        assert!(
            ws.join(".agentloop/state/last_gate.txt").exists(),
            "latest-run file still maintained"
        );

        let _ = std::fs::remove_dir_all(&ws);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib gate_appends`
Expected: FAIL — `gate.log` missing.

- [ ] **Step 3: Rewrite `gate` + add the append helper**

Replace `gate` (lines 130-155) with:

```rust
/// Run verify.sh; capture output to last_gate.txt (latest run) and append it to
/// logs/gate.log (every run, forever); return its exit code (1 if absent).
pub fn gate(ws: &Path) -> i32 {
    let gate = ws.join(".agentloop/verify.sh");
    let out = ws.join(".agentloop/state/last_gate.txt");
    let (code, buf): (i32, Vec<u8>) = if gate.exists() {
        match std::process::Command::new("/bin/bash")
            .arg(&gate)
            .current_dir(ws)
            .output()
        {
            Ok(o) => {
                let mut buf = o.stdout.clone();
                buf.extend_from_slice(&o.stderr);
                (o.status.code().unwrap_or(1), buf)
            }
            Err(_) => (1, b"verify.sh spawn failed".to_vec()),
        }
    } else {
        (1, b"no verify.sh yet".to_vec())
    };
    let _ = std::fs::write(&out, &buf);
    append_gate_log(ws, code, &buf);
    code
}

/// Append one gate run (timestamp, rc, full output) to `.agentloop/logs/gate.log`.
fn append_gate_log(ws: &Path, code: i32, output: &[u8]) {
    use std::io::Write;
    let log = ws.join(".agentloop/logs/gate.log");
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
    {
        let _ = writeln!(
            f,
            "=== {} rc={code} ===",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        let _ = f.write_all(output);
        let _ = writeln!(f);
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib gate_appends && cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/orchestrator.rs
git commit -m "feat(gate): append every verify.sh run to logs/gate.log"
```

---

### Task 6: TUI header — ellipsized goal, running time always visible

A long goal currently pushes `iter/gate/open/⏱` off-screen (`src/tui.rs:379-398`). Give the fixed suffix priority and ellipsize the goal into the remaining width.

**Files:**
- Modify: `src/tui.rs`
- Test: `tests/tui_render_test.rs`, `tests/tui_helpers_test.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/tui_render_test.rs`:

```rust
#[test]
fn long_goal_is_ellipsized_and_counters_stay_visible() {
    let long_goal = "Implement a production-ready chat app, has 2 part: FE is a swift mac app, \
                     BE is rust-based. This chat app supports DM chat and group chat, support \
                     emoji picker, attach file. This chat app is secured, e2e encryption everything.";
    let s = started(long_goal);
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();

    assert!(find(&term, "\u{23f1}").is_some(), "⏱ running time visible");
    assert!(find(&term, "open:").is_some(), "open counter visible");
    assert!(find(&term, "iter").is_some(), "iteration counter visible");
    assert!(find(&term, "…").is_some(), "goal is ellipsized");
}
```

Append to `tests/tui_helpers_test.rs`:

```rust
#[test]
fn ellipsize_truncates_on_char_boundaries() {
    use agentloop::tui::ellipsize;
    assert_eq!(ellipsize("hello", 10), "hello");
    assert_eq!(ellipsize("hello", 5), "hello");
    assert_eq!(ellipsize("hello world", 8), "hello w…");
    assert_eq!(ellipsize("hello", 1), "…");
    assert_eq!(ellipsize("hello", 0), "");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --test tui_render_test long_goal && cargo test --test tui_helpers_test ellipsize`
Expected: FAIL (no `ellipsize`; ⏱ pushed off-screen by the long goal).

- [ ] **Step 3: Implement**

Add to `src/tui.rs` (next to `fmt_elapsed`):

```rust
/// Truncate `s` to at most `max` chars, ending in `…` when cut.
pub fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}
```

Replace the top-status-bar block in `render` (lines 379-391):

```rust
    // --- Top status bar: the fixed suffix (iter/gate/open/⏱) always fits; the
    // goal gets the remaining width and is ellipsized so a long goal can't push
    // the counters off-screen. Newlines would break the one-line bar.
    let total = fmt_elapsed(s.total_elapsed());
    let prefix = if s.standby { " ✓ DONE · standby  │  " } else { " " };
    let suffix = format!(
        "  │  iter {}  │  gate: {}  │  open: {}  │  ⏱ {}",
        s.iter, s.gate, s.open, total
    );
    let avail = (chunks[0].width as usize)
        .saturating_sub(prefix.chars().count() + suffix.chars().count());
    let goal = ellipsize(&s.goal.replace('\n', " "), avail);
    let status_text = format!("{prefix}{goal}{suffix}");
```

(The `let status_bar = ...` and `f.render_widget(status_bar, chunks[0]);` lines that follow stay unchanged.)

- [ ] **Step 4: Run the TUI tests**

Run: `cargo test --test tui_render_test --test tui_helpers_test --test tui_viewmodel_test`
Expected: all PASS (including the pre-existing `status_bar_shows_total_time`).

- [ ] **Step 5: Commit**

```bash
git add src/tui.rs tests/tui_render_test.rs tests/tui_helpers_test.rs
git commit -m "fix(tui): ellipsize long goal so iter/gate/open/running-time stay visible"
```

---

### Task 7: Silence startup git probes (Ctrl-C junk lines)

`src/cli.rs:35-43`'s `git()` uses `.status()`, which inherits the terminal's stdout/stderr — so `git config user.email`, `git config user.name`, and `git rev-parse HEAD` probes print the user's email, name, and HEAD sha straight onto the terminal at startup (the junk seen above the `^C`). Capture instead.

**Files:**
- Modify: `src/cli.rs:35-43`
- Test: `tests/cli_bootstrap_test.rs`

- [ ] **Step 1: Write the failing test**

Append to `tests/cli_bootstrap_test.rs`:

```rust
#[test]
fn startup_git_probes_stay_off_the_terminal() {
    let ws = std::env::temp_dir().join(format!(
        "alboot-quiet-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&ws).unwrap();
    // Pre-seed a repo whose config the bootstrap probes will read back.
    let git = |args: &[&str]| {
        assert!(Command::new("git")
            .arg("-C")
            .arg(&ws)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "probe@example.com"]);
    git(&["config", "user.name", "prober"]);
    std::fs::write(ws.join("seed.txt"), "seed").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);

    let cfg = ws.join("config.json");
    let out = Command::new(env!("CARGO_BIN_EXE_agentloop"))
        .arg("--workspace")
        .arg(&ws)
        .arg("--max-iterations")
        .arg("1")
        .env("AGENTLOOP_CONFIG", &cfg)
        .env("FAKE_AGENT", "1")
        .env("FAKE_AGENT_BIN", "/usr/bin/true")
        .env("WS", &ws)
        .output()
        .unwrap();

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !text.contains("probe@example.com"),
        "git probe output leaked to the terminal:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&ws);
}
```

(Note: `AGENTLOOP_CONFIG` keeps the default config write inside the temp dir; `FAKE_AGENT=1` keeps real agents from spawning and — after Task 9 — also skips the preflight check. Piped stdout means the binary takes the headless path and exits quickly on the invalid/empty manager round; the test only asserts on output content, not exit code.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test cli_bootstrap_test startup_git_probes`
Expected: FAIL — captured output contains `probe@example.com` (printed by the inherited-stdout probe).

- [ ] **Step 3: Fix the helper**

Replace `git` in `src/cli.rs` (lines 35-43):

```rust
/// Run git, capturing (and discarding) its output — bootstrap probes like
/// `git config user.email` and `git rev-parse HEAD` must not print onto the
/// user's terminal. Returns whether the command succeeded.
fn git(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test --test cli_bootstrap_test`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs tests/cli_bootstrap_test.rs
git commit -m "fix(cli): capture git probe output so bootstrap prints no email/sha junk"
```

---

### Task 8: Handle SIGINT in the TUI path (clean Ctrl-C)

The TUI installs a SIGTERM-only handler (`src/app.rs:88-106`). A Ctrl-C that lands as a real SIGINT (during startup before raw mode, or during the post-`restore_terminal` shutdown wait) takes the default action: instant death, no `kill_all_agents()` — orphaning in-flight claude/codex. Handle SIGINT identically.

**Files:**
- Modify: `src/app.rs:86-110`

- [ ] **Step 1: Replace the handler**

Replace `install_tui_sigterm_handler` (lines 86-106) and its call site (line 110):

```rust
/// On SIGINT/SIGTERM while the TUI path is active, raw mode may be on and the alt
/// screen active. Restore the terminal best-effort, kill in-flight agents, and exit
/// so nothing is orphaned. (While the TUI event loop runs, raw mode swallows Ctrl-C
/// into a key event; this covers the startup/shutdown windows where raw mode is off.)
#[cfg(unix)]
fn install_tui_signal_handler() {
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            return;
        };
        let code = tokio::select! {
            _ = term.recv() => 143,
            _ = int.recv() => 130,
        };
        let _ = disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        crate::spawn::kill_all_agents();
        std::process::exit(code);
    });
}
```

and in `run_tui`:

```rust
    #[cfg(unix)]
    install_tui_signal_handler();
```

- [ ] **Step 2: Build + run the suite**

Run: `cargo test`
Expected: all green (signal behavior itself is not unit-testable here).

- [ ] **Step 3: Manual verification**

Run: `cargo build --release && ./target/release/agentloop --workspace /tmp/agentloop-sigint-check` in a real terminal; press Ctrl-C on the goal-entry screen, then run it again and press Ctrl-C twice quickly while quitting.
Expected: the shell prompt returns clean each time — no stray email/name/sha lines (Task 7), no garbled raw-mode terminal, and `ps` shows no leftover claude/codex processes. Then `rm -rf /tmp/agentloop-sigint-check`.

- [ ] **Step 4: Commit**

```bash
git add src/app.rs
git commit -m "fix(app): restore terminal and kill agents on SIGINT, not just SIGTERM"
```

---

### Task 9: Preflight — error out when a configured agent CLI is missing

Before any run, every tool the config routes roles to (e.g. `claude`, `codex`) must exist on PATH. Example from the spec: config routes `manager` to `claude`/`haiku` but `claude` is not installed → name the tool and the roles, show the install command, and quit. Skipped under `FAKE_AGENT=1` (tests).

**Files:**
- Create: `src/preflight.rs`
- Modify: `src/lib.rs`, `src/cli.rs`
- Test: `tests/preflight_test.rs`

- [ ] **Step 1: Write the failing tests**

Create `tests/preflight_test.rs`:

```rust
use agentloop::config::Config;
use agentloop::preflight;

fn cfg_with(routing: &str) -> Config {
    serde_json::from_str(&format!(
        r#"{{"routing": {routing}, "defaults": {{"role":"builder"}}}}"#
    ))
    .unwrap()
}

#[test]
fn required_tools_maps_each_tool_to_its_roles() {
    let cfg = cfg_with(
        r#"{"manager":{"tool":"claude","model":"haiku"},"builder":{"tool":"codex"},"customer":{"tool":"claude"}}"#,
    );
    let req = preflight::required_tools(&cfg);
    assert_eq!(
        req.get("claude").unwrap(),
        &vec!["customer".to_string(), "manager".to_string()]
    );
    assert_eq!(req.get("codex").unwrap(), &vec!["builder".to_string()]);
}

#[test]
fn check_fails_when_a_configured_tool_is_not_installed() {
    let cfg = cfg_with(r#"{"manager":{"tool":"claude","model":"haiku"}}"#);
    let err = preflight::check_with_path(&cfg, "/nonexistent-dir").unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("claude"), "names the missing tool: {msg}");
    assert!(msg.contains("manager"), "names the roles that need it: {msg}");
    assert!(msg.contains("install"), "tells the user to install: {msg}");
}

#[test]
fn check_passes_when_tools_are_executable_on_path() {
    let dir = std::env::temp_dir().join(format!(
        "preflight-bin-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for bin in ["claude", "codex"] {
        let p = dir.join(bin);
        std::fs::write(&p, "#!/bin/bash\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let cfg = cfg_with(r#"{"manager":{"tool":"claude"},"builder":{"tool":"codex"}}"#);
    preflight::check_with_path(&cfg, dir.to_str().unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_routing_is_an_error() {
    let cfg = cfg_with("{}");
    let err = preflight::check_with_path(&cfg, "/usr/bin").unwrap_err();
    assert!(format!("{err:#}").contains("routes no roles"));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --test preflight_test`
Expected: FAIL to compile — `agentloop::preflight` does not exist.

- [ ] **Step 3: Implement `src/preflight.rs`**

```rust
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::path::Path;

use crate::config::Config;

/// tool -> sorted roles that route to it, from the config
/// (e.g. {"claude": ["customer", "manager"], "codex": ["builder"]}).
pub fn required_tools(cfg: &Config) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (role, r) in &cfg.routing {
        if let Some(tool) = r.tool.as_deref().filter(|t| !t.is_empty()) {
            out.entry(tool.to_string()).or_default().push(role.clone());
        }
    }
    // BTreeMap iteration over routing is already sorted by role.
    out
}

fn is_executable(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    if !md.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return md.permissions().mode() & 0o111 != 0;
    }
    #[cfg(not(unix))]
    true
}

/// Whether `bin` is an executable file on `path_var` (a PATH-style string).
pub fn on_path(bin: &str, path_var: &str) -> bool {
    std::env::split_paths(path_var).any(|dir| is_executable(&dir.join(bin)))
}

/// Fail fast when a tool the config routes roles to is not installed, naming the
/// missing tool, the roles that need it, and how to install it.
pub fn check_with_path(cfg: &Config, path_var: &str) -> Result<()> {
    let required = required_tools(cfg);
    if required.is_empty() {
        bail!(
            "config routes no roles to any agent tool; set routing.<role>.tool to \"claude\" or \"codex\""
        );
    }
    let missing: Vec<(&String, &Vec<String>)> = required
        .iter()
        .filter(|(tool, _)| !on_path(tool, path_var))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut msg = String::from("missing required agent CLI(s):\n");
    for (tool, roles) in &missing {
        msg.push_str(&format!("  - {tool} (used by roles: {})\n", roles.join(", ")));
    }
    msg.push_str("install them (or change the config routing):\n");
    for (tool, _) in &missing {
        match tool.as_str() {
            "claude" => msg.push_str("  claude: npm install -g @anthropic-ai/claude-code\n"),
            "codex" => msg.push_str("  codex:  npm install -g @openai/codex\n"),
            other => msg.push_str(&format!(
                "  {other}: not a known tool — fix routing.<role>.tool in the config\n"
            )),
        }
    }
    bail!(msg);
}

/// Preflight against the real PATH. Skipped for FAKE_AGENT runs (offline tests).
pub fn check(cfg: &Config) -> Result<()> {
    if std::env::var("FAKE_AGENT").as_deref() == Ok("1") {
        return Ok(());
    }
    check_with_path(cfg, &std::env::var("PATH").unwrap_or_default())
}
```

In `src/lib.rs`, add `pub mod preflight;` between `pub mod orchestrator;` and `pub mod requests;`.

In `src/cli.rs::run()`, right after the `cfg.caps.max_iterations` override block (and before the `is_tty` / dry-run dispatch):

```rust
    crate::preflight::check(&cfg)?;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --test preflight_test && cargo test`
Expected: all green (the FAKE_AGENT skip keeps the loop/bootstrap tests unaffected).

- [ ] **Step 5: Commit**

```bash
git add src/preflight.rs src/lib.rs src/cli.rs tests/preflight_test.rs
git commit -m "feat(preflight): fail fast when a configured agent CLI is not installed"
```

---

### Task 10: Docs + final verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Document the new behavior**

In the `## Layout` block of `README.md`, after the `events.rs` line, add:

```
  history.rs       append-only state/events.jsonl, artifact archiving, --report
  preflight.rs     startup check that configured agent CLIs are installed
```

After the `## Interactive mode (TUI)` section (before `## Layout`), add:

```markdown
## Troubleshooting

Nothing the loop produces is deleted:

- `.agentloop/state/events.jsonl` — append-only history of every dispatch and
  status transition (bounced/failed/merged/approved/rejected/redesign) with its
  reason. `agentloop --report --workspace <dir>` prints all bounced and failed
  cases plus what is currently failed in the backlog and builder plans.
- `.agentloop/logs/iter-N/` — per-iteration agent logs, plus each builder's
  archived result JSON (timestamp-prefixed).
- `.agentloop/logs/gate.log` — every verify.sh run (timestamp, rc, full output);
  `state/last_gate.txt` keeps just the latest.
- `.agentloop/state/tasks/<id>/archive/` — superseded builder plans, designs,
  customer reviews, and redesign counters.
- `.agentloop/logs/answered-<id>-<ts>.json` — consumed agent questions.

Before running, agentloop verifies that every CLI tool the config routes roles
to (claude/codex) is installed, and exits with install instructions otherwise.
```

- [ ] **Step 2: Full verification**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo build --release`
Expected: all tests pass, no clippy warnings, release build OK.

- [ ] **Step 3: Manual smoke (optional but recommended)**

Run `./target/release/agentloop --report --workspace /Users/ngthluu/choscor/test-chat-app` — prints the (empty-events) report plus the 3 currently-failed backlog items and 2 failed builders from the existing state. Do **not** run a live loop against test-chat-app from this plan.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: troubleshooting persistence, --report, and preflight check"
```
