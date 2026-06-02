# TUI Navigation, Working-Time, Stable Frame, and Additive Re-run — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `agentloop` TUI render as one stable frame, let the user navigate between the Jobs and Inbox panes and open a per-job live-log detail view with a real-time working timer, and make a re-run with new goal text add work instead of reporting an instant "Done."

**Architecture:** All UI lives in `src/tui.rs` (a view-model: events→state, keys→commands, plus `render`), driven by `src/app.rs` over channels. Events flow from `src/orchestrator.rs` through the `Reporter` trait (`src/events.rs`). We extend the view-model with focus/view/timer state, thread each job's log path through the dispatch event, redirect the process stderr to a log file while the TUI owns the terminal, and add additive re-run handling in `src/cli.rs`.

**Tech Stack:** Rust 2021, ratatui 0.29 + crossterm 0.28 (TUI), tokio (async orchestrator), nix 0.29 (Unix fd dup for stderr redirect), serde_json (state files). Tests are offline (`cargo test`, no API tokens) plus a manual `scripts/tui_demo.sh` pass-by-eye.

**Spec:** `docs/superpowers/specs/2026-06-02-tui-navigation-and-iterate-design.md`

---

## File Structure

- `src/cli.rs` — MODIFY: additive re-run logic in `run()` (append new goal text as a pending request + accumulate into `goal.md`).
- `src/events.rs` — MODIFY: add `log_path` to `Reporter::dispatch` and `Event::JobDispatched`; update both reporters.
- `src/orchestrator.rs` — MODIFY: pass each job's log path into `reporter.dispatch(...)`.
- `src/tui.rs` — MODIFY: `Job` gains `log_path`/`started`/`frozen`; `AppState` gains `focus`/`view`/`selected_job`/`log_scroll`; new key handling, `fmt_elapsed`, `tail_file`, and a detail-view render branch.
- `src/app.rs` — MODIFY: redirect process stderr to `.agentloop/logs/run.log` for the lifetime of the TUI (Unix).
- `tests/cli_rerun_test.rs` — CREATE: additive re-run behavior.
- `tests/tui_viewmodel_test.rs` — MODIFY: update `JobDispatched` constructions for the new field; add focus/detail and timer tests.
- `tests/tui_helpers_test.rs` — CREATE: `fmt_elapsed` and `tail_file` unit tests.
- `scripts/tui_demo.sh` — MODIFY: extend the by-eye checklist.
- `README.md` — MODIFY: document the new keys and re-run behavior.

Order of work: Task 1 (re-run, fully isolated) → Task 2 (`fmt_elapsed`) → Task 3 (`tail_file`) → Task 4 (event `log_path` plumbing) → Task 5 (`Job` timer + `log_path` fields & apply) → Task 6 (focus/view state + keys) → Task 7 (render integration) → Task 8 (stderr redirect) → Task 9 (demo + docs).

---

## Task 1: Additive re-run in `cli.rs`

A re-run with new goal text (no `--fresh`) must append that text as a pending request and accumulate it into `goal.md`, rather than being ignored. Identical text is a no-op.

**Files:**
- Create: `tests/cli_rerun_test.rs`
- Modify: `src/cli.rs` (the `run()` function, after `bootstrap_workspace` returns and before building `Config`)

- [ ] **Step 1: Write the failing test**

Create `tests/cli_rerun_test.rs`:

```rust
use agentloop::cli;
use agentloop::requests;

fn tmp_ws() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "alrerun-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn new_goal_text_is_appended_as_request_and_accumulated() {
    let ws = tmp_ws();
    // First run: bootstrap writes goal.md with the original goal.
    cli::bootstrap_workspace(&ws, "build a todo app", None).unwrap();

    // Re-run with additional text.
    cli::fold_rerun_goal(&ws, "also add due dates").unwrap();

    let goal = std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap();
    assert!(goal.contains("build a todo app"), "original goal kept");
    assert!(goal.contains("also add due dates"), "new text accumulated");

    let pending = requests::pending(&ws).unwrap();
    assert_eq!(pending, vec!["also add due dates".to_string()]);

    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn identical_rerun_text_is_a_noop() {
    let ws = tmp_ws();
    cli::bootstrap_workspace(&ws, "build a todo app", None).unwrap();

    // Re-run with the exact same goal: nothing added.
    cli::fold_rerun_goal(&ws, "build a todo app").unwrap();

    let pending = requests::pending(&ws).unwrap();
    assert!(pending.is_empty(), "identical text adds no request");

    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test cli_rerun_test`
Expected: FAIL — `cli::fold_rerun_goal` does not exist (compile error `no function or associated item named fold_rerun_goal`).

- [ ] **Step 3: Implement `fold_rerun_goal` and call it from `run()`**

In `src/cli.rs`, add this public function (near `bootstrap_workspace`):

```rust
/// Additive re-run: any new goal text is treated as MORE context layered onto the
/// existing effort, never a different goal. If goal.md already contains the text,
/// this is a no-op (a plain resume). Otherwise the text is queued as a pending
/// request (so the planner folds it into the backlog) and appended to goal.md.
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
```

In `run()`, immediately after the line `let cfg_path = bootstrap_workspace(&ws, &args.goal, args.config.as_deref())?;` and the `let ws = ws.canonicalize()...` line, add (only when not starting fresh):

```rust
    if !args.fresh {
        fold_rerun_goal(&ws, &args.goal)?;
    }
```

Note: on a first run, `bootstrap_workspace` has just written `goal.md` equal to `args.goal`, so `existing.contains(trimmed)` is true and `fold_rerun_goal` is a no-op — first runs are unaffected.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test cli_rerun_test`
Expected: PASS (both tests).

- [ ] **Step 5: Verify the existing bootstrap test still passes**

Run: `cargo test --test cli_bootstrap_test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs tests/cli_rerun_test.rs
git commit -m "feat(cli): re-run with new goal text adds it as more work, not a reset"
```

---

## Task 2: `fmt_elapsed` helper

A pure formatting helper for the working-time display.

**Files:**
- Create: `tests/tui_helpers_test.rs`
- Modify: `src/tui.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/tui_helpers_test.rs`:

```rust
use agentloop::tui::fmt_elapsed;
use std::time::Duration;

#[test]
fn formats_seconds_minutes_hours() {
    assert_eq!(fmt_elapsed(Duration::from_secs(0)), "0s");
    assert_eq!(fmt_elapsed(Duration::from_secs(7)), "7s");
    assert_eq!(fmt_elapsed(Duration::from_secs(59)), "59s");
    assert_eq!(fmt_elapsed(Duration::from_secs(60)), "1m00s");
    assert_eq!(fmt_elapsed(Duration::from_secs(192)), "3m12s");
    assert_eq!(fmt_elapsed(Duration::from_secs(3599)), "59m59s");
    assert_eq!(fmt_elapsed(Duration::from_secs(3600)), "1h00m");
    assert_eq!(fmt_elapsed(Duration::from_secs(3600 + 5 * 60)), "1h05m");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test tui_helpers_test`
Expected: FAIL — `fmt_elapsed` not found.

- [ ] **Step 3: Implement `fmt_elapsed`**

In `src/tui.rs`, add at module scope (after the `use` lines near the top, outside any function), and make it public so the test can reach it:

```rust
/// Human working-time: "{s}s" under a minute, "{m}m{s:02}s" under an hour,
/// else "{h}h{m:02}m".
pub fn fmt_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test tui_helpers_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tui.rs tests/tui_helpers_test.rs
git commit -m "feat(tui): fmt_elapsed working-time formatter"
```

---

## Task 3: `tail_file` helper

Reads the last lines of a log file for the detail view. Bounded by line and byte count so a huge log never blows up a render tick.

**Files:**
- Modify: `src/tui.rs`
- Modify: `tests/tui_helpers_test.rs`

- [ ] **Step 1: Write the failing test**

Append to `tests/tui_helpers_test.rs`:

```rust
use agentloop::tui::tail_file;

#[test]
fn tail_file_returns_last_lines_or_placeholder() {
    // Missing file -> placeholder.
    let missing = std::env::temp_dir().join("altail-does-not-exist.log");
    let _ = std::fs::remove_file(&missing);
    assert_eq!(tail_file(&missing, 10, 4096), vec!["(no output yet)".to_string()]);

    // File with more lines than the cap -> only the last `max_lines`.
    let p = std::env::temp_dir().join(format!(
        "altail-{}.log",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&p, &body).unwrap();
    let last = tail_file(&p, 3, 4096);
    assert_eq!(last, vec!["line 18".to_string(), "line 19".to_string(), "line 20".to_string()]);
    let _ = std::fs::remove_file(&p);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test tui_helpers_test tail_file_returns_last_lines_or_placeholder`
Expected: FAIL — `tail_file` not found.

- [ ] **Step 3: Implement `tail_file`**

In `src/tui.rs`, add at module scope:

```rust
/// Last `max_lines` lines of `path` (reading at most the final `max_bytes`),
/// for the job-detail log view. Returns a single "(no output yet)" line when the
/// file is missing or empty.
pub fn tail_file(path: &std::path::Path, max_lines: usize, max_bytes: u64) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let placeholder = || vec!["(no output yet)".to_string()];
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return placeholder(),
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return placeholder();
    }
    let start = len.saturating_sub(max_bytes);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return placeholder();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return placeholder();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
    }
    if lines.is_empty() {
        placeholder()
    } else {
        lines
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test tui_helpers_test`
Expected: PASS (both helper tests).

- [ ] **Step 5: Commit**

```bash
git add src/tui.rs tests/tui_helpers_test.rs
git commit -m "feat(tui): tail_file helper for job-detail log view"
```

---

## Task 4: Thread `log_path` through the dispatch event

The detail view needs each job's log path. The orchestrator already computes it; carry it through `Reporter::dispatch` and `Event::JobDispatched`.

**Files:**
- Modify: `src/events.rs`
- Modify: `src/orchestrator.rs` (lines ~51-55 planner dispatch; ~99-117 worker dispatch)
- Modify: `tests/tui_viewmodel_test.rs` (existing `JobDispatched` constructions)

- [ ] **Step 1: Update the `Reporter` trait and `Event` in `events.rs`**

Add `use std::path::{Path, PathBuf};` to the top of `src/events.rs`.

Change the trait method signature:

```rust
    /// A job (planner or worker) has been dispatched. `log` is the job's log file.
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str, log: Option<&Path>);
```

Update `Event::JobDispatched`:

```rust
    JobDispatched {
        id: String,
        label: String,
        tool: String,
        model: String,
        log_path: Option<PathBuf>,
    },
```

Update `EventLineReporter::dispatch` (ignores the path):

```rust
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str, _log: Option<&Path>) {
        eprintln!("{}  dispatch {:<10} {}/{}  {}", hms(), id, tool, model, label);
    }
```

Update `ChannelReporter::dispatch`:

```rust
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str, log: Option<&Path>) {
        let _ = self.tx.send(Event::JobDispatched {
            id: id.into(),
            label: label.into(),
            tool: tool.into(),
            model: model.into(),
            log_path: log.map(|p| p.to_path_buf()),
        });
    }
```

- [ ] **Step 2: Update the dispatch call sites in `orchestrator.rs`**

For the planner (around line 51-55), build the planner log path and pass it:

```rust
    reporter.dispatch("planner", "planning", &ptool, &pmodel, Some(&ldir.join("planner.log")));
```

For the worker, move the `log` binding above the dispatch call. Replace the block around lines 103-108 so it reads:

```rust
        let label = item["title"].as_str().unwrap_or("").to_string();
        let log = ldir.join(format!("item-{id}.log"));
        reporter.dispatch(&id, &label, &tool, &model, Some(&log));

        let cfg2 = cfg.clone();
        let ws2 = ws.to_path_buf();
        let item2: Value = item.clone();
        let id2 = id.clone();
```

(The later `tokio::spawn` closure already moves `log` into `worker::worker_dispatch(..., &log, ...)`; it now refers to this hoisted binding. Delete the old `let log = ldir.join(format!("item-{id}.log"));` line that previously sat below the dispatch call so it is not declared twice.)

- [ ] **Step 3: Update existing view-model test constructions**

In `tests/tui_viewmodel_test.rs`, every `Event::JobDispatched { ... }` literal must add `log_path: None`. On line 8:

```rust
    s.apply(Event::JobDispatched { id: "it-1".into(), label: "scaffold".into(), tool: "codex".into(), model: "gpt-5".into(), log_path: None });
```

- [ ] **Step 4: Update `AppState::apply` to ignore the new field for now**

In `src/tui.rs`, the `Event::JobDispatched` arm currently destructures `{ id, label, tool, model }`. Change the pattern to `{ id, label, tool, model, log_path }` and, for now, bind it so the code compiles (Task 5 will store it):

```rust
            Event::JobDispatched { id, label, tool, model, log_path: _ } => {
```

- [ ] **Step 5: Build and run the full test suite**

Run: `cargo build && cargo test`
Expected: PASS — the project compiles and all existing tests pass with the new signature.

- [ ] **Step 6: Commit**

```bash
git add src/events.rs src/orchestrator.rs src/tui.rs tests/tui_viewmodel_test.rs
git commit -m "feat(events): carry job log_path through the dispatch event"
```

---

## Task 5: `Job` timer + `log_path` fields and apply logic

Store the log path and a real-time working timer on each `Job`.

**Files:**
- Modify: `src/tui.rs` (`Job` struct, `AppState::apply`)
- Modify: `tests/tui_viewmodel_test.rs` (add timer/log_path assertions)

- [ ] **Step 1: Write the failing test**

Append to `tests/tui_viewmodel_test.rs`:

```rust
#[test]
fn dispatch_starts_timer_and_stores_log_path_then_freezes() {
    use std::path::PathBuf;
    let mut s = AppState::new("g".into());
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: Some(PathBuf::from("/tmp/item-it-1.log")),
    });
    let j = s.jobs.iter().find(|j| j.id == "it-1").unwrap();
    assert!(j.started.is_some(), "timer starts on dispatch");
    assert!(j.frozen.is_none(), "not frozen while running");
    assert_eq!(j.log_path.as_deref(), Some(std::path::Path::new("/tmp/item-it-1.log")));

    s.apply(Event::JobStatus { id: "it-1".into(), status: "merged".into() });
    let j = s.jobs.iter().find(|j| j.id == "it-1").unwrap();
    assert!(j.frozen.is_some(), "timer freezes on a terminal status");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test tui_viewmodel_test dispatch_starts_timer_and_stores_log_path_then_freezes`
Expected: FAIL — `Job` has no `started` / `frozen` / `log_path` fields (compile error).

- [ ] **Step 3: Extend the `Job` struct**

In `src/tui.rs`, replace the `Job` struct with:

```rust
#[derive(Clone)]
pub struct Job {
    pub id: String,
    pub label: String,
    pub tool: String,
    pub model: String,
    pub status: String,
    pub log_path: Option<std::path::PathBuf>,
    pub started: Option<std::time::Instant>,
    pub frozen: Option<std::time::Duration>,
}
```

- [ ] **Step 4: Update `AppState::apply` for dispatch and status**

Replace the `Event::JobDispatched` arm with one that starts the timer and stores the path (a re-dispatch of an existing job restarts its timer):

```rust
            Event::JobDispatched { id, label, tool, model, log_path } => {
                let now = std::time::Instant::now();
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    j.label = label;
                    j.tool = tool;
                    j.model = model;
                    j.status = "running".into();
                    j.log_path = log_path;
                    j.started = Some(now);
                    j.frozen = None;
                } else {
                    self.jobs.push(Job {
                        id,
                        label,
                        tool,
                        model,
                        status: "running".into(),
                        log_path,
                        started: Some(now),
                        frozen: None,
                    });
                }
            }
```

Replace the `Event::JobStatus` arm so a terminal status freezes the timer:

```rust
            Event::JobStatus { id, status } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    let terminal = matches!(status.as_str(), "merged" | "done" | "failed" | "bounced");
                    if terminal && j.frozen.is_none() {
                        j.frozen = j.started.map(|s| s.elapsed());
                    }
                    j.status = status;
                }
            }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test tui_viewmodel_test`
Expected: PASS (all view-model tests, including the new one).

- [ ] **Step 6: Commit**

```bash
git add src/tui.rs tests/tui_viewmodel_test.rs
git commit -m "feat(tui): per-job log path + real-time working timer"
```

---

## Task 6: Focus / view state + key handling

Add pane focus (Jobs ↔ Inbox), a job-detail view, and the keys to drive them. Logic only — rendering is Task 7.

**Files:**
- Modify: `src/tui.rs` (`AppState` fields, `on_key`, small accessors)
- Modify: `tests/tui_viewmodel_test.rs` (focus + detail navigation tests)

- [ ] **Step 1: Write the failing tests**

Append to `tests/tui_viewmodel_test.rs`:

```rust
#[test]
fn tab_toggles_focus_and_enter_opens_job_detail() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;
    let mut s = AppState::new("g".into());
    s.apply(Event::JobDispatched {
        id: "it-1".into(), label: "scaffold".into(), tool: "codex".into(),
        model: "gpt-5".into(), log_path: Some(PathBuf::from("/tmp/x.log")),
    });

    // Default focus is Inbox; Tab moves it to Jobs.
    assert!(!s.focus_is_jobs());
    assert!(s.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).is_none());
    assert!(s.focus_is_jobs());

    // Enter on the Jobs pane opens the detail view; no command emitted.
    assert!(s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());
    assert!(s.in_job_detail());

    // Esc returns to the list.
    assert!(s.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).is_none());
    assert!(!s.in_job_detail());
}

#[test]
fn enter_on_inbox_focus_still_answers() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = AppState::new("g".into());
    s.apply(Event::QuestionRaised { item_id: "db".into(), label: "db".into(), text: "q?".into(), context: "".into() });
    // Focus defaults to Inbox: Enter opens the answer editor (no command yet).
    assert!(s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());
    for c in "yes".chars() { s.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)); }
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::AnswerQuestion { ref item_id, .. }) if item_id == "db"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test tui_viewmodel_test tab_toggles_focus_and_enter_opens_job_detail`
Expected: FAIL — `focus_is_jobs` / `in_job_detail` not found.

- [ ] **Step 3: Add the `Focus` / `View` enums and `AppState` fields**

In `src/tui.rs`, add near the `Mode` enum:

```rust
#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Jobs,
    Inbox,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    List,
    JobDetail,
}
```

Add fields to `AppState` (after `input: String,`):

```rust
    focus: Focus,
    view: View,
    selected_job: usize,
    log_scroll: u16,
```

Initialize them in `AppState::new` (after `input: String::new(),`):

```rust
            focus: Focus::Inbox,
            view: View::List,
            selected_job: 0,
            log_scroll: 0,
```

- [ ] **Step 4: Rewrite the `Mode::Normal` key arm and add accessors**

Replace the entire `Mode::Normal => match k.code { ... }` block in `on_key` with one that branches on the current view and focus:

```rust
            Mode::Normal => {
                // Detail view has its own keys.
                if self.view == View::JobDetail {
                    match k.code {
                        KeyCode::Esc => {
                            self.view = View::List;
                            self.log_scroll = 0;
                        }
                        KeyCode::Up => {
                            self.log_scroll = self.log_scroll.saturating_add(1);
                        }
                        KeyCode::Down => {
                            self.log_scroll = self.log_scroll.saturating_sub(1);
                        }
                        KeyCode::Char('q') => return Some(Command::Quit),
                        _ => {}
                    }
                    return None;
                }
                match k.code {
                    KeyCode::Char('q') => Some(Command::Quit),
                    KeyCode::Char('a') => {
                        self.mode = Mode::AddingTask;
                        self.input.clear();
                        None
                    }
                    KeyCode::Tab => {
                        self.focus = match self.focus {
                            Focus::Jobs => Focus::Inbox,
                            Focus::Inbox => Focus::Jobs,
                        };
                        None
                    }
                    KeyCode::Up => {
                        match self.focus {
                            Focus::Jobs => {
                                if self.selected_job > 0 {
                                    self.selected_job -= 1;
                                }
                            }
                            Focus::Inbox => {
                                if self.selected > 0 {
                                    self.selected -= 1;
                                }
                            }
                        }
                        None
                    }
                    KeyCode::Down => {
                        match self.focus {
                            Focus::Jobs => {
                                if self.selected_job + 1 < self.jobs.len() {
                                    self.selected_job += 1;
                                }
                            }
                            Focus::Inbox => {
                                if self.selected + 1 < self.inbox.len() {
                                    self.selected += 1;
                                }
                            }
                        }
                        None
                    }
                    KeyCode::Enter => {
                        match self.focus {
                            Focus::Jobs => {
                                if !self.jobs.is_empty() {
                                    self.view = View::JobDetail;
                                    self.log_scroll = 0;
                                }
                            }
                            Focus::Inbox => {
                                if !self.inbox.is_empty() {
                                    self.mode = Mode::Answering;
                                    self.input.clear();
                                }
                            }
                        }
                        None
                    }
                    _ => None,
                }
            }
```

Add these accessors in the `impl AppState` block (near `mode_is_answering`):

```rust
    pub fn focus_is_jobs(&self) -> bool {
        self.focus == Focus::Jobs
    }

    pub fn in_job_detail(&self) -> bool {
        self.view == View::JobDetail
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test tui_viewmodel_test`
Expected: PASS (all view-model tests, including the existing answer/add-task tests — they rely on the default Inbox focus, which is preserved).

- [ ] **Step 6: Commit**

```bash
git add src/tui.rs tests/tui_viewmodel_test.rs
git commit -m "feat(tui): pane focus (Tab) + job-detail view navigation"
```

---

## Task 7: Render integration — focus borders, elapsed column, detail view

Wire the new state into `render`: focused-pane highlight, a working-time column in the Jobs list, and the job-detail layout.

**Files:**
- Modify: `src/tui.rs` (`render`)

This task has no unit test (ratatui rendering is verified by eye via `tui_demo.sh` in Task 9). Each step is a focused edit; build after each.

- [ ] **Step 1: Add a job-duration helper**

In `src/tui.rs`, add a small method on `AppState` (or a free fn) used by render:

```rust
impl Job {
    /// Frozen duration if finished, else live elapsed since dispatch, else None.
    pub fn elapsed(&self) -> Option<std::time::Duration> {
        self.frozen.or_else(|| self.started.map(|s| s.elapsed()))
    }
}
```

- [ ] **Step 2: Show the working time in the Jobs list and highlight focus**

In `render`, replace the Jobs-list construction (the `job_items` map and the `jobs_list` block) with:

```rust
    let job_items: Vec<ListItem> = s
        .jobs
        .iter()
        .enumerate()
        .map(|(i, j)| {
            let glyph = status_glyph(&j.status);
            let dur = j.elapsed().map(fmt_elapsed).unwrap_or_default();
            let line = format!(" {} {} [{}/{}]  {}", glyph, j.label, j.tool, j.model, dur);
            let style = if s.focus_is_jobs() && i == s.selected_job {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(line)).style(style)
        })
        .collect();
    let jobs_border = if s.focus_is_jobs() { Color::Yellow } else { Color::Blue };
    let jobs_list = List::new(job_items).block(
        Block::default()
            .title(" Jobs ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(jobs_border)),
    );
    f.render_widget(jobs_list, main_chunks[0]);
```

- [ ] **Step 3: Highlight the Inbox border only when focused**

In the Inbox-list block, replace the fixed `Color::Magenta` border with a focus-aware color. Change the inbox item-selection style so the highlight tracks focus, and the border:

```rust
    let inbox_items: Vec<ListItem> = s
        .inbox
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let style = if !s.focus_is_jobs() && i == s.selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(format!(" ❓ {} — {}", p.label, p.text))).style(style)
        })
        .collect();
    let inbox_border = if s.focus_is_jobs() { Color::Magenta } else { Color::Yellow };
    let inbox_list = List::new(inbox_items).block(
        Block::default()
            .title(" Inbox ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(inbox_border)),
    );
    f.render_widget(inbox_list, main_chunks[1]);
```

- [ ] **Step 4: Render the detail view instead of the two panes when active**

Wrap the main-area rendering so that when `s.in_job_detail()` is true, the detail layout is drawn into `chunks[1]` instead of the `main_chunks` split. Add this near the top of the main-area section, replacing the `let main_chunks = ...; ... f.render_widget(inbox_list, main_chunks[1]);` region with a conditional. Concretely, guard the two-pane block and add a detail branch:

```rust
    if s.in_job_detail() {
        render_job_detail(f, s, chunks[1]);
    } else {
        // --- two-pane Jobs + Inbox (existing code from Steps 2-3) ---
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[1]);
        // ... job_items / jobs_list / inbox_items / inbox_list from Steps 2-3 ...
        f.render_widget(jobs_list, main_chunks[0]);
        f.render_widget(inbox_list, main_chunks[1]);
    }
```

Then add the `render_job_detail` free function at the bottom of `src/tui.rs`:

```rust
fn render_job_detail(f: &mut ratatui::Frame, s: &AppState, area: ratatui::layout::Rect) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Borders, Paragraph};

    let job = s.jobs.get(s.selected_job);
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(0)])
        .split(area);

    let (title, header_lines) = match job {
        Some(j) => {
            let dur = j.elapsed().map(fmt_elapsed).unwrap_or_default();
            (
                format!(" Job: {} — {} ", j.id, j.label),
                vec![
                    Line::from(format!(
                        " status: {} {}   role/tool: {}/{}   {}",
                        status_glyph(&j.status), j.status, j.tool, j.model, dur
                    )),
                ],
            )
        }
        None => (" Job ".to_string(), vec![Line::from(" (no job selected)")]),
    };

    let header = Paragraph::new(header_lines).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(header, parts[0]);

    // Log tail.
    let lines: Vec<Line> = match job.and_then(|j| j.log_path.as_deref()) {
        Some(path) => tail_file(path, 400, 32 * 1024)
            .into_iter()
            .map(Line::from)
            .collect(),
        None => vec![Line::from("(no output yet)")],
    };
    let body = parts[1];
    // Show the lines that fit, honoring log_scroll as an offset from the bottom.
    let visible = body.height.saturating_sub(2) as usize; // minus the borders
    let total = lines.len();
    let scroll = (s.log_scroll as usize).min(total.saturating_sub(visible.max(1)));
    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(visible);
    let shown: Vec<Line> = lines[start..end].to_vec();
    let log = Paragraph::new(shown).block(
        Block::default()
            .title(" log — [↑↓] scroll  [esc] back ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(log, body);
}
```

- [ ] **Step 5: Update the footer hint for the list view**

In the final `else` branch of the footer (the non-editing, non-standby case), update the hint text to mention Tab and detail:

```rust
        Paragraph::new(Line::from(
            " [tab] switch pane  [↑↓] navigate  [enter] open/answer  [a] add task  [q] quit",
        ))
```

- [ ] **Step 6: Build and run the full suite**

Run: `cargo build && cargo test`
Expected: PASS — compiles and all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/tui.rs
git commit -m "feat(tui): render focus highlight, working-time column, job-detail log view"
```

---

## Task 8: Redirect process stderr while the TUI owns the terminal

Stop orchestrator `eprintln!` diagnostics from scrolling over the alt-screen by sending the process's stderr to `.agentloop/logs/run.log` for the TUI's lifetime, restoring it on exit.

**Files:**
- Modify: `src/app.rs`

No unit test (fd redirection + terminal behavior is verified by eye in Task 9). Build-checked here.

- [ ] **Step 1: Add a Unix stderr-redirect guard to `app.rs`**

At the top of `src/app.rs`, add:

```rust
use std::path::Path;
```

Add this guard type near the other helpers (e.g. after `restore_terminal`):

```rust
/// While alive, the process's stderr (fd 2) is redirected to a log file so the
/// orchestrator's `eprintln!` diagnostics don't scroll over the alt-screen TUI.
/// Dropping it restores the original stderr. Unix-only; a no-op elsewhere.
#[cfg(unix)]
struct StderrRedirect {
    saved: std::os::unix::io::RawFd,
}

#[cfg(unix)]
impl StderrRedirect {
    fn to_log(log_dir: &Path) -> Option<Self> {
        use std::os::unix::io::AsRawFd;
        let _ = std::fs::create_dir_all(log_dir);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("run.log"))
            .ok()?;
        let stderr_fd = std::io::stderr().as_raw_fd();
        let saved = nix::unistd::dup(stderr_fd).ok()?;
        if nix::unistd::dup2(file.as_raw_fd(), stderr_fd).is_err() {
            let _ = nix::unistd::close(saved);
            return None;
        }
        Some(StderrRedirect { saved })
    }
}

#[cfg(unix)]
impl Drop for StderrRedirect {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        let stderr_fd = std::io::stderr().as_raw_fd();
        let _ = nix::unistd::dup2(self.saved, stderr_fd);
        let _ = nix::unistd::close(self.saved);
    }
}
```

- [ ] **Step 2: Engage the redirect for the TUI lifetime**

In `run_tui`, immediately after `let mut term = setup_terminal()?;`, add:

```rust
    #[cfg(unix)]
    let _stderr_guard = StderrRedirect::to_log(&ws.join(".agentloop/logs"));
```

Because `_stderr_guard` lives until the end of `run_tui`, stderr is restored (via `Drop`) after the event loop and `restore_terminal`. Keep `restore_terminal(&mut term)` where it is; the guard drops at function scope-end, after the terminal is restored, so the final summary printed by `cli::run()` still reaches the real terminal.

- [ ] **Step 3: Build**

Run: `cargo build`
Expected: compiles with no errors (and no new warnings about unused `Path` — it is used by `to_log`).

- [ ] **Step 4: Run the full suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "fix(tui): redirect stderr to run.log so diagnostics don't scroll the frame"
```

---

## Task 9: Demo checklist + docs

Update the by-eye verification checklist and user-facing docs.

**Files:**
- Modify: `scripts/tui_demo.sh` (comment block only)
- Modify: `README.md`

- [ ] **Step 1: Extend the demo checklist**

In `scripts/tui_demo.sh`, add to the "What to verify by eye" comment block these items (after the existing numbered list):

```bash
#   6. The frame stays in one place — no panel piling up in scrollback while the
#      planner/workers run (stderr now goes to .agentloop/logs/run.log).
#   7. Press  tab  to focus the Jobs pane (its border highlights). Use  up/down  to
#      pick a job, then  enter  to open its detail view: a header with status, role,
#      tool/model, and a live ticking working-time, plus the tail of the job's log.
#   8. Press  esc  to return to the two-pane list.
#   9. Each job row shows a working-time that ticks while running and freezes when the
#      job merges/fails.
```

- [ ] **Step 2: Run the demo and verify by eye**

Run: `./scripts/tui_demo.sh`
Expected (verify each): single stable frame (no scrollback pile-up); `tab` highlights the Jobs pane; `enter` opens a job detail with a live log tail and a ticking timer; `esc` returns; job rows show working-times that freeze on completion; `q` restores the terminal cleanly. Then confirm `cat "$WS/.agentloop/logs/run.log"` contains the orchestrator diagnostics that used to bleed onto the screen.

- [ ] **Step 3: Update the README**

In `README.md`, in the "Interactive mode (TUI)" key list, replace the keys block with:

```markdown
- `tab` — switch focus between the Jobs and Inbox panes (the focused pane is highlighted)
- `↑`/`↓` — navigate the focused pane (jobs or the question inbox)
- `enter` — on the Jobs pane: open the selected job's detail (live log tail + a real-time
  working timer); on the Inbox: answer the selected question (type, `enter` submit, `esc` cancel)
- `esc` — leave the job-detail view
- `a` — add a task (type a natural-language request, `enter` to submit)
- `q` — quit
```

And in the "How it works" section, add a bullet under the existing list:

```markdown
- **Re-run = more context:** re-running with new goal text (without `--fresh`) appends it
  to `goal.md` and queues it as a pending request, so the planner folds it into the backlog
  as new tasks and the loop re-engages — instead of reporting an instant "Done, nothing changed."
```

- [ ] **Step 4: Commit**

```bash
git add scripts/tui_demo.sh README.md
git commit -m "docs: TUI navigation/detail keys, working-time, re-run behavior; demo checklist"
```

---

## Self-Review

**Spec coverage:**
- Feature 1 (stable frame) → Task 8 (stderr redirect) + Task 9 by-eye verification. ✓
- Feature 2 (navigate + detail) → Task 4 (log_path event), Task 5 (Job.log_path), Task 6 (focus/view + keys), Task 7 (render + detail), Task 3 (tail_file). ✓
- Feature 3 (real-time working time) → Task 2 (fmt_elapsed), Task 5 (timer fields + freeze), Task 7 (render column + detail header). ✓
- Feature 4 (additive re-run) → Task 1. ✓
- Testing section → offline tests in Tasks 1-6, manual demo in Task 9. ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code. The Task 7 two-pane `else` block references "existing code from Steps 2-3" but those steps provide the complete code immediately above in the same task, so an engineer reading in order has it. ✓

**Type consistency:** `fmt_elapsed(Duration) -> String`, `tail_file(&Path, usize, u64) -> Vec<String>`, `Job { log_path: Option<PathBuf>, started: Option<Instant>, frozen: Option<Duration> }`, `Job::elapsed() -> Option<Duration>`, `Reporter::dispatch(.., log: Option<&Path>)`, `Event::JobDispatched { .., log_path: Option<PathBuf> }`, accessors `focus_is_jobs()`/`in_job_detail()` — all used consistently across tasks. ✓
