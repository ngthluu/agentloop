# TUI Goal-Entry, Persistent Input & Enhanced Planner — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an interactive goal-entry screen, replace the two modal TUI inputs with one persistent wrapping bottom input, and remove the hollow `architect`/`fix`/`trivial` roles by folding technical design into the planner.

**Architecture:** Three loosely-coupled changes to the existing Rust crate. (1) A new `Command::StartRun` and an `AppState` `View::GoalEntry` gate the orchestrator until the user commits a goal. (2) `AppState` drops its `Mode` enum for an always-live `input` whose submission target is derived from pane focus/selection. (3) `templates/config.yaml` keeps only `planner`/`build`/`resolver`; the planner prompt now maintains `design.md` and emits a dependency-aware graph of `build` items, and the worker prompt reads `design.md`.

**Tech Stack:** Rust 2021, tokio, ratatui + crossterm (TUI), serde_json, anyhow. Offline tests via the in-crate `fake_agent` + scripted stub.

---

## File Structure

- `src/events.rs` — add `Command::StartRun { goal }`.
- `src/orchestrator.rs` — pre-start gate in `run_interactive`; handle/ignore `StartRun` in existing command match arms.
- `src/cli.rs` — add `commit_goal` helper (writes/folds the goal at start time).
- `src/planner.rs` — enhanced planner prompt (maintain `design.md`, `role="build"`, dependency-aware graph).
- `src/worker.rs` — `worker_prompt` injects `design.md` content.
- `src/tui.rs` — `View::GoalEntry`; remove `Mode`; persistent wrapping input; new key routing; render goal screen + bottom input bar.
- `src/app.rs` — unchanged logic, but verify `StartRun` flows through (no code change expected; confirmation step only).
- `templates/config.yaml` — remove `architect`/`fix`/`trivial`.
- `README.md` — document the entry screen, persistent input + key model, trimmed roles.
- `tests/planner_worker_test.rs`, `tests/tui_viewmodel_test.rs`, `tests/tui_render_test.rs`, `tests/cli_goal_test.rs` — new + updated tests.

**Key reconciliation (Enter in the list view):** Section 2 of the spec says "Enter submits the input," but the existing TUI also opens a job's detail with Enter on the Jobs pane. The plan resolves this explicitly: **Enter submits when the input is non-empty; when the input is empty it performs the focused pane's navigation action (open job detail on Jobs; no-op on Inbox).** This preserves job-detail access without a new key.

---

## Task 1: Trim roles in the default config

**Files:**
- Modify: `templates/config.yaml:9-17`

- [ ] **Step 1: Remove architect/fix/trivial from routing**

Replace the `routing:` and `defaults:` block so only `planner`, `build`, `resolver` remain:

```yaml
routing:                      # role -> how to spawn
  planner:  { tool: claude, model: opus,   effort: high,   flags: "--dangerously-skip-permissions" }
  build:    { tool: codex,  model: gpt-5.5, effort: high,   flags: "--dangerously-bypass-approvals-and-sandbox" }
  resolver: { tool: claude, model: sonnet, effort: medium, flags: "--dangerously-skip-permissions" }

defaults: { role: build }
```

- [ ] **Step 2: Verify the crate still builds**

Run: `cargo build`
Expected: builds with no errors (the template is `include_str!`'d; no code references the removed roles).

- [ ] **Step 3: Commit**

```bash
git add templates/config.yaml
git commit -m "config: drop architect/fix/trivial roles (planner/build/resolver only)"
```

---

## Task 2: Enhance the planner prompt (design.md + dependency-aware build graph)

**Files:**
- Modify: `src/planner.rs:8-41`
- Test: `tests/planner_worker_test.rs`

- [ ] **Step 1: Write the failing test**

Add to `tests/planner_worker_test.rs`:

```rust
#[test]
fn planner_prompt_maintains_design_and_build_graph() {
    let ws = ws_with_state();
    let p = planner::planner_prompt(&ws, 3);
    // Planner owns the technical design now.
    assert!(p.contains("design.md"), "planner is told to maintain design.md");
    // Work items are all role=build; no architect/fix/trivial.
    assert!(p.contains(r#"role="build""#), "items are tagged build");
    assert!(!p.contains("architect"), "architect role removed");
    // Dependency-aware decomposition is requested.
    assert!(p.contains("dependency-aware"), "asks for a dependency-aware task graph");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test planner_worker_test planner_prompt_maintains_design_and_build_graph`
Expected: FAIL (current prompt mentions `architect` and lacks `design.md`).

- [ ] **Step 3: Rewrite the planner prompt**

Replace the `format!` body in `planner_prompt` (`src/planner.rs:14-40`) with:

```rust
    format!(r#"You are the PLANNER for an autonomous app build. Working dir: {ws} (a git repo).
You own BOTH the technical design and the backlog.

GOAL:
{goal}

CURRENT master.md:
{master}

CURRENT backlog.json:
{backlog}

Your job each round:
1. Read worker results in .agentloop/results/ and the latest gate output in
   .agentloop/state/last_gate.txt (if present). Mark finished items status="done".
2. Maintain .agentloop/state/design.md — the technical solution for the GOAL: chosen
   stack, architecture/structure, and key decisions/constraints. Author it on the first
   round and keep it current as the design evolves. Build workers implement against it.
3. Add/split/refine items so the GOAL gets built per design.md. First round: scaffold the
   project and write an executable .agentloop/verify.sh that builds/tests the app (start simple).
4. The orchestrator FAILS any item once its attempts reach {max_attempts} (the max_attempts cap).
   So for any item nearing attempts={max_attempts}, redesign it (smaller/different) or drop it
   instead of re-queueing the same work.
5. Decompose the work into a dependency-aware task graph: give every open work item
   role="build", realistic deps (ids of items that must finish first), and a concrete
   acceptance string, so build workers run in the correct order.

OUTPUT CONTRACT — you MUST overwrite .agentloop/state/backlog.json with valid JSON:
{{"items":[{{"id","title","desc","role":"build","deps":[],"status":"ready|in_progress|done|failed|blocked","attempts":0,"acceptance"}}]}}
Also write .agentloop/state/design.md (the technical design) and rewrite
.agentloop/state/master.md as a human-readable status board.
Do not print the JSON to stdout; write the files.{requests}"#,
        ws = ws.display(), goal = goal, master = master, backlog = backlog, max_attempts = max_attempts, requests = requests)
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test planner_worker_test`
Expected: PASS (including the existing `planner_prompt_has_contract` and `planner_prompt_includes_pending_requests`).

- [ ] **Step 5: Commit**

```bash
git add src/planner.rs tests/planner_worker_test.rs
git commit -m "planner: maintain design.md and emit a dependency-aware build graph"
```

---

## Task 3: Worker prompt reads design.md

**Files:**
- Modify: `src/worker.rs:9-34`
- Test: `tests/planner_worker_test.rs`

- [ ] **Step 1: Write the failing test**

Add to `tests/planner_worker_test.rs`:

```rust
#[test]
fn worker_prompt_injects_design_when_present() {
    let ws = std::env::temp_dir().join(format!("alwd-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
    std::fs::write(ws.join(".agentloop/state/design.md"), "Use SQLite for storage.").unwrap();

    let item = serde_json::json!({"id":"it-9","title":"T","desc":"D","role":"build","acceptance":"A"});
    let p = agentloop::worker::worker_prompt(&ws, &item);
    assert!(p.contains("TECHNICAL DESIGN"), "design block header present");
    assert!(p.contains("Use SQLite for storage."), "design content injected");
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test planner_worker_test worker_prompt_injects_design_when_present`
Expected: FAIL (prompt does not yet read `design.md`).

- [ ] **Step 3: Inject the design block**

In `src/worker.rs`, inside `worker_prompt`, after the `let prior = ...` line (`src/worker.rs:13`), add:

```rust
    let design = std::fs::read_to_string(ws.join(".agentloop/state/design.md")).unwrap_or_default();
    let design_block = if design.trim().is_empty() {
        String::new()
    } else {
        format!("\n\nTECHNICAL DESIGN (.agentloop/state/design.md) — implement consistently with this:\n{design}")
    };
```

Then append `{design_block}` to the prompt: change the format string's tail from `then stop. ...{prior}"#,` so the trailing interpolations read `{prior}{design_block}"#,` and add `design_block = design_block,` to the `format!` args. Concretely, the closing of the `format!` becomes:

```rust
  then stop. The user will answer and you will be re-dispatched with their answer.{prior}{design_block}"#,
        id = id, title = title, desc = desc, acc = acc, ws = ws.display(), prior = prior, design_block = design_block)
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test planner_worker_test`
Expected: PASS (new test + existing `worker_prompt_*` tests, which pass an item with no `design.md` so `design_block` is empty).

- [ ] **Step 5: Commit**

```bash
git add src/worker.rs tests/planner_worker_test.rs
git commit -m "worker: implement against design.md when present"
```

---

## Task 4: Add `Command::StartRun`

**Files:**
- Modify: `src/events.rs:75-80`

- [ ] **Step 1: Add the variant**

In `src/events.rs`, extend the `Command` enum:

```rust
/// UI -> orchestrator.
#[derive(Debug, Clone)]
pub enum Command {
    StartRun { goal: String },
    AnswerQuestion { item_id: String, text: String },
    AddTask { request: String },
    Quit,
}
```

- [ ] **Step 2: Verify it compiles (expect match-arm errors next task)**

Run: `cargo build`
Expected: build FAILS in `src/orchestrator.rs` with non-exhaustive `match` errors on `Command`. That is expected and fixed in Task 5. (If you prefer a clean build between tasks, do Task 5 immediately.)

- [ ] **Step 3: Commit**

```bash
git add src/events.rs
git commit -m "events: add Command::StartRun"
```

---

## Task 5: Orchestrator pre-start gate

**Files:**
- Modify: `src/orchestrator.rs:340-417`

- [ ] **Step 1: Add the pre-start gate at the top of `run_interactive`**

In `run_interactive`, immediately after the `let mut iters_this_window = 0u32;` line and before `'outer: loop {` (`src/orchestrator.rs:355`), insert:

```rust
    // Wait for the TUI to commit a goal (the goal-entry screen) before doing any work.
    // Nothing — no planner, no workers — runs until StartRun arrives.
    loop {
        match crx.recv().await {
            None | Some(Command::Quit) => return Ok(0),
            Some(Command::StartRun { goal }) => {
                let _ = crate::cli::commit_goal(ws, &goal);
                break;
            }
            // Stray answer/add-task before the run starts: ignore.
            Some(Command::AnswerQuestion { .. }) | Some(Command::AddTask { .. }) => {}
        }
    }
```

- [ ] **Step 2: Add `StartRun` arms to the three in-loop command matches**

`run_interactive` matches `Command` in three more places. Add an ignore arm to each (a `StartRun` after the run has started is a no-op):

In the drain loop (`src/orchestrator.rs:360-366`), add inside `match cmd {`:

```rust
                    Command::StartRun { .. } => {}
```

In the user-blocked `crx.recv().await` match (`src/orchestrator.rs:385-389`), add:

```rust
                    Some(Command::StartRun { .. }) => {}
```

In the standby `crx.recv().await` match (`src/orchestrator.rs:405-409`), add:

```rust
                Some(Command::StartRun { .. }) => {}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: builds (after Task 6 adds `commit_goal`, this fully resolves). If `commit_goal` does not exist yet, build fails on that call — do Task 6 next. To keep builds green, implement Task 6 before re-running.

- [ ] **Step 4: Commit**

```bash
git add src/orchestrator.rs
git commit -m "orchestrator: gate run_interactive on Command::StartRun"
```

---

## Task 6: `commit_goal` helper in cli.rs

**Files:**
- Modify: `src/cli.rs` (add `commit_goal` near `fold_rerun_goal`, after `src/cli.rs:115`)
- Test: `tests/cli_goal_test.rs`

- [ ] **Step 1: Write the failing test**

Add to `tests/cli_goal_test.rs`:

```rust
use agentloop::cli::commit_goal;

#[test]
fn commit_goal_writes_when_fresh_and_folds_when_existing() {
    let ws = std::env::temp_dir().join(format!(
        "alcommit-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
    std::fs::write(ws.join(".agentloop/state/goal.md"), "").unwrap();

    // Fresh (empty goal.md): commit writes the goal directly.
    commit_goal(&ws, "build a todo app").unwrap();
    let g = std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap();
    assert!(g.contains("build a todo app"));

    // Existing goal + new text: folded as an addition (appended).
    commit_goal(&ws, "also add a --due flag").unwrap();
    let g2 = std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap();
    assert!(g2.contains("build a todo app"));
    assert!(g2.contains("also add a --due flag"));

    // Re-committing identical existing text is a no-op (no duplicate).
    commit_goal(&ws, "build a todo app").unwrap();
    let g3 = std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap();
    assert_eq!(g3.matches("build a todo app").count(), 1);

    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test cli_goal_test commit_goal_writes_when_fresh_and_folds_when_existing`
Expected: FAIL with "cannot find function `commit_goal`".

- [ ] **Step 3: Implement `commit_goal`**

Add to `src/cli.rs` after `fold_rerun_goal` (after `src/cli.rs:115`):

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test cli_goal_test`
Expected: PASS (new test + existing `goal_text_prefers_arg_then_goal_md_then_empty`).

- [ ] **Step 5: Verify the whole crate builds**

Run: `cargo build`
Expected: builds clean (Tasks 4–6 now resolve together).

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs tests/cli_goal_test.rs
git commit -m "cli: add commit_goal for entry-screen goal commit"
```

---

## Task 7: TUI — `View::GoalEntry` state + key handling

**Files:**
- Modify: `src/tui.rs` (View enum `:44-48`, AppState `:50-87`, `on_key` `:141-273`, accessors `:275-302`)
- Test: `tests/tui_viewmodel_test.rs`

This task replaces the `Mode` enum and modal `on_key` with the goal-entry + persistent-input model. It is the largest change; do all of it before re-running the TUI tests (Task 8 fixes the now-stale existing tests).

- [ ] **Step 1: Replace the `Mode`/`View` types**

In `src/tui.rs`, delete the `Mode` enum (`:31-36`) and replace the `View` enum (`:44-48`) with:

```rust
#[derive(PartialEq, Clone, Copy)]
enum View {
    GoalEntry,
    List,
    JobDetail,
}
```

- [ ] **Step 2: Update `AppState` fields and `new`**

In the `AppState` struct (`:50-66`), remove the `mode: Mode,` field and add `goal_focus_continue: bool,`. Update `AppState::new` (`:68-87`) so the input is pre-filled with the goal and the view starts on the entry screen:

```rust
impl AppState {
    pub fn new(goal: String) -> Self {
        Self {
            goal: goal.clone(),
            jobs: vec![],
            inbox: vec![],
            selected: 0,
            iter: 0,
            gate: "init".into(),
            open: 0,
            standby: false,
            input: goal,
            focus: Focus::Inbox,
            view: View::GoalEntry,
            goal_focus_continue: false,
            selected_job: 0,
            log_scroll: 0,
            started: std::time::Instant::now(),
        }
    }
```

(Leave the `apply` method unchanged.)

- [ ] **Step 3: Replace `on_key` with view-dispatched handlers**

Replace the entire `on_key` method (`:140-273`) with:

```rust
    /// Map a key to an optional Command. Returns None when the key only changes UI state.
    pub fn on_key(&mut self, k: KeyEvent) -> Option<Command> {
        match self.view {
            View::GoalEntry => self.on_key_goal_entry(k),
            View::JobDetail => self.on_key_job_detail(k),
            View::List => self.on_key_list(k),
        }
    }

    fn is_newline(k: &KeyEvent) -> bool {
        k.code == KeyCode::Enter
            && k.modifiers.intersects(crossterm::event::KeyModifiers::SHIFT | crossterm::event::KeyModifiers::ALT)
    }

    fn on_key_goal_entry(&mut self, k: KeyEvent) -> Option<Command> {
        if Self::is_newline(&k) {
            self.input.push('\n');
            return None;
        }
        match k.code {
            KeyCode::Enter => {
                let goal = self.input.trim().to_string();
                if goal.is_empty() {
                    return None; // nothing to start yet; stay on the entry screen
                }
                self.goal = goal.clone();
                self.view = View::List;
                Some(Command::StartRun { goal })
            }
            KeyCode::Tab => {
                self.goal_focus_continue = !self.goal_focus_continue;
                None
            }
            KeyCode::Backspace => {
                self.input.pop();
                None
            }
            KeyCode::Esc => {
                self.input.clear();
                None
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                None
            }
            _ => None,
        }
    }

    fn on_key_job_detail(&mut self, k: KeyEvent) -> Option<Command> {
        if Self::is_newline(&k) {
            self.input.push('\n');
            return None;
        }
        match k.code {
            KeyCode::Esc => {
                self.view = View::List;
                self.log_scroll = 0;
                None
            }
            KeyCode::Up => {
                self.log_scroll = self.log_scroll.saturating_add(1);
                None
            }
            KeyCode::Down => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
                None
            }
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.input.pop();
                None
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                None
            }
            _ => None,
        }
    }

    fn on_key_list(&mut self, k: KeyEvent) -> Option<Command> {
        if Self::is_newline(&k) {
            self.input.push('\n');
            return None;
        }
        match k.code {
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
                // Non-empty input submits; empty input runs the focused pane's action.
                if self.input.trim().is_empty() {
                    if self.focus == Focus::Jobs && self.selected_job < self.jobs.len() {
                        self.view = View::JobDetail;
                        self.log_scroll = 0;
                    }
                    None
                } else {
                    self.submit()
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                None
            }
            KeyCode::Esc => {
                self.input.clear();
                None
            }
            KeyCode::Char('q') if self.input.is_empty() => Some(Command::Quit),
            KeyCode::Char(c) => {
                self.input.push(c);
                None
            }
            _ => None,
        }
    }

    /// Submit the current input, routing by focus/selection. Clears the input.
    fn submit(&mut self) -> Option<Command> {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return None;
        }
        if self.focus == Focus::Inbox && !self.inbox.is_empty() {
            let idx = self.selected.min(self.inbox.len().saturating_sub(1));
            let p = self.inbox.remove(idx);
            self.selected = 0;
            self.input.clear();
            Some(Command::AnswerQuestion { item_id: p.item_id, text })
        } else {
            self.input.clear();
            Some(Command::AddTask { request: text })
        }
    }
```

- [ ] **Step 4: Replace the stale accessors**

Replace the `is_editing` / `mode_is_adding` / `mode_is_answering` accessors (`:279-289`) with the new view/target accessors (keep `input_buffer`, `focus_is_jobs`, `in_job_detail`, `total_elapsed`):

```rust
    pub fn in_goal_entry(&self) -> bool {
        self.view == View::GoalEntry
    }

    pub fn goal_continue_focused(&self) -> bool {
        self.goal_focus_continue
    }

    /// Label shown above the input: what a submission will do right now.
    pub fn input_target_label(&self) -> String {
        if self.focus == Focus::Inbox && !self.inbox.is_empty() {
            let idx = self.selected.min(self.inbox.len().saturating_sub(1));
            format!("Answering {}", self.inbox[idx].item_id)
        } else {
            "Add task".to_string()
        }
    }
```

- [ ] **Step 5: Write the new viewmodel tests**

Add to `tests/tui_viewmodel_test.rs`:

```rust
#[test]
fn goal_entry_commit_emits_start_run() {
    let mut s = AppState::new(String::new());
    assert!(s.in_goal_entry());
    // Empty input: Enter is a no-op (still on the entry screen).
    assert!(s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());
    assert!(s.in_goal_entry());
    for c in "build app".chars() { s.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)); }
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::StartRun { ref goal }) if goal == "build app"));
    assert!(!s.in_goal_entry());
}

#[test]
fn goal_entry_prefill_commits_existing_goal() {
    let mut s = AppState::new("resume this goal".into());
    // Pre-filled with the existing goal; Enter commits it unchanged.
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::StartRun { ref goal }) if goal == "resume this goal"));
}

#[test]
fn shift_enter_inserts_newline_in_goal_entry() {
    let mut s = AppState::new(String::new());
    s.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    s.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    assert_eq!(s.input_buffer(), "a\nb");
    assert!(s.in_goal_entry(), "shift+enter does not commit");
}
```

- [ ] **Step 6: Run the new tests (existing ones still fail until Task 8)**

Run: `cargo test --test tui_viewmodel_test goal_entry_commit_emits_start_run goal_entry_prefill_commits_existing_goal shift_enter_inserts_newline_in_goal_entry`
Expected: these three PASS. (The pre-existing tests in this file are expected to fail to compile/pass until Task 8 rewrites them — that is why they are scoped out by name here.)

- [ ] **Step 7: Commit**

```bash
git add src/tui.rs tests/tui_viewmodel_test.rs
git commit -m "tui: goal-entry view + persistent-input key model (state/keys)"
```

---

## Task 8: TUI — rewrite stale viewmodel tests + add persistent-input tests

**Files:**
- Modify: `tests/tui_viewmodel_test.rs` (the four modal-era tests)

- [ ] **Step 1: Replace the four modal-era tests**

In `tests/tui_viewmodel_test.rs`, delete `key_input_maps_to_commands`, `add_task_key_path_emits_command`, `enter_on_inbox_focus_still_answers`, and `tab_toggles_focus_and_enter_opens_job_detail`, and replace them with versions for the new model. (Add this `start` helper near the top of the file too.)

```rust
/// Drive an AppState past the goal-entry screen into the List view.
fn start(goal: &str) -> AppState {
    let mut s = AppState::new(goal.into());
    let _ = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)); // commit goal
    s
}

#[test]
fn typing_then_enter_answers_selected_question() {
    let mut s = start("g");
    s.apply(Event::QuestionRaised { item_id: "db".into(), label: "db".into(), text: "q?".into(), context: "".into() });
    // Focus defaults to Inbox with the question selected: typing goes straight to input.
    for c in "yes".chars() { s.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)); }
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::AnswerQuestion { ref item_id, ref text }) if item_id == "db" && text == "yes"));
    // Input is cleared after submit, so 'q' now quits.
    assert!(matches!(s.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)), Some(Command::Quit)));
}

#[test]
fn typing_then_enter_adds_task_when_no_question() {
    let mut s = start("g");
    // No questions: target is Add task.
    for c in "due flag".chars() { s.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)); }
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::AddTask { ref request }) if request == "due flag"));
}

#[test]
fn q_quits_only_when_input_empty() {
    let mut s = start("g");
    // Empty input: 'q' quits.
    assert!(matches!(s.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)), Some(Command::Quit)));
    // With text, 'q' types a literal q.
    let mut s2 = start("g");
    s2.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(s2.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)).is_none());
    assert_eq!(s2.input_buffer(), "xq");
}

#[test]
fn tab_switches_focus_and_empty_enter_opens_job_detail() {
    use std::path::PathBuf;
    let mut s = start("g");
    s.apply(Event::JobDispatched {
        id: "it-1".into(), label: "scaffold".into(), tool: "codex".into(),
        model: "gpt-5".into(), log_path: Some(PathBuf::from("/tmp/x.log")),
    });
    // Default focus is Inbox; Tab moves it to Jobs.
    assert!(!s.focus_is_jobs());
    assert!(s.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).is_none());
    assert!(s.focus_is_jobs());
    // Empty input + Enter on Jobs opens the detail view.
    assert!(s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());
    assert!(s.in_job_detail());
    // Esc returns to the list.
    assert!(s.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).is_none());
    assert!(!s.in_job_detail());
}

#[test]
fn target_label_tracks_focus_and_inbox() {
    let mut s = start("g");
    assert_eq!(s.input_target_label(), "Add task");
    s.apply(Event::QuestionRaised { item_id: "db".into(), label: "db".into(), text: "q?".into(), context: "".into() });
    // Inbox focused (default) with a question -> answering.
    assert_eq!(s.input_target_label(), "Answering db");
    s.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)); // focus Jobs
    assert_eq!(s.input_target_label(), "Add task");
}
```

- [ ] **Step 2: Run the full viewmodel suite**

Run: `cargo test --test tui_viewmodel_test`
Expected: PASS (all tests, old replacements + the Task 7 entry tests + the ones that didn't change: `applies_events_to_view_model`, `standby_event_sets_flag`, `dispatch_starts_timer_and_stores_log_path_then_freezes`).

- [ ] **Step 3: Commit**

```bash
git add tests/tui_viewmodel_test.rs
git commit -m "tui: rewrite viewmodel tests for goal-entry + persistent input"
```

---

## Task 9: TUI — render the goal-entry screen and persistent input bar

**Files:**
- Modify: `src/tui.rs` `render` (`:364-510`)
- Test: `tests/tui_render_test.rs`

- [ ] **Step 1: Update the render-test helpers to start in List view**

In `tests/tui_render_test.rs`, the two existing tests render a fresh `AppState`, which now starts on the goal-entry screen. Drive them past it first. Replace the two tests with:

```rust
fn started(goal: &str) -> AppState {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = AppState::new(goal.into());
    let _ = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)); // leave goal-entry
    s
}

#[test]
fn jobs_render_above_inbox_full_width() {
    let mut s = started("goal");
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
    assert!(jobs.0 < inbox.0, "Jobs ({jobs:?}) is above Inbox ({inbox:?})");
}

#[test]
fn status_bar_shows_total_time() {
    let s = started("goal");
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(find(&term, "\u{23f1}").is_some(), "status bar shows the ⏱ total-time glyph");
}
```

- [ ] **Step 2: Add a goal-entry render test**

Add to `tests/tui_render_test.rs`:

```rust
#[test]
fn goal_entry_screen_shows_prompt_and_continue() {
    let s = AppState::new(String::new()); // starts on the goal-entry view
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(find(&term, "Continue").is_some(), "Continue button rendered");
    assert!(find(&term, "build").is_some(), "entry prompt mentions what to build");
}

#[test]
fn list_view_shows_input_target_label() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = AppState::new("g".into());
    let _ = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(find(&term, "Add task").is_some(), "bottom input shows the Add task target");
}
```

- [ ] **Step 3: Run to verify the new render tests fail**

Run: `cargo test --test tui_render_test goal_entry_screen_shows_prompt_and_continue list_view_shows_input_target_label`
Expected: FAIL (render does not yet branch on `GoalEntry` or show the target label; `render` still references removed `is_editing`/`mode_*` and will not compile).

- [ ] **Step 4: Rewrite `render` to branch on the goal-entry view and draw the persistent input bar**

Replace the body of `render` (`:364-510`) with the following. The goal-entry view takes the full area; otherwise render status bar + main + a persistent input footer (replacing the old editing/standby/normal footer branches).

```rust
pub fn render(f: &mut ratatui::Frame, s: &AppState) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

    let area = f.area();

    if s.in_goal_entry() {
        render_goal_entry(f, s, area);
        return;
    }

    // Bottom input bar height grows with the number of input lines (capped).
    let input_lines = s.input_buffer().split('\n').count().max(1) as u16;
    let footer_height = (input_lines + 3).min(12); // label + input lines + hint + border
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .split(area);

    // --- Top status bar ---
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
    let status_bar = Paragraph::new(status_text)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD));
    f.render_widget(status_bar, chunks[0]);

    // --- Main area: jobs (top) + inbox (bottom), or the job-detail view ---
    if s.in_job_detail() {
        render_job_detail(f, s, chunks[1]);
    } else {
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(chunks[1]);

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
                ListItem::new(Line::from(format!(" \u{2753} {} \u{2014} {}", p.label, p.text))).style(style)
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
    }

    // --- Persistent bottom input bar ---
    let footer = chunks[2];
    let fchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(footer);

    let title = format!(" {} ", s.input_target_label());
    let input = Paragraph::new(s.input_buffer())
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(title)
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().fg(Color::White));
    f.render_widget(input, fchunks[0]);

    let hint = if s.standby {
        " ✓ standby · [enter] submit  [shift+enter] newline  [tab] pane  [↑↓] nav  [esc] clear  [q] quit"
    } else {
        " [enter] submit  [shift+enter] newline  [tab] pane  [↑↓] nav  [esc] clear  [q] quit"
    };
    let hint_para = Paragraph::new(Line::from(hint))
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(hint_para, fchunks[1]);
}

fn render_goal_entry(f: &mut ratatui::Frame, s: &AppState, area: ratatui::layout::Rect) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(
        " Describe what to build — or edit the goal below, then Continue:",
    ))
    .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    f.render_widget(title, chunks[1]);

    let input = Paragraph::new(s.input_buffer())
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" Goal ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );
    f.render_widget(input, chunks[2]);

    let button_style = if s.goal_continue_focused() {
        Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    };
    let button = Paragraph::new(Line::from("  [ Continue ]   ([enter] start  ·  [shift+enter] newline  ·  [ctrl-c] quit)"))
        .style(button_style)
        .block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(button, chunks[3]);
}
```

(Leave `render_job_detail` unchanged.)

- [ ] **Step 5: Run the render tests**

Run: `cargo test --test tui_render_test`
Expected: PASS (all four).

- [ ] **Step 6: Verify the whole crate compiles**

Run: `cargo build`
Expected: builds clean — no remaining references to `is_editing`/`mode_is_*`.

- [ ] **Step 7: Commit**

```bash
git add src/tui.rs tests/tui_render_test.rs
git commit -m "tui: render goal-entry screen + persistent wrapping input bar"
```

---

## Task 10: Confirm app.rs wiring + full test suite

**Files:**
- Read/verify: `src/app.rs:104-180`

- [ ] **Step 1: Confirm `run_tui` needs no change**

Read `src/app.rs`. Confirm: `AppState::new(goal)` is called with the resolved goal (now used as the entry-screen prefill); `state.on_key(k)` returns `Command::StartRun` which is forwarded over `ctx` by the existing generic `let _ = ctx.send(cmd);` path; only `Command::Quit` breaks the loop. No code change is required. If any `match` on `Command` exists in `app.rs` that is now non-exhaustive, add a `Command::StartRun { .. } => {}` arm. (Expected: none — `app.rs` only uses `matches!(cmd, Command::Quit)`.)

- [ ] **Step 2: Run the entire test suite**

Run: `cargo test`
Expected: PASS — all offline tests green (no tokens spent; uses `fake_agent` + scripted stub).

- [ ] **Step 3: Manual smoke check (optional, no agents)**

Run: `cargo build --release` then `./target/release/agentloop --workspace ./smoke-tmp`
Expected: the TUI opens on the goal-entry screen; nothing runs until you type a goal and press Enter; `Ctrl-C` exits without spawning agents. Clean up: `rm -rf ./smoke-tmp`.

- [ ] **Step 4: Commit (if app.rs changed)**

```bash
git add src/app.rs
git commit -m "app: handle Command::StartRun in TUI wiring"
```

(Skip if no change was needed.)

---

## Task 11: Update README

**Files:**
- Modify: `README.md` (Usage `:22-39`, Interactive mode `:72-90`, How-it-works roles `:43-47`)

- [ ] **Step 1: Document the goal-entry screen in Usage**

Replace the Usage code block + the paragraph about the optional goal (`README.md:24-32`) with text describing that every interactive launch opens a goal-entry screen pre-filled with the existing goal, that nothing runs until you press Continue (`enter`), and that `Ctrl-C` quits without running. Note the goal CLI arg now pre-fills the screen rather than starting immediately.

- [ ] **Step 2: Update the Interactive mode (TUI) key list**

In the keys list (`README.md:74-87`), document the new model:
- printable keys always type into the persistent bottom input (which wraps); `shift+enter` (or `alt+enter`) inserts a newline.
- `enter` submits: answers the selected Inbox question (when Inbox is focused with a question), else adds a task to the planner. With the input empty, `enter` on the Jobs pane opens the selected job's detail.
- `tab` switches Jobs/Inbox focus; `↑`/`↓` navigate the focused pane (or scroll the log in job detail).
- `esc` clears the input (or leaves job detail).
- `q` quits only when the input is empty; **`Ctrl-C` always quits.**

Remove the old `[a] add task` / modal-answer descriptions.

- [ ] **Step 3: Update the roles description**

In "How it works" (`README.md:45-46`), update the routing/role text: roles are `planner`, `build`, `resolver`. The planner owns the technical design (`.agentloop/state/design.md`) and emits a dependency-aware backlog of `build` items; `resolver` handles merge conflicts. Remove mentions implying `architect`/`fix`/`trivial`.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: goal-entry screen, persistent input keys, trimmed roles"
```

---

## Self-Review Notes

- **Spec coverage:** Section 1 (goal-entry) → Tasks 4–7, 9, 11. Section 2 (persistent input) → Tasks 7–9, 11. Section 3 (planner/roles) → Tasks 1–3, 11. The Enter-vs-job-detail ambiguity is resolved explicitly in the header and Task 7.
- **Type consistency:** `Command::StartRun { goal }` is defined (Task 4) and matched in `orchestrator` (Task 5) and `tui` (Task 7). `commit_goal(ws, goal)` defined in Task 6, called in Task 5. `input_target_label`, `in_goal_entry`, `goal_continue_focused` defined in Task 7 and used by render in Task 9. `View::{GoalEntry,List,JobDetail}` consistent across Tasks 7 and 9.
- **No placeholders:** all code shown in full; no TBD/TODO.
- **Build-order caveat:** Tasks 4–6 leave intermediate non-compiling states between commits (documented in each task). If a green build per commit is required, implement Tasks 4, 5, 6 back-to-back before re-running `cargo build`.
