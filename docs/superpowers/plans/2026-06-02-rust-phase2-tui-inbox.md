# agentloop Rust — Phase 2 (TUI + question inbox) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Prerequisite:** Phase 1 plan (`2026-06-02-rust-port-phase1.md`) is complete and its tests pass.

**Goal:** Add a `ratatui` terminal UI with a live progress panel and a navigable question inbox, plus the async file-based mechanism for agents to ask the user a question and resume once answered.

**Architecture:** The orchestrator and the TUI run concurrently and communicate over `tokio::sync::mpsc` channels: the orchestrator emits `Event`s, the TUI sends `Command`s. The orchestrator remains the single writer of `.agentloop/`. An agent raises a question by writing `.agentloop/questions/<id>.json` and a result with `status:"needs_input"`; the orchestrator marks the item `blocked` and emits `QuestionRaised`; the user answers in the inbox; the answer is persisted to `.agentloop/answers/<id>.json`, the item flips to `ready`, and the next dispatch re-includes the prior Q&A.

**Tech Stack:** Adds `ratatui` + `crossterm` to the Phase 1 stack. The `Reporter` trait becomes channel-backed; the headless `EventLineReporter` from Phase 1 stays for non-TTY mode.

---

## File Structure (changes from Phase 1)

```
src/events.rs       EXTEND: Event enum, Command enum, ChannelReporter (Reporter -> mpsc::Sender<Event>)
src/inbox.rs        NEW: Question/Answer file IO; raise/list/answer/consume; prior-Q&A prompt block
src/worker.rs       MODIFY: worker_prompt gains needs_input clause + prior-Q&A block
src/orchestrator.rs MODIFY: handle needs_input -> blocked + QuestionRaised; answer routing; blocked-aware termination
src/tui.rs          NEW: ratatui App (view-model + render + input -> Command); run loop
src/app.rs          NEW: wires orchestrator task + TUI over channels; TTY detection -> TUI vs headless
src/cli.rs          MODIFY: run() picks app::run_tui (TTY) or headless orchestrator::run (non-TTY)
tests/inbox_test.rs        NEW
tests/loop_needs_input_test.rs  NEW (fake agent emits needs_input, answer re-dispatches to done)
tests/tui_viewmodel_test.rs     NEW (view-model snapshot; render is manually verified)
```

---

### Task 1: events.rs — Event/Command enums + channel reporter

**Files:**
- Modify: `src/events.rs`
- Modify: `src/lib.rs` (add `pub mod inbox; pub mod tui; pub mod app;`)

- [ ] **Step 1: Extend `src/events.rs`** (keep the existing `Reporter` trait + `EventLineReporter`, add below)

```rust
use tokio::sync::mpsc;

/// Orchestrator -> UI.
#[derive(Debug, Clone)]
pub enum Event {
    JobDispatched { id: String, label: String, tool: String, model: String },
    JobStatus { id: String, status: String },
    QuestionRaised { item_id: String, label: String, text: String, context: String },
    Iteration { n: u32, merged: u32, gate: String, open: i64 },
    EnteredStandby,   // used in Phase 3; defined here so the enum is stable
    Shutdown,
}

/// UI -> orchestrator.
#[derive(Debug, Clone)]
pub enum Command {
    AnswerQuestion { item_id: String, text: String },
    AddTask { request: String }, // Phase 3
    Quit,
}

/// Reporter that forwards to the TUI over a channel.
pub struct ChannelReporter {
    tx: mpsc::UnboundedSender<Event>,
}

impl ChannelReporter {
    pub fn new(tx: mpsc::UnboundedSender<Event>) -> Self { Self { tx } }
    pub fn question(&self, item_id: &str, label: &str, text: &str, context: &str) {
        let _ = self.tx.send(Event::QuestionRaised {
            item_id: item_id.into(), label: label.into(), text: text.into(), context: context.into(),
        });
    }
}

impl Reporter for ChannelReporter {
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str) {
        let _ = self.tx.send(Event::JobDispatched { id: id.into(), label: label.into(), tool: tool.into(), model: model.into() });
    }
    fn status(&self, id: &str, status: &str, _tool: &str, _model: &str) {
        let _ = self.tx.send(Event::JobStatus { id: id.into(), status: status.into() });
    }
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64) {
        let _ = self.tx.send(Event::Iteration { n, merged, gate: gate.into(), open });
    }
}
```

The `Reporter` trait gains one method so the orchestrator can raise questions through the same seam:

```rust
pub trait Reporter: Send + Sync {
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str);
    fn status(&self, id: &str, status: &str, tool: &str, model: &str);
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64);
    fn question(&self, _item_id: &str, _label: &str, _text: &str, _context: &str) {} // default no-op
}
```

Add the matching `question` override to `ChannelReporter` (above) and leave `EventLineReporter` using the default (it can also `eprintln!` a `question:` line — implement that for headless visibility).

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: compiles (tui/app/inbox are empty stubs for now).

- [ ] **Step 3: Commit**

```bash
git add src/events.rs src/lib.rs
git commit -m "feat(events): Event/Command enums + ChannelReporter + question seam"
```

---

### Task 2: inbox.rs — question/answer file IO

**Files:**
- Modify: `src/inbox.rs`
- Test: `tests/inbox_test.rs`

On-disk contract: `.agentloop/questions/<id>.json` = `{"question","context"}` (written by agent); `.agentloop/answers/<id>.json` = `{"question","answer","ts"}` (written by orchestrator on answer). Answered questions are archived under `.agentloop/logs/`.

- [ ] **Step 1: Write the failing test** in `tests/inbox_test.rs`

```rust
use agentloop::inbox;
use std::path::PathBuf;

fn tmp_ws() -> PathBuf {
    let ws = std::env::temp_dir().join(format!("alinbox-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(ws.join(".agentloop/questions")).unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/answers")).unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/logs")).unwrap();
    ws
}

#[test]
fn read_question_and_record_answer() {
    let ws = tmp_ws();
    // Agent wrote a question file.
    std::fs::write(ws.join(".agentloop/questions/it-1.json"),
        r#"{"question":"SQLite or Postgres?","context":"storage layer"}"#).unwrap();

    let q = inbox::read_question(&ws, "it-1").unwrap();
    assert_eq!(q.question, "SQLite or Postgres?");
    assert_eq!(q.context, "storage layer");

    // Orchestrator records the user's answer.
    inbox::record_answer(&ws, "it-1", "SQLite or Postgres?", "SQLite").unwrap();
    let a = inbox::read_answer(&ws, "it-1").unwrap();
    assert_eq!(a.answer, "SQLite");

    // Prior-Q&A block for the re-dispatch prompt.
    let block = inbox::prior_qa_block(&ws, "it-1").unwrap();
    assert!(block.contains("SQLite or Postgres?"));
    assert!(block.contains("SQLite"));

    // Consuming archives the question file so it isn't re-raised.
    inbox::consume_question(&ws, "it-1").unwrap();
    assert!(!ws.join(".agentloop/questions/it-1.json").exists());
}

#[test]
fn missing_question_is_none() {
    let ws = tmp_ws();
    assert!(inbox::read_question(&ws, "nope").is_err());
    assert!(inbox::prior_qa_block(&ws, "nope").unwrap().is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test inbox_test`
Expected: FAIL — `inbox` functions undefined.

- [ ] **Step 3: Implement `src/inbox.rs`**

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub question: String,
    pub answer: String,
    pub ts: i64,
}

fn qpath(ws: &Path, id: &str) -> std::path::PathBuf { ws.join(format!(".agentloop/questions/{id}.json")) }
fn apath(ws: &Path, id: &str) -> std::path::PathBuf { ws.join(format!(".agentloop/answers/{id}.json")) }

pub fn read_question(ws: &Path, id: &str) -> Result<Question> {
    let text = std::fs::read_to_string(qpath(ws, id)).with_context(|| format!("no question for {id}"))?;
    serde_json::from_str(&text).context("parse question json")
}

pub fn has_question(ws: &Path, id: &str) -> bool { qpath(ws, id).exists() }

pub fn record_answer(ws: &Path, id: &str, question: &str, answer: &str) -> Result<()> {
    let a = Answer { question: question.into(), answer: answer.into(), ts: chrono::Local::now().timestamp() };
    std::fs::create_dir_all(ws.join(".agentloop/answers"))?;
    std::fs::write(apath(ws, id), serde_json::to_vec_pretty(&a)?)?;
    Ok(())
}

pub fn read_answer(ws: &Path, id: &str) -> Result<Answer> {
    let text = std::fs::read_to_string(apath(ws, id)).with_context(|| format!("no answer for {id}"))?;
    serde_json::from_str(&text).context("parse answer json")
}

/// A prompt block describing the prior question + the user's answer, or "" if none.
pub fn prior_qa_block(ws: &Path, id: &str) -> Result<String> {
    match read_answer(ws, id) {
        Ok(a) => Ok(format!(
            "\n\nEARLIER YOU ASKED THE USER A QUESTION; HERE IS THEIR ANSWER:\n  Q: {}\n  A: {}\nProceed using this answer.",
            a.question, a.answer)),
        Err(_) => Ok(String::new()),
    }
}

/// Archive the question file under logs/ so it isn't re-raised.
pub fn consume_question(ws: &Path, id: &str) -> Result<()> {
    let q = qpath(ws, id);
    if q.exists() {
        let dest = ws.join(format!(".agentloop/logs/answered-{id}.json"));
        std::fs::rename(&q, &dest).or_else(|_| std::fs::remove_file(&q))?;
    }
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test inbox_test`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/inbox.rs tests/inbox_test.rs
git commit -m "feat(inbox): question/answer file IO + prior-Q&A prompt block"
```

---

### Task 3: worker prompt — needs_input clause + prior Q&A

**Files:**
- Modify: `src/worker.rs`
- Test: `tests/planner_worker_test.rs` (add a case)

- [ ] **Step 1: Add the failing test** to `tests/planner_worker_test.rs`

```rust
#[test]
fn worker_prompt_documents_needs_input_and_prior_qa() {
    let ws = std::env::temp_dir().join(format!("alwq-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(ws.join(".agentloop/answers")).unwrap();
    // pre-existing answer should be injected into the prompt
    agentloop::inbox::record_answer(&ws, "it-9", "DB?", "SQLite").unwrap();

    let item = serde_json::json!({"id":"it-9","title":"T","desc":"D","role":"build","acceptance":"A"});
    let p = agentloop::worker::worker_prompt(&ws, &item);
    assert!(p.contains("needs_input"), "documents the needs_input escape hatch");
    assert!(p.contains("questions/it-9.json"));
    assert!(p.contains("SQLite"), "prior answer injected");
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test planner_worker_test`
Expected: FAIL — prompt lacks the new clauses.

- [ ] **Step 3: Update `worker_prompt` in `src/worker.rs`**

Append a needs_input clause and the prior-Q&A block (from `inbox::prior_qa_block`):

```rust
pub fn worker_prompt(ws: &Path, item: &Value) -> String {
    let id = item["id"].as_str().unwrap_or("");
    let title = item["title"].as_str().unwrap_or("");
    let desc = item["desc"].as_str().unwrap_or("");
    let acc = item["acceptance"].as_str().unwrap_or("the change builds and tests pass");
    let prior = crate::inbox::prior_qa_block(ws, id).unwrap_or_default();
    format!(r#"You are a WORKER on an autonomous app build. You are in a git worktree of the project.
Implement exactly this item and nothing else:

  id:    {id}
  title: {title}
  task:  {desc}
  done when: {acc}

Rules:
- Make focused commits in this worktree as you go.
- Verify your work against the acceptance criteria before finishing.
- When finished, write {ws}/.agentloop/results/{id}.json:
  {{"status":"done|failed","summary":"one line","files_changed":["..."]}}
- If you are blocked needing a decision that only the user can make, DO NOT guess.
  Write {ws}/.agentloop/questions/{id}.json:
  {{"question":"<your question>","context":"<brief context>"}}
  and write the result file with {{"status":"needs_input","summary":"<what you need>"}} instead,
  then stop. The user will answer and you will be re-dispatched with their answer.{prior}"#,
        id = id, title = title, desc = desc, acc = acc, ws = ws.display(), prior = prior)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test planner_worker_test`
Expected: PASS (now 3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/worker.rs tests/planner_worker_test.rs
git commit -m "feat(worker): needs_input escape hatch + prior-Q&A injection"
```

---

### Task 4: orchestrator — handle needs_input, route answers, blocked-aware termination

**Files:**
- Modify: `src/orchestrator.rs`
- Test: `tests/loop_needs_input_test.rs`
- Modify: `tests/common/mod.rs` (add a stub variant that asks once, then completes)

This is the behavioral core of Phase 2. Three changes:

1. **Integration** — a result with `status:"needs_input"` → set item `blocked` with the question text in `notes`, emit `reporter.question(...)`, clean up the worktree (no merge). Add this branch before the existing done/failed handling.
2. **Answer application** — a new `apply_answer(ws, item_id, text)` that records the answer (`inbox::record_answer`), consumes the question file, and flips the item `blocked`→`ready`. Called by the app loop when a `Command::AnswerQuestion` arrives (Task 6).
3. **Termination** — `run` learns about blocked items: if the only open work is `blocked` (awaiting the user) and nothing merged, do **not** count it as a stall; instead keep iterating/idling rather than tripping the no-progress stop. Add a `state::blocked_count` helper.

- [ ] **Step 1: Add the stub variant** to `tests/common/mod.rs`

```rust
/// Stub that, as worker, asks a question the FIRST time (needs_input) and completes
/// the SECOND time (after an answer exists). Planner seeds one item, marks done when result present.
pub fn init_ws_with_asking_stub() -> PathBuf {
    let ws = init_ws_with_stub(); // reuse base setup
    let stub = r#"#!/bin/bash
tool="$1"; shift
ws="$WS"; ws_state="$ws/.agentloop/state"; res="$ws/.agentloop/results"
prompt="$*"
case "$prompt" in
  *PLANNER*)
    n=$(cat "$ws/.plan_n" 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > "$ws/.plan_n"
    if [ "$n" -eq 1 ]; then
      echo '{"items":[{"id":"it-1","title":"f","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/backlog.json"
      printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$ws/.agentloop/verify.sh"; chmod +x "$ws/.agentloop/verify.sh"
    elif [ -f "$res/it-1.json" ]; then
      python3 -c "import json; p='$ws_state/backlog.json'; d=json.load(open(p)); [i.__setitem__('status','done') for i in d['items']]; json.dump(d,open(p,'w'))"
    fi
    echo "# updated" > "$ws_state/master.md"
    ;;
  *WORKER*)
    if [ -f "$ws/.agentloop/answers/it-1.json" ]; then
      echo made > "$PWD/made.txt"; git add -A; git commit -qm "worker" 2>/dev/null
      echo '{"status":"done","summary":"made file","files_changed":["made.txt"]}' > "$res/it-1.json"
    else
      echo '{"question":"make the file?","context":"need confirm"}' > "$ws/.agentloop/questions/it-1.json"
      echo '{"status":"needs_input","summary":"confirm"}' > "$res/it-1.json"
    fi
    ;;
esac
exit 0
"#;
    std::fs::write(ws.join("stub.sh"), stub).unwrap();
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(ws.join("stub.sh"), std::fs::Permissions::from_mode(0o755)).unwrap(); }
    ws
}
```

- [ ] **Step 2: Write the failing test** in `tests/loop_needs_input_test.rs`

```rust
mod common;
use agentloop::config::Config;
use agentloop::events::EventLineReporter;
use agentloop::{orchestrator, state};
use std::sync::Arc;

fn cfg() -> Config {
    serde_yaml::from_str(r#"
caps: { max_iterations: 6, max_parallel: 1, item_timeout_sec: 30, total_budget_sec: 300, max_attempts: 5 }
routing:
  planner: { tool: claude, model: opus, effort: high, flags: "" }
  build:   { tool: codex,  model: gpt-5, effort: high, flags: "" }
defaults: { role: build }
"#).unwrap()
}

#[tokio::test]
async fn item_goes_blocked_then_answer_completes() {
    let ws = common::init_ws_with_asking_stub();
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);
    let bk = ws.join(".agentloop/state/backlog.json");

    // One iteration: worker asks -> item becomes blocked, not merged.
    let merged = orchestrator::iterate(&cfg(), &ws, 1, &(Arc::new(EventLineReporter) as Arc<dyn agentloop::events::Reporter>)).await.unwrap();
    assert_eq!(merged, 0);
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "it-1").unwrap()["status"], "blocked");

    // User answers -> item flips to ready.
    orchestrator::apply_answer(&ws, "it-1", "yes").unwrap();
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "it-1").unwrap()["status"], "ready");

    // Next iteration completes (stub now sees an answer file).
    let merged2 = orchestrator::iterate(&cfg(), &ws, 2, &(Arc::new(EventLineReporter) as Arc<dyn agentloop::events::Reporter>)).await.unwrap();
    assert_eq!(merged2, 1);
    assert_eq!(std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(), "made");

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --test loop_needs_input_test`
Expected: FAIL — needs_input not handled; `apply_answer` undefined.

- [ ] **Step 4: Implement the changes in `src/orchestrator.rs`**

Add to the integration loop, as the first branch when reading the result:

```rust
// inside the `for id in &dispatched` loop, after reading the result Value `rv`:
let status = result_value.as_ref().map(|v| v["status"].clone()).unwrap_or(serde_json::Value::Null);
if status == "needs_input" {
    // Surface the question; block the item; do not merge.
    if let Ok(q) = crate::inbox::read_question(ws, id) {
        let label = state::item(&state::read(&bk)?, id)
            .and_then(|i| i["title"].as_str().map(String::from)).unwrap_or_default();
        reporter.question(id, &label, &q.question, &q.context);
        state::set_status(&bk, id, "blocked", &q.question)?;
    } else {
        // Malformed/missing question file: treat as a normal non-done -> bounce.
        state::set_status(&bk, id, "ready", "needs_input without a question file")?;
        reporter.status(id, "bounced", "", "");
    }
    worktree::remove(ws, &ws.join(format!(".agentloop/worktrees/{id}")), &format!("item/{id}"));
    let _ = std::fs::remove_file(&rfile);
    continue;
}
```

> Refactor note: read the result file once into `result_value: Option<Value>` near the top of the loop body and derive both `result_done` (status=="done") and the `needs_input` branch from it, instead of reading twice.

Add the answer-application function:

```rust
/// Apply a user's answer to a blocked item: persist it, consume the question,
/// flip the item blocked->ready so it is re-dispatched with the prior Q&A.
pub fn apply_answer(ws: &Path, item_id: &str, text: &str) -> Result<()> {
    let bk = ws.join(".agentloop/state/backlog.json");
    let question = crate::inbox::read_question(ws, item_id)
        .map(|q| q.question)
        .or_else(|_| {
            // question may already be in notes if the file was consumed
            let v = state::read(&bk)?;
            Ok::<String, anyhow::Error>(state::item(&v, item_id).and_then(|i| i["notes"].as_str().map(String::from)).unwrap_or_default())
        })?;
    crate::inbox::record_answer(ws, item_id, &question, text)?;
    let _ = crate::inbox::consume_question(ws, item_id);
    state::set_status(&bk, item_id, "ready", "answered; re-dispatching")?;
    Ok(())
}
```

Add the blocked counter to `src/state.rs`:

```rust
pub fn blocked_count(path: &Path) -> Result<i64> {
    let v = read(path)?;
    let empty = vec![];
    Ok(v["items"].as_array().unwrap_or(&empty).iter()
        .filter(|i| i["status"] == "blocked").count() as i64)
}
```

Adjust `run`'s stall logic so blocked-only does not stall:

```rust
let open = state::open_count(&bk)?;
let blocked = state::blocked_count(&bk)?;
reporter.iteration(n, merged, gate_state, open);

if gate_state == "pass" && open == 0 { eprintln!("DONE"); return Ok(0); }

// If the only open work is items blocked on the user, idle instead of stalling.
let awaiting_user = open > 0 && open == blocked;
if awaiting_user {
    stalls = 0;
    // In headless mode there is no one to answer; avoid a busy spin.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    prev_gate = gate_state.to_string();
    continue;
}

if merged == 0 && gate_state == prev_gate {
    stalls += 1;
    if stalls >= 2 { eprintln!("STOP: no progress for 2 stalls (3 consecutive iterations)"); return Ok(1); }
} else {
    stalls = 0;
}
prev_gate = gate_state.to_string();
```

> In headless mode `awaiting_user` would loop forever waiting for an answer that can't come. Guard it: in headless (`EventLineReporter`) mode, after one `awaiting_user` pass with no external answer, return `Ok(1)` with "STOP: blocked on user input (headless)". Detect headless via a bool flag threaded into `run` (Task 6 passes `interactive: bool`). For Phase 2 add the `interactive` param to `run` now and short-circuit headless `awaiting_user` to the stop.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --test loop_needs_input_test`
Expected: PASS.

- [ ] **Step 6: Run the full suite (no regressions)**

Run: `cargo test`
Expected: all PASS (Phase 1 loop_test still green).

- [ ] **Step 7: Commit**

```bash
git add src/orchestrator.rs src/state.rs tests/loop_needs_input_test.rs tests/common/mod.rs
git commit -m "feat(orchestrator): needs_input -> blocked, answer routing, blocked-aware termination"
```

---

### Task 5: tui.rs — view-model + render + input mapping

**Files:**
- Modify: `src/tui.rs`
- Test: `tests/tui_viewmodel_test.rs`

The render to a real terminal is verified manually (Task 7). The **view-model** (the pure state the UI derives from events + the input→Command mapping) is unit-tested so the logic is covered without a TTY.

- [ ] **Step 1: Write the failing test** in `tests/tui_viewmodel_test.rs`

```rust
use agentloop::events::{Event, Command};
use agentloop::tui::AppState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn applies_events_to_view_model() {
    let mut s = AppState::new("build a todo app".into());
    s.apply(Event::JobDispatched { id: "it-1".into(), label: "scaffold".into(), tool: "codex".into(), model: "gpt-5".into() });
    s.apply(Event::QuestionRaised { item_id: "db".into(), label: "db-schema".into(), text: "SQLite or Postgres?".into(), context: "storage".into() });
    assert_eq!(s.jobs.len(), 1);
    assert_eq!(s.inbox.len(), 1);
    assert_eq!(s.inbox[0].item_id, "db");

    s.apply(Event::JobStatus { id: "it-1".into(), status: "merged".into() });
    assert_eq!(s.jobs.iter().find(|j| j.id == "it-1").unwrap().status, "merged");
}

#[test]
fn key_input_maps_to_commands() {
    let mut s = AppState::new("g".into());
    s.apply(Event::QuestionRaised { item_id: "db".into(), label: "db".into(), text: "q?".into(), context: "".into() });

    // 'enter' on selected question opens the answer editor; typing + enter -> AnswerQuestion
    assert!(matches!(s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), None)); // opens editor
    s.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    s.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    s.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::AnswerQuestion { ref item_id, ref text }) if item_id == "db" && text == "yes"));

    // 'q' (in normal mode) -> Quit
    assert!(matches!(s.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)), Some(Command::Quit)));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test tui_viewmodel_test`
Expected: FAIL — `AppState` undefined.

- [ ] **Step 3: Implement the view-model in `src/tui.rs`**

```rust
use crossterm::event::{KeyCode, KeyEvent};
use crate::events::{Command, Event};

#[derive(Clone)]
pub struct Job { pub id: String, pub label: String, pub tool: String, pub model: String, pub status: String }
#[derive(Clone)]
pub struct Pending { pub item_id: String, pub label: String, pub text: String, pub context: String }

#[derive(PartialEq)]
enum Mode { Normal, Answering, AddingTask } // AddingTask used in Phase 3

pub struct AppState {
    pub goal: String,
    pub jobs: Vec<Job>,
    pub inbox: Vec<Pending>,
    pub selected: usize,
    pub iter: u32, pub gate: String, pub open: i64,
    pub standby: bool, // Phase 3
    mode: Mode,
    input: String,
}

impl AppState {
    pub fn new(goal: String) -> Self {
        Self { goal, jobs: vec![], inbox: vec![], selected: 0, iter: 0,
               gate: "init".into(), open: 0, standby: false, mode: Mode::Normal, input: String::new() }
    }

    pub fn apply(&mut self, ev: Event) {
        match ev {
            Event::JobDispatched { id, label, tool, model } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    j.label = label; j.tool = tool; j.model = model; j.status = "running".into();
                } else {
                    self.jobs.push(Job { id, label, tool, model, status: "running".into() });
                }
            }
            Event::JobStatus { id, status } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) { j.status = status; }
            }
            Event::QuestionRaised { item_id, label, text, context } => {
                if !self.inbox.iter().any(|p| p.item_id == item_id) {
                    self.inbox.push(Pending { item_id, label, text, context });
                }
            }
            Event::Iteration { n, merged: _, gate, open } => { self.iter = n; self.gate = gate; self.open = open; }
            Event::EnteredStandby => { self.standby = true; }
            Event::Shutdown => {}
        }
    }

    /// Map a key to an optional Command. Returns None when the key only changes UI state.
    pub fn on_key(&mut self, k: KeyEvent) -> Option<Command> {
        match self.mode {
            Mode::Normal => match k.code {
                KeyCode::Char('q') => Some(Command::Quit),
                KeyCode::Char('a') => { self.mode = Mode::AddingTask; self.input.clear(); None } // Phase 3
                KeyCode::Up => { if self.selected > 0 { self.selected -= 1; } None }
                KeyCode::Down => { if self.selected + 1 < self.inbox.len() { self.selected += 1; } None }
                KeyCode::Enter => { if !self.inbox.is_empty() { self.mode = Mode::Answering; self.input.clear(); } None }
                _ => None,
            },
            Mode::Answering => match k.code {
                KeyCode::Esc => { self.mode = Mode::Normal; self.input.clear(); None }
                KeyCode::Backspace => { self.input.pop(); None }
                KeyCode::Char(c) => { self.input.push(c); None }
                KeyCode::Enter => {
                    let p = self.inbox.remove(self.selected.min(self.inbox.len().saturating_sub(1)));
                    let text = std::mem::take(&mut self.input);
                    self.selected = 0;
                    self.mode = Mode::Normal;
                    Some(Command::AnswerQuestion { item_id: p.item_id, text })
                }
                _ => None,
            },
            Mode::AddingTask => match k.code { // Phase 3 wires the submit
                KeyCode::Esc => { self.mode = Mode::Normal; self.input.clear(); None }
                KeyCode::Backspace => { self.input.pop(); None }
                KeyCode::Char(c) => { self.input.push(c); None }
                KeyCode::Enter => {
                    let text = std::mem::take(&mut self.input);
                    self.mode = Mode::Normal;
                    Some(Command::AddTask { request: text })
                }
                _ => None,
            },
        }
    }

    pub fn input_buffer(&self) -> &str { &self.input }
    pub fn is_editing(&self) -> bool { self.mode != Mode::Normal }
}
```

Note: the `'y','e','s'` test types into the answer editor → buffer `"yes"`; `Enter` emits `AnswerQuestion{text:"yes"}`. Matches the test.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test tui_viewmodel_test`
Expected: PASS (2 tests).

- [ ] **Step 5: Add the render function (manually verified, no unit test)**

Add `pub fn render(f: &mut ratatui::Frame, s: &AppState)` drawing the layout from the spec: top status bar (goal, iter, gate, open, ❓count), a horizontal split — left `jobs` list (glyph per status: running ●, merged ✓, failed ✗, bounced ↺, queued ·), right `inbox` list (highlight `selected`); a footer that shows keybindings in Normal mode or the input buffer + `[enter] submit [esc] cancel` while editing; and, when `s.standby`, the standby banner. Use `ratatui::widgets::{Block, Borders, List, ListItem, Paragraph}` and `Layout`. Keep it under ~120 lines.

- [ ] **Step 6: Commit**

```bash
git add src/tui.rs tests/tui_viewmodel_test.rs
git commit -m "feat(tui): app view-model + key->command mapping + ratatui render"
```

---

### Task 6: app.rs — wire orchestrator + TUI over channels

**Files:**
- Modify: `src/app.rs`
- Modify: `src/cli.rs` (dispatch TTY → `app::run_tui`, non-TTY → headless `orchestrator::run`)

- [ ] **Step 1: Implement `src/app.rs`**

```rust
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::events::{ChannelReporter, Command, Event, Reporter};
use crate::orchestrator;
use crate::tui::{self, AppState};

/// Run the interactive TUI: orchestrator task + ratatui loop, joined by channels.
pub async fn run_tui(cfg: Config, ws: std::path::PathBuf, goal: String) -> Result<i32> {
    let (etx, mut erx) = mpsc::unbounded_channel::<Event>();
    let (ctx, mut crx) = mpsc::unbounded_channel::<Command>();

    // Orchestrator task.
    let reporter: Arc<dyn Reporter> = Arc::new(ChannelReporter::new(etx.clone()));
    let cfg_o = cfg.clone();
    let ws_o = ws.clone();
    let orch = tokio::spawn(async move {
        // `run` consumes commands via crx for answers/add-task/quit (Phase 3 add-task).
        orchestrator::run_interactive(&cfg_o, &ws_o, reporter, &mut crx).await
    });

    // TUI: terminal setup + event/render loop.
    let mut terminal = tui::setup_terminal()?;
    let mut state = AppState::new(goal);
    let res = tui::event_loop(&mut terminal, &mut state, &mut erx, &ctx).await;
    tui::restore_terminal(&mut terminal)?;

    let _ = ctx.send(Command::Quit);
    let rc = orch.await.unwrap_or(Ok(1)).unwrap_or(1);
    res.map(|_| rc)
}
```

> `orchestrator::run_interactive` is `run` extended to also `tokio::select!` on the `Command` receiver: `AnswerQuestion` → `apply_answer` (+ emit a status), `AddTask` → Phase 3, `Quit` → break and shut down agents. For Phase 2, implement `AnswerQuestion` and `Quit`; leave `AddTask` matched but a no-op (Phase 3 fills it). Thread `interactive: true` so the awaiting-user idle does not auto-stop.

`tui::setup_terminal`/`restore_terminal`/`event_loop` (in `src/tui.rs`): enter alt-screen + raw mode; the loop `tokio::select!`s a ~60ms render tick, drained `erx` events (`state.apply`), and `crossterm` key events (`state.on_key` → forward returned `Command` on `ctx`; on `Command::Quit` break). On every tick call `tui::render`. `restore_terminal` always runs (guard with a scopeguard or explicit call on all exit paths) so a panic doesn't leave the terminal in raw mode.

- [ ] **Step 2: Update `src/cli.rs` `run()`** to branch on TTY

```rust
use std::io::IsTerminal;
// ... after loading cfg and bootstrapping ...
if std::io::stdout().is_terminal() {
    let rc = crate::app::run_tui(cfg, ws.clone(), args.goal.clone()).await?;
    std::process::exit(rc);
} else {
    let rc = orchestrator::run(&cfg, &ws, Arc::new(EventLineReporter), /*interactive=*/false).await?;
    eprintln!("=== agentloop finished (rc={rc}). ===");
    std::process::exit(rc);
}
```

- [ ] **Step 3: Build + full test suite**

Run: `cargo build && cargo test`
Expected: compiles; all tests PASS (headless path unchanged; TUI path not unit-tested here).

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/cli.rs src/orchestrator.rs
git commit -m "feat(app): wire orchestrator + ratatui TUI over channels; TTY dispatch"
```

---

### Task 7: Manual verification (TUI smoke with fake agent)

- [ ] **Step 1: Run the asking-stub end-to-end under a real terminal**

Build, then run the binary in a terminal against the asking stub from `tests/common` (replicate its workspace setup in a scratch dir, export `FAKE_AGENT=1 FAKE_AGENT_BIN=<stub> WS=<ws>`), and run `agentloop "make one file" --workspace <ws>`.

Expected, observed by eye:
- progress panel shows the planner then the `it-1` worker;
- a question appears in the inbox ("make the file?");
- arrow-key to it, press `enter`, type `yes`, press `enter`;
- the item re-dispatches, `made.txt` is created and merged, the run reaches DONE;
- `q` exits cleanly and the terminal is restored (no raw-mode breakage).

- [ ] **Step 2: Commit any fixes found during manual verification**

```bash
git commit -am "fix(tui): adjustments from manual verification"
```

---

## Self-Review

**Spec coverage (Phase 2):**
- Agent raises question via `questions/<id>.json` + `needs_input` → Task 3 (prompt) + Task 4 (handling). ✓
- Orchestrator: blocked status, `QuestionRaised`, no merge, worktree cleanup → Task 4. ✓
- Blocked items don't trip the stall; awaiting-input idling (interactive) / safe stop (headless) → Task 4. ✓
- TUI inbox navigation + inline answer → Task 5 (view-model/keys) + Task 6 (loop) + Task 7 (manual). ✓
- Answer persisted to `answers/<id>.json`, question consumed, item ready, prior-Q&A re-injected → Task 2 + Task 3 + Task 4. ✓
- Channels/actor model (Event/Command, ChannelReporter) → Task 1 + Task 6. ✓
- Headless preserved (EventLineReporter, non-TTY path) → Task 6. ✓

**Deferred to Phase 3:** `Command::AddTask` handling, `requests.jsonl`, planner "PENDING USER REQUESTS" section, standby lifecycle, the `'a'` add-task editor submit wiring (view-model already emits `AddTask`; the orchestrator no-ops it until Phase 3).

**Type consistency:** `Event`/`Command` variants used identically across events.rs, tui.rs (`apply`/`on_key`), app.rs. `AppState::{apply,on_key,input_buffer,is_editing}` consistent between Task 5 and Task 6. `inbox::{read_question,record_answer,read_answer,prior_qa_block,consume_question,has_question}` consistent across Tasks 2,3,4. `orchestrator::{iterate,run,run_interactive,apply_answer}` and the added `interactive` param consistent across Tasks 4,6. `state::blocked_count` added in Task 4 and used in `run`.

**Placeholder scan:** `Mode::AddingTask` and `Command::AddTask` are defined now but intentionally inert until Phase 3 — they are complete code, not placeholders, and are called out as deferred behavior, not missing implementation. The ratatui `render` body is specified by content/widgets rather than full source (Task 5 Step 5) because it's the one manually-verified seam; everything it reads (`AppState` fields) is fully defined.
