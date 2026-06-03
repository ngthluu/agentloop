# TUI Scroll + Architect/Manager Redesign-Loop Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the TUI Jobs/Inbox panes scroll to follow the selection, and stop the architect from regenerating an identical `builders.json` forever when a completed task keeps failing the global gate or customer review.

**Architecture:** Part A switches the two `List` widgets in `src/tui.rs` from stateless `render_widget` to `render_stateful_widget` with a per-frame `ListState` whose `select(...)` is driven by the existing `selected_job` / `selected` indices — ratatui then auto-scrolls to keep the highlighted row visible. Part B fixes the redesign loop with two coordinated changes: (1) the architect now receives the previous failure feedback so a redesign differs from the last one, and (2) an orchestrator-owned `redesign.json` counter per task caps redesigns so a task that keeps failing post-completion is marked `failed` instead of looping. Both the counter and the feedback live in orchestrator-owned files (`.agentloop/state/tasks/<id>/redesign.json`) so the manager's full rewrite of `backlog.json` cannot clobber them.

**Tech Stack:** Rust, ratatui 0.29 (TUI), crossterm 0.28, serde_json, tokio, anyhow.

**Root causes (verified during planning):**
- `state::increment_attempts` exists but is **never called** — business-task `attempts` stays `0`, so the manager's documented "orchestrator FAILS any item once attempts reach max_attempts" never fires. Redesigns are unbounded.
- `architect::architect_prompt` never reads the parent task's failure notes — the gate/customer feedback stored by `reopen_parent_for_redesign` never reaches the architect, so it regenerates an identical plan.
- `reopen_parent_for_redesign` → `invalidate_task_plan` deletes `builders.json` on every post-completion failure (orchestrator.rs:167-176), which is why "all builders done" tasks see their `builders.json` vanish and get re-architected over and over.

**Scope note:** This is one plan with two independent parts. Part A and Part B touch disjoint files and can be implemented/committed in either order.

---

## File Structure

- `src/tui.rs` — TUI rendering + view-model. Part A modifies only the `render` function's Jobs/Inbox list construction.
- `tests/tui_render_test.rs` — TUI render tests (TestBackend). Part A appends two scroll tests.
- `src/task_state.rs` — per-task JSON state helpers. Part B adds the `redesign.json` counter/feedback helpers + unit tests.
- `src/architect.rs` — architect prompt + validation. Part B makes `architect_prompt` include prior failure feedback + a unit test.
- `src/orchestrator.rs` — the iterate loop. Part B rewires `reopen_parent_for_redesign` to bump+cap the counter, resets it on customer approval, and adds a `tests` module.

---

# PART A — Scrollable Jobs / Inbox

The arrow keys already move `selected_job` / `selected` (see `on_key_list` in `src/tui.rs`), but the panes render with stateless `List` widgets, so the selection silently scrolls off-screen. Switching to a stateful list makes ratatui keep the selected row visible.

### Task A1: Jobs pane scrolls to follow selection

**Files:**
- Test: `tests/tui_render_test.rs` (append)
- Modify: `src/tui.rs` — `render` function, the `use` line (~line 433) and the Jobs list block (~lines 494-523)

- [ ] **Step 1: Write the failing test**

Append to `tests/tui_render_test.rs` (the file already defines `find(...)` and `started(...)` helpers and imports `Event`, `tui`, `AppState`, `TestBackend`, `Terminal`):

```rust
#[test]
fn jobs_pane_scrolls_to_keep_selection_visible() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = started("goal");
    for i in 0..40 {
        s.apply(Event::JobDispatched {
            id: format!("it-{i}"),
            label: format!("jobLABEL{i}"),
            tool: "codex".into(),
            model: "gpt".into(),
            log_path: None,
        });
    }
    // Focus Jobs, then move the selection to the last job.
    s.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    for _ in 0..39 {
        s.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let backend = TestBackend::new(80, 20);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "jobLABEL39").is_some(),
        "selected (last) job scrolled into view"
    );
    assert!(
        find(&term, "jobLABEL0 ").is_none(),
        "first job scrolled out of view"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test tui_render_test jobs_pane_scrolls_to_keep_selection_visible`
Expected: FAIL — `jobLABEL39` is not found (stateless list shows only the first rows; the selection never scrolls into view).

- [ ] **Step 3: Add `ListState` to the render imports**

In `src/tui.rs`, inside `pub fn render(...)`, change the widgets `use` line:

```rust
    use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
```

to:

```rust
    use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
```

- [ ] **Step 4: Make the Jobs list stateful/scrollable**

In `src/tui.rs`, replace the entire Jobs list block (currently lines ~494-523, starting at `let job_items: Vec<ListItem> = s` and ending at `f.render_widget(jobs_list, main_chunks[0]);`) with:

```rust
        let job_items: Vec<ListItem> = s
            .jobs
            .iter()
            .map(|j| {
                let glyph = status_glyph(&j.status);
                let dur = j.elapsed().map(fmt_elapsed).unwrap_or_default();
                let line = format!(" {} {} [{}/{}]  {}", glyph, j.label, j.tool, j.model, dur);
                ListItem::new(Line::from(line))
            })
            .collect();
        let jobs_border = if s.focus_is_jobs() {
            Color::Yellow
        } else {
            Color::Blue
        };
        let jobs_highlight = if s.focus_is_jobs() {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let jobs_list = List::new(job_items)
            .block(
                Block::default()
                    .title(" Jobs ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(jobs_border)),
            )
            .highlight_style(jobs_highlight);
        let mut jobs_state = ListState::default();
        if !s.jobs.is_empty() {
            jobs_state.select(Some(s.selected_job.min(s.jobs.len() - 1)));
        }
        f.render_stateful_widget(jobs_list, main_chunks[0], &mut jobs_state);
```

Note: `render` lives in `tui.rs`, so it can read the private fields `s.selected_job` and `s.selected` directly.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --test tui_render_test jobs_pane_scrolls_to_keep_selection_visible`
Expected: PASS

- [ ] **Step 6: Verify no existing TUI test regressed**

Run: `cargo test --test tui_render_test`
Expected: PASS (all tests, including `jobs_render_above_inbox_full_width`).

- [ ] **Step 7: Commit**

```bash
git add src/tui.rs tests/tui_render_test.rs
git commit -m "feat(tui): scroll Jobs pane to keep selection visible"
```

---

### Task A2: Inbox pane scrolls to follow selection

**Files:**
- Test: `tests/tui_render_test.rs` (append)
- Modify: `src/tui.rs` — the Inbox list block (~lines 525-555)

- [ ] **Step 1: Write the failing test**

Append to `tests/tui_render_test.rs`:

```rust
#[test]
fn inbox_pane_scrolls_to_keep_selection_visible() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = started("goal");
    for i in 0..40 {
        s.apply(Event::QuestionRaised {
            item_id: format!("q-{i}"),
            label: format!("inbLABEL{i}"),
            text: "pick one".into(),
            context: "".into(),
        });
    }
    // Focus defaults to Inbox; move the selection to the last question.
    for _ in 0..39 {
        s.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let backend = TestBackend::new(80, 20);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "inbLABEL39").is_some(),
        "selected (last) question scrolled into view"
    );
    assert!(
        find(&term, "inbLABEL0 ").is_none(),
        "first question scrolled out of view"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test tui_render_test inbox_pane_scrolls_to_keep_selection_visible`
Expected: FAIL — `inbLABEL39` is not found.

- [ ] **Step 3: Make the Inbox list stateful/scrollable**

In `src/tui.rs`, replace the entire Inbox list block (currently lines ~525-555, starting at `let inbox_items: Vec<ListItem> = s` and ending at `f.render_widget(inbox_list, main_chunks[1]);`) with:

```rust
        let inbox_items: Vec<ListItem> = s
            .inbox
            .iter()
            .map(|p| {
                ListItem::new(Line::from(format!(
                    " \u{2753} {} \u{2014} {}",
                    p.label, p.text
                )))
            })
            .collect();
        let inbox_border = if s.focus_is_jobs() {
            Color::Magenta
        } else {
            Color::Yellow
        };
        let inbox_highlight = if !s.focus_is_jobs() {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let inbox_list = List::new(inbox_items)
            .block(
                Block::default()
                    .title(" Inbox ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(inbox_border)),
            )
            .highlight_style(inbox_highlight);
        let mut inbox_state = ListState::default();
        if !s.inbox.is_empty() {
            inbox_state.select(Some(s.selected.min(s.inbox.len() - 1)));
        }
        f.render_stateful_widget(inbox_list, main_chunks[1], &mut inbox_state);
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test tui_render_test inbox_pane_scrolls_to_keep_selection_visible`
Expected: PASS

- [ ] **Step 5: Verify the whole TUI test suite still passes**

Run: `cargo test --test tui_render_test && cargo test --test tui_viewmodel_test`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/tui.rs tests/tui_render_test.rs
git commit -m "feat(tui): scroll Inbox pane to keep selection visible"
```

---

# PART B — Bounded, Feedback-Driven Redesign

Replace the unbounded delete-and-redesign behavior with (1) failure feedback fed to the architect and (2) a capped, orchestrator-owned redesign counter. State lives in `.agentloop/state/tasks/<id>/redesign.json`:

```json
{"count": 2, "feedback": "verify.sh failed; redesign required: <output>"}
```

### Task B1: `redesign.json` counter/feedback helpers in `task_state.rs`

**Files:**
- Modify: `src/task_state.rs` — add helpers near `customer_path` (~line 22) and a `tests` module at the end of the file.

- [ ] **Step 1: Write the failing test**

`src/task_state.rs` currently has no `tests` module. Append one at the very end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

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
    fn redesign_counter_bumps_persists_and_resets() {
        let ws = tmp_ws("redesign-helpers");
        // Missing file reads as (0, "").
        assert_eq!(read_redesign(&ws, "task-1"), (0, String::new()));

        // First bump -> count 1, feedback stored.
        assert_eq!(bump_redesign(&ws, "task-1", "gate failed").unwrap(), 1);
        assert_eq!(
            read_redesign(&ws, "task-1"),
            (1, "gate failed".to_string())
        );
        assert_eq!(redesign_feedback(&ws, "task-1"), "gate failed");

        // Second bump -> count 2, feedback replaced.
        assert_eq!(bump_redesign(&ws, "task-1", "customer rejected").unwrap(), 2);
        assert_eq!(
            read_redesign(&ws, "task-1"),
            (2, "customer rejected".to_string())
        );

        // Reset clears the file back to defaults.
        reset_redesign(&ws, "task-1");
        assert_eq!(read_redesign(&ws, "task-1"), (0, String::new()));

        let _ = std::fs::remove_dir_all(&ws);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib task_state::tests::redesign_counter_bumps_persists_and_resets`
Expected: FAIL — compile error: `read_redesign`, `bump_redesign`, `redesign_feedback`, `reset_redesign` not found.

- [ ] **Step 3: Implement the helpers**

In `src/task_state.rs`, add these functions immediately after `customer_path` (after line 22, before `read_builders`):

```rust
pub fn redesign_path(ws: &Path, task_id: &str) -> PathBuf {
    task_dir(ws, task_id).join("redesign.json")
}

/// Orchestrator-owned redesign bookkeeping: `(count, feedback)`. Missing or
/// unparseable file reads as `(0, "")`. Lives outside backlog.json so the manager's
/// full rewrite of the backlog cannot reset it.
pub fn read_redesign(ws: &Path, task_id: &str) -> (u32, String) {
    match std::fs::read_to_string(redesign_path(ws, task_id)) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(v) => (
                v.get("count").and_then(|c| c.as_u64()).unwrap_or(0) as u32,
                v.get("feedback")
                    .and_then(|f| f.as_str())
                    .unwrap_or("")
                    .to_string(),
            ),
            Err(_) => (0, String::new()),
        },
        Err(_) => (0, String::new()),
    }
}

/// The stored feedback from the most recent redesign trigger, or "" if none.
pub fn redesign_feedback(ws: &Path, task_id: &str) -> String {
    read_redesign(ws, task_id).1
}

/// Increment the redesign count and store the latest failure feedback. Returns the
/// new count.
pub fn bump_redesign(ws: &Path, task_id: &str, feedback: &str) -> Result<u32> {
    let (count, _) = read_redesign(ws, task_id);
    let next = count + 1;
    ensure_task_dir(ws, task_id)?;
    write_atomic(
        &redesign_path(ws, task_id),
        &json!({"count": next, "feedback": feedback}),
    )?;
    Ok(next)
}

/// Clear the redesign counter (called when the task is genuinely completed).
pub fn reset_redesign(ws: &Path, task_id: &str) {
    let _ = std::fs::remove_file(redesign_path(ws, task_id));
}
```

(`json!`, `Value`, `PathBuf`, `Path`, `ensure_task_dir`, `task_dir`, and `write_atomic` are all already imported/defined in `task_state.rs`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib task_state::tests::redesign_counter_bumps_persists_and_resets`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/task_state.rs
git commit -m "feat(task_state): add orchestrator-owned redesign counter + feedback"
```

---

### Task B2: Architect receives prior failure feedback

**Files:**
- Modify: `src/architect.rs` — `architect_prompt` (lines 10-46) and its `tests` module (append one test).

- [ ] **Step 1: Write the failing test**

Append a test to the existing `#[cfg(test)] mod tests` block in `src/architect.rs` (it already defines `tmp_ws` and imports `super::*`, `json`, `PathBuf`):

```rust
    #[test]
    fn architect_prompt_includes_redesign_feedback() {
        let ws = tmp_ws("archfeedback");
        let task = json!({"id":"task-1","title":"Login","desc":"Let users log in","acceptance":"user can log in"});

        // No feedback yet: prompt must not mention a prior attempt.
        let p0 = architect_prompt(&ws, &task);
        assert!(!p0.contains("PREVIOUS ATTEMPT"));

        // After a redesign is recorded, the feedback must appear in the prompt.
        crate::task_state::bump_redesign(&ws, "task-1", "verify.sh failed: missing logout route")
            .unwrap();
        let p1 = architect_prompt(&ws, &task);
        assert!(p1.contains("PREVIOUS ATTEMPT"));
        assert!(p1.contains("missing logout route"));

        let _ = std::fs::remove_dir_all(&ws);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib architect::tests::architect_prompt_includes_redesign_feedback`
Expected: FAIL — `p1` does not contain `"PREVIOUS ATTEMPT"` (the prompt ignores feedback today).

- [ ] **Step 3: Add the feedback block to `architect_prompt`**

In `src/architect.rs`, in `architect_prompt`, after the line:

```rust
    let task_dir = format!(".agentloop/state/tasks/{id}");
```

add:

```rust
    let feedback = crate::task_state::redesign_feedback(ws, id);
    let feedback_block = if feedback.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n\nA PREVIOUS ATTEMPT AT THIS TASK WAS REJECTED. Your earlier plan did not satisfy the gate or the customer. Produce a DIFFERENT plan that directly addresses this feedback:\n{feedback}\n"
        )
    };
```

Then insert `{feedback_block}` into the prompt's format string. Change this region:

```rust
BUSINESS TASK:
  id: {id}
  title: {title}
  task: {desc}
  acceptance criteria: {acc}

Your job:
```

to:

```rust
BUSINESS TASK:
  id: {id}
  title: {title}
  task: {desc}
  acceptance criteria: {acc}{feedback_block}

Your job:
```

And add `feedback_block = feedback_block,` to the `format!` argument list (place it right after `task_dir = task_dir`):

```rust
        ws = ws.display(),
        id = id,
        title = title,
        desc = desc,
        acc = acc,
        task_dir = task_dir,
        feedback_block = feedback_block
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib architect::tests::architect_prompt_includes_redesign_feedback`
Expected: PASS

- [ ] **Step 5: Verify the rest of the architect tests still pass**

Run: `cargo test --lib architect`
Expected: PASS (existing validation tests unaffected).

- [ ] **Step 6: Commit**

```bash
git add src/architect.rs
git commit -m "feat(architect): feed prior redesign failure into the architect prompt"
```

---

### Task B3: Cap redesigns in the orchestrator + reset on approval

**Files:**
- Modify: `src/orchestrator.rs` — `reopen_parent_for_redesign` (lines 172-176), its 5 call sites, the customer-approved branch (~line 537), and add a `tests` module.

- [ ] **Step 1: Write the failing test**

`src/orchestrator.rs` has no `tests` module. Append one at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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

    /// A workspace with one in_progress business task whose single builder is done.
    fn setup(ws: &Path) {
        let sdir = ws.join(".agentloop/state");
        std::fs::create_dir_all(&sdir).unwrap();
        let backlog = json!({"items":[{
            "id":"task-1","title":"t","desc":"d","deps":[],
            "status":"in_progress","attempts":0,"acceptance":"a"
        }]});
        std::fs::write(sdir.join("backlog.json"), serde_json::to_vec(&backlog).unwrap()).unwrap();
        let tdir = sdir.join("tasks/task-1");
        std::fs::create_dir_all(&tdir).unwrap();
        std::fs::write(tdir.join("design.md"), "design").unwrap();
        std::fs::write(
            tdir.join("builders.json"),
            r#"{"items":[{"id":"task-1-b1","title":"t","desc":"d","deps":[],"status":"done","attempts":1,"acceptance":"a"}]}"#,
        )
        .unwrap();
    }

    #[test]
    fn redesign_under_cap_reopens_invalidates_and_records_feedback() {
        let ws = tmp_ws("orch-under");
        setup(&ws);
        let bk = ws.join(".agentloop/state/backlog.json");

        reopen_parent_for_redesign(&bk, &ws, "task-1", "gate failed", 3).unwrap();

        let v = state::read(&bk).unwrap();
        assert_eq!(state::item(&v, "task-1").unwrap()["status"], "ready");
        assert!(
            !task_state::builders_path(&ws, "task-1").exists(),
            "plan invalidated under the cap"
        );
        let (count, fb) = task_state::read_redesign(&ws, "task-1");
        assert_eq!(count, 1);
        assert_eq!(fb, "gate failed");

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn redesign_at_cap_fails_task_and_keeps_plan() {
        let ws = tmp_ws("orch-cap");
        setup(&ws);
        let bk = ws.join(".agentloop/state/backlog.json");

        // Two prior redesigns already recorded; cap is 3.
        task_state::bump_redesign(&ws, "task-1", "x").unwrap();
        task_state::bump_redesign(&ws, "task-1", "x").unwrap();

        // This call makes count = 3 == cap -> the task fails instead of looping.
        reopen_parent_for_redesign(&bk, &ws, "task-1", "still failing", 3).unwrap();

        let v = state::read(&bk).unwrap();
        assert_eq!(state::item(&v, "task-1").unwrap()["status"], "failed");
        assert!(
            task_state::builders_path(&ws, "task-1").exists(),
            "plan is kept for inspection when the cap is hit"
        );

        let _ = std::fs::remove_dir_all(&ws);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib orchestrator::tests`
Expected: FAIL — compile error: `reopen_parent_for_redesign` takes 4 args, not 5 (the cap parameter does not exist yet).

- [ ] **Step 3: Rewrite `reopen_parent_for_redesign` to bump + cap**

In `src/orchestrator.rs`, replace the current function (lines 172-176):

```rust
fn reopen_parent_for_redesign(bk: &Path, ws: &Path, task_id: &str, note: &str) -> Result<()> {
    state::set_status(bk, task_id, "ready", note)?;
    invalidate_task_plan(ws, task_id);
    Ok(())
}
```

with:

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
        state::set_status(
            bk,
            task_id,
            "failed",
            &format!("redesign cap ({max_redesigns}) reached; last failure: {note}"),
        )?;
    } else {
        // Under the cap: reopen for a fresh, feedback-informed architect pass.
        state::set_status(bk, task_id, "ready", note)?;
        invalidate_task_plan(ws, task_id);
    }
    Ok(())
}
```

- [ ] **Step 4: Pass the cap at all 5 call sites**

In `src/orchestrator.rs`, all five calls live inside `iterate`, which already defines `let maxatt = cfg.max_attempts();`. Add `, maxatt` to each:

- ~line 344 (builder exceeded max_attempts, not-yet-dispatched branch):
  ```rust
                    reopen_parent_for_redesign(&bk, ws, task_id, &note, maxatt)?;
  ```
- ~line 370 (worktree create failed, not-yet-dispatched branch):
  ```rust
                    reopen_parent_for_redesign(&bk, ws, task_id, &note, maxatt)?;
  ```
- ~line 512 (pending_redesign drain loop):
  ```rust
        reopen_parent_for_redesign(&bk, ws, &task_id, &note, maxatt)?;
  ```
- ~line 523 (gate failed after all builders done):
  ```rust
            reopen_parent_for_redesign(&bk, ws, &task_id, &feedback, maxatt)?;
  ```
- ~line 541 (customer rejected):
  ```rust
            reopen_parent_for_redesign(&bk, ws, &task_id, &feedback, maxatt)?;
  ```

- [ ] **Step 5: Reset the counter on customer approval**

In `src/orchestrator.rs`, the customer-approved branch currently reads:

```rust
        if customer::customer_run(cfg, ws, &task, &log, itimeout).await? {
            state::set_status(&bk, &task_id, "done", "")?;
            reporter.status(&cid, "approved", &ctool, &cmodel);
        } else {
```

Change the approved arm to also clear the redesign counter (genuine completion):

```rust
        if customer::customer_run(cfg, ws, &task, &log, itimeout).await? {
            state::set_status(&bk, &task_id, "done", "")?;
            task_state::reset_redesign(ws, &task_id);
            reporter.status(&cid, "approved", &ctool, &cmodel);
        } else {
```

- [ ] **Step 6: Run the orchestrator tests to verify they pass**

Run: `cargo test --lib orchestrator::tests`
Expected: PASS (both `redesign_under_cap_...` and `redesign_at_cap_...`).

- [ ] **Step 7: Verify the whole crate builds and all tests pass**

Run: `cargo test`
Expected: PASS — full suite green.

- [ ] **Step 8: Confirm clippy is clean (house style: warnings denied)**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no errors.

- [ ] **Step 9: Commit**

```bash
git add src/orchestrator.rs
git commit -m "fix(orchestrator): cap post-completion redesigns instead of looping forever"
```

---

## Self-Review

**Spec coverage:**
- "Make the jobs/inbox scrollable, up/down moves up/down" → Part A, Tasks A1 (Jobs) + A2 (Inbox). Arrow-key movement already existed (`on_key_list`); the gap was rendering, now fixed with stateful lists.
- "Fix the manager ↔ architect loop: builders.json all done but deleted, architect re-creates forever" → Part B. The deletion is intentional (`invalidate_task_plan`); the loop was unbounded because (a) feedback never reached the architect (Task B2) and (b) nothing counted/capped redesigns (Tasks B1 + B3). The user chose to keep JSON (not migrate to SQLite); a separate SQLite migration can be planned later if file-rewrite fragility persists.

**Placeholder scan:** No TBD/TODO/"add error handling" placeholders; every code step contains complete code and exact commands with expected output.

**Type consistency:** Helper names are consistent across tasks — `read_redesign`, `bump_redesign(ws, task_id, feedback) -> Result<u32>`, `redesign_feedback`, `reset_redesign`, `redesign_path` (B1), consumed unchanged in B2 (`redesign_feedback`) and B3 (`bump_redesign`, `reset_redesign`, `read_redesign`, `builders_path`). `reopen_parent_for_redesign` gains a single `max_redesigns: u32` parameter, updated at all 5 call sites with the in-scope `maxatt`. Render uses the existing private fields `selected_job` / `selected` and the new `ListState` import.

**Why the cap is robust against the manager:** `redesign.json` is written/read only by the orchestrator under `.agentloop/state/tasks/<id>/`, never by the manager (which only rewrites `backlog.json`). `invalidate_task_plan` deletes `builders.json` + `customer.json` but not `redesign.json`, so the count accumulates across redesign cycles and only `reset_redesign` (on customer approval) clears it.
