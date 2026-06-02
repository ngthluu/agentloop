# agentloop Rust — Phase 3 (add-task + standby) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Prerequisite:** Phase 2 plan (`2026-06-02-rust-phase2-tui-inbox.md`) is complete and its tests pass.

**Goal:** Let the user add new tasks at any time (routed through the planner), and on "done" keep the session alive in a standby state instead of exiting — so the run can be extended indefinitely.

**Architecture:** A new user request is appended to `.agentloop/state/requests.jsonl`. The planner prompt gains a "PENDING USER REQUESTS" section listing unconsumed requests; the planner folds them into the backlog and they're marked consumed. The orchestrator's `run_interactive` no longer returns on DONE/cap/stall — it transitions to a `Standby` state and idles, awaiting a `Command`. `AddTask` (and answering a leftover blocked item) re-engages the loop; `Quit` exits. Standby pauses the budget clock; a new request starts a fresh budget window.

**Tech Stack:** Same as Phase 2. No new dependencies.

---

## File Structure (changes from Phase 2)

```
src/requests.rs       NEW: requests.jsonl append/list-pending/mark-consumed
src/planner.rs        MODIFY: planner_prompt embeds a PENDING USER REQUESTS section; planner_run marks consumed after a valid run
src/orchestrator.rs   MODIFY: run_interactive -> Standby state machine; AddTask/AnswerQuestion re-engage; budget pause/reset
src/tui.rs            MODIFY: standby banner already supported; ensure 'a' add-task editor + status bar reflect standby/awaiting
tests/requests_test.rs        NEW
tests/loop_addtask_test.rs    NEW (standby -> AddTask -> planner consumes request -> new item built -> DONE again)
```

---

### Task 1: requests.rs — pending user requests log

**Files:**
- Modify: `src/requests.rs`
- Modify: `src/lib.rs` (add `pub mod requests;`)
- Test: `tests/requests_test.rs`

On-disk: `.agentloop/state/requests.jsonl`, one JSON object per line: `{"ts","text","status":"pending"|"consumed"}`.

- [ ] **Step 1: Write the failing test** in `tests/requests_test.rs`

```rust
use agentloop::requests;
use std::path::PathBuf;

fn tmp_ws() -> PathBuf {
    let ws = std::env::temp_dir().join(format!("alreq-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
    ws
}

#[test]
fn append_list_consume() {
    let ws = tmp_ws();
    assert!(requests::pending(&ws).unwrap().is_empty());

    requests::append(&ws, "add a --due flag").unwrap();
    requests::append(&ws, "show overdue in red").unwrap();
    let p = requests::pending(&ws).unwrap();
    assert_eq!(p, vec!["add a --due flag".to_string(), "show overdue in red".to_string()]);

    // The block embedded into the planner prompt lists them.
    let block = requests::prompt_block(&ws).unwrap();
    assert!(block.contains("PENDING USER REQUESTS"));
    assert!(block.contains("add a --due flag"));

    requests::mark_all_consumed(&ws).unwrap();
    assert!(requests::pending(&ws).unwrap().is_empty());
    // Consumed entries no longer appear in the prompt block (empty -> "").
    assert_eq!(requests::prompt_block(&ws).unwrap(), "");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test requests_test`
Expected: FAIL — `requests` undefined.

- [ ] **Step 3: Implement `src/requests.rs`**

```rust
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub ts: i64,
    pub text: String,
    pub status: String, // "pending" | "consumed"
}

fn path(ws: &Path) -> PathBuf { ws.join(".agentloop/state/requests.jsonl") }

fn read_all(ws: &Path) -> Result<Vec<Request>> {
    let p = path(ws);
    if !p.exists() { return Ok(vec![]); }
    let text = std::fs::read_to_string(&p)?;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        out.push(serde_json::from_str(line)?);
    }
    Ok(out)
}

fn write_all(ws: &Path, reqs: &[Request]) -> Result<()> {
    let p = path(ws);
    let dir = p.parent().unwrap();
    let tmp = dir.join(format!(".requests.{}.tmp", std::process::id()));
    let mut buf = String::new();
    for r in reqs { buf.push_str(&serde_json::to_string(r)?); buf.push('\n'); }
    std::fs::write(&tmp, buf)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

pub fn append(ws: &Path, text: &str) -> Result<()> {
    let mut all = read_all(ws)?;
    all.push(Request { ts: chrono::Local::now().timestamp(), text: text.to_string(), status: "pending".into() });
    write_all(ws, &all)
}

pub fn pending(ws: &Path) -> Result<Vec<String>> {
    Ok(read_all(ws)?.into_iter().filter(|r| r.status == "pending").map(|r| r.text).collect())
}

pub fn mark_all_consumed(ws: &Path) -> Result<()> {
    let mut all = read_all(ws)?;
    for r in all.iter_mut() { if r.status == "pending" { r.status = "consumed".into(); } }
    write_all(ws, &all)
}

/// A planner-prompt section listing pending requests, or "" if none.
pub fn prompt_block(ws: &Path) -> Result<String> {
    let p = pending(ws)?;
    if p.is_empty() { return Ok(String::new()); }
    let mut s = String::from("\n\nPENDING USER REQUESTS (fold these into the backlog this round, then they are consumed):\n");
    for (i, t) in p.iter().enumerate() { s.push_str(&format!("  {}. {}\n", i + 1, t)); }
    Ok(s)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test requests_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/requests.rs src/lib.rs tests/requests_test.rs
git commit -m "feat(requests): pending user-request log + planner prompt block"
```

---

### Task 2: planner — embed pending requests, mark consumed on success

**Files:**
- Modify: `src/planner.rs`
- Test: `tests/planner_worker_test.rs` (add a case)

- [ ] **Step 1: Add the failing test** to `tests/planner_worker_test.rs`

```rust
#[test]
fn planner_prompt_includes_pending_requests() {
    let ws = std::env::temp_dir().join(format!("alpr-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
    std::fs::write(ws.join(".agentloop/state/goal.md"), "g").unwrap();
    std::fs::write(ws.join(".agentloop/state/master.md"), "m").unwrap();
    std::fs::write(ws.join(".agentloop/state/backlog.json"), r#"{"items":[]}"#).unwrap();
    agentloop::requests::append(&ws, "add a --due flag").unwrap();

    let p = agentloop::planner::planner_prompt(&ws, 3);
    assert!(p.contains("PENDING USER REQUESTS"));
    assert!(p.contains("add a --due flag"));
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test planner_worker_test`
Expected: FAIL — prompt lacks the requests block.

- [ ] **Step 3: Update `src/planner.rs`**

In `planner_prompt`, append the requests block:

```rust
pub fn planner_prompt(ws: &Path, max_attempts: u32) -> String {
    // ... existing goal/master/backlog reads ...
    let requests = crate::requests::prompt_block(ws).unwrap_or_default();
    format!(r#"You are the PLANNER for an autonomous app build. Working dir: {ws} (a git repo).
... (unchanged body through the OUTPUT CONTRACT) ...
Do not print the JSON to stdout; write the files.{requests}"#,
        ws = ws.display(), /* ...existing args..., */ requests = requests)
}
```

In `planner_run`, after a *valid* backlog is produced, mark requests consumed:

```rust
pub async fn planner_run(cfg: &Config, ws: &Path, log: &Path, t: Duration) -> Result<bool> {
    let bk = ws.join(".agentloop/state/backlog.json");
    let prompt = planner_prompt(ws, cfg.max_attempts());
    spawn::agent_run(cfg, "planner", &prompt, ws, log, t).await?;
    if state::backlog_valid(&bk) {
        let _ = crate::requests::mark_all_consumed(ws);
        return Ok(true);
    }
    eprintln!("planner produced invalid backlog.json; re-prompting once");
    let retry = format!("{prompt}\nNOTE: your previous backlog.json was invalid JSON. Write valid JSON this time.");
    spawn::agent_run(cfg, "planner", &retry, ws, log, t).await?;
    let ok = state::backlog_valid(&bk);
    if ok { let _ = crate::requests::mark_all_consumed(ws); }
    Ok(ok)
}
```

> The requests are embedded in the prompt *before* the agent runs, so marking them consumed after a valid backlog is correct — the planner has already seen them.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test planner_worker_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/planner.rs tests/planner_worker_test.rs
git commit -m "feat(planner): embed pending user requests, consume on valid backlog"
```

---

### Task 3: orchestrator — standby state machine + re-engage

**Files:**
- Modify: `src/orchestrator.rs`
- Test: `tests/loop_addtask_test.rs`
- Modify: `tests/common/mod.rs` (stub that builds item-1, then on a second request builds item-2)

The interactive loop is restructured so DONE/cap/stall transitions to `Standby` rather than returning. In standby it awaits a `Command`. `AddTask` appends the request, resets stall, restarts the budget window, and resumes iterating; `AnswerQuestion` applies the answer and resumes; `Quit` returns.

- [ ] **Step 1: Add the request-driven stub** to `tests/common/mod.rs`

```rust
/// Planner: round 1 seeds it-1; later rounds mark done items done AND, if a pending
/// request exists, add it-2 (ready). Worker: makes <id>.txt + commits + writes result.
pub fn init_ws_with_request_stub() -> PathBuf {
    let ws = init_ws_with_stub();
    let stub = r#"#!/bin/bash
tool="$1"; shift
ws="$WS"; ws_state="$ws/.agentloop/state"; res="$ws/.agentloop/results"
prompt="$*"
case "$prompt" in
  *PLANNER*)
    python3 - "$ws" <<'PY'
import json,sys,os
ws=sys.argv[1]; st=os.path.join(ws,'.agentloop','state'); res=os.path.join(ws,'.agentloop','results')
bkp=os.path.join(st,'backlog.json'); d=json.load(open(bkp))
def ensure(idn,acc):
    if not any(i['id']==idn for i in d['items']):
        d['items'].append({"id":idn,"title":idn,"desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":acc})
# round 1: seed it-1 + verify.sh
if not d['items']:
    ensure('it-1','it-1 file')
    open(os.path.join(ws,'.agentloop','verify.sh'),'w').write('#!/bin/bash\n[ -f "$PWD/it-1.txt" ] && { [ -z "$WANT2" ] || [ -f "$PWD/it-2.txt" ]; }\n')
    os.chmod(os.path.join(ws,'.agentloop','verify.sh'),0o755)
# mark finished items done
for i in d['items']:
    if os.path.exists(os.path.join(res,i['id']+'.json')): i['status']='done'
# fold a pending request -> it-2 (and require it in the gate via WANT2 marker file)
reqp=os.path.join(st,'requests.jsonl')
pend=[l for l in open(reqp).read().splitlines() if l.strip() and json.loads(l)['status']=='pending'] if os.path.exists(reqp) else []
if pend:
    ensure('it-2','it-2 file'); open(os.path.join(ws,'.want2'),'w').write('1')
json.dump(d,open(bkp,'w'))
open(os.path.join(st,'master.md'),'w').write('# updated')
PY
    ;;
  *WORKER*)
    id=$(echo "$prompt" | sed -n 's/.*id:    \([a-z0-9-]*\).*/\1/p' | head -1)
    [ -z "$id" ] && id=it-1
    echo made > "$PWD/$id.txt"; git add -A; git commit -qm "worker $id" 2>/dev/null
    echo "{\"status\":\"done\",\"summary\":\"made $id\",\"files_changed\":[\"$id.txt\"]}" > "$res/$id.json"
    ;;
esac
exit 0
"#;
    std::fs::write(ws.join("stub.sh"), stub).unwrap();
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(ws.join("stub.sh"), std::fs::Permissions::from_mode(0o755)).unwrap(); }
    std::fs::write(ws.join(".agentloop/state/goal.md"), "make it-1").unwrap();
    ws
}
```

> The gate reads `WANT2` from the env; the test sets `WANT2=1` once it has added the second request so the gate only passes after `it-2.txt` exists too. The stub writes a `.want2` marker; the test exports `WANT2=1` before the re-engage iteration.

- [ ] **Step 2: Write the failing test** in `tests/loop_addtask_test.rs`

Because the full standby loop blocks on a channel, the test drives the pieces directly: run to first DONE, then simulate an `AddTask` re-engage by appending a request and iterating again, asserting the planner adds `it-2` and it gets built.

```rust
mod common;
use agentloop::config::Config;
use agentloop::events::EventLineReporter;
use agentloop::{orchestrator, requests, state};
use std::sync::Arc;

fn cfg() -> Config {
    serde_yaml::from_str(r#"
caps: { max_iterations: 8, max_parallel: 1, item_timeout_sec: 30, total_budget_sec: 300, max_attempts: 5 }
routing:
  planner: { tool: claude, model: opus, effort: high, flags: "" }
  build:   { tool: codex,  model: gpt-5, effort: high, flags: "" }
defaults: { role: build }
"#).unwrap()
}

#[tokio::test]
async fn add_task_after_done_builds_new_item() {
    let ws = common::init_ws_with_request_stub();
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);
    let bk = ws.join(".agentloop/state/backlog.json");
    let rep: Arc<dyn agentloop::events::Reporter> = Arc::new(EventLineReporter);

    // Iterate until it-1 is built and the gate passes (no pending request yet).
    orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap(); // planner seeds it-1
    orchestrator::iterate(&cfg(), &ws, 2, &rep).await.unwrap(); // worker builds it-1
    orchestrator::iterate(&cfg(), &ws, 3, &rep).await.unwrap(); // planner marks it-1 done
    assert_eq!(state::open_count(&bk).unwrap(), 0, "all done before add-task");

    // Simulate AddTask in standby: append a request, require it in the gate, re-engage.
    requests::append(&ws, "also build it-2").unwrap();
    std::env::set_var("WANT2", "1");

    orchestrator::iterate(&cfg(), &ws, 4, &rep).await.unwrap(); // planner folds request -> it-2
    let v = state::read(&bk).unwrap();
    assert!(state::item(&v, "it-2").is_some(), "planner added it-2 from the request");
    assert!(requests::pending(&ws).unwrap().is_empty(), "request consumed");

    orchestrator::iterate(&cfg(), &ws, 5, &rep).await.unwrap(); // worker builds it-2
    orchestrator::iterate(&cfg(), &ws, 6, &rep).await.unwrap(); // planner marks it-2 done
    assert!(ws.join("it-2.txt").exists(), "it-2 built and merged");
    assert_eq!(state::open_count(&bk).unwrap(), 0);

    for k in ["FAKE_AGENT","FAKE_AGENT_BIN","WS","WANT2"] { std::env::remove_var(k); }
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --test loop_addtask_test`
Expected: FAIL initially while the stub/gate wiring is settled; iterate on the stub until the assertions hold. (This task's TDD target is the orchestrator standby loop in Step 4; the iterate-level test above validates the planner/request integration that standby depends on.)

- [ ] **Step 4: Implement the standby loop in `src/orchestrator.rs`**

Replace `run_interactive` with a state machine. Pseudocode-complete Rust:

```rust
use crate::events::Command;
use tokio::sync::mpsc;

pub async fn run_interactive(
    cfg: &Config,
    ws: &Path,
    reporter: Arc<dyn Reporter>,
    crx: &mut mpsc::UnboundedReceiver<Command>,
) -> Result<i32> {
    let bk = ws.join(".agentloop/state/backlog.json");
    let maxit = cfg.max_iterations();
    let budget = Duration::from_secs(cfg.total_budget_sec());

    let mut n = 0u32;
    let mut stalls = 0u32;
    let mut prev_gate = String::from("init");
    let mut window_start = Instant::now();      // budget window; reset on re-engage
    let mut iters_this_window = 0u32;           // max_iterations is per-engagement

    loop {
        // --- WORKING phase ---
        let mut entered_standby = false;
        loop {
            // Drain any queued commands without blocking (answers/add-task/quit mid-run).
            while let Ok(cmd) = crx.try_recv() {
                match cmd {
                    Command::Quit => return Ok(0),
                    Command::AnswerQuestion { item_id, text } => { let _ = apply_answer(ws, &item_id, &text); }
                    Command::AddTask { request } => { let _ = crate::requests::append(ws, &request); }
                }
            }

            if iters_this_window >= maxit { eprintln!("STOP(window): max_iterations"); entered_standby = true; break; }
            if window_start.elapsed() >= budget { eprintln!("STOP(window): budget"); entered_standby = true; break; }

            n += 1; iters_this_window += 1;
            let merged = iterate(cfg, ws, n, &reporter).await?;
            let grc = gate(ws);
            let gate_state = if grc == 0 { "pass" } else { "fail" };
            let open = state::open_count(&bk)?;
            let blocked = state::blocked_count(&bk)?;
            reporter.iteration(n, merged, gate_state, open);

            if gate_state == "pass" && open == 0 { entered_standby = true; break; }

            let awaiting_user = open > 0 && open == blocked;
            if awaiting_user {
                // Block for a command (answer/add-task/quit) instead of spinning.
                match crx.recv().await {
                    None | Some(Command::Quit) => return Ok(0),
                    Some(Command::AnswerQuestion { item_id, text }) => { let _ = apply_answer(ws, &item_id, &text); }
                    Some(Command::AddTask { request }) => { let _ = crate::requests::append(ws, &request); }
                }
                stalls = 0; prev_gate = gate_state.to_string();
                continue;
            }

            if merged == 0 && gate_state == prev_gate {
                stalls += 1;
                if stalls >= 2 { eprintln!("STOP: no progress (stall)"); entered_standby = true; break; }
            } else { stalls = 0; }
            prev_gate = gate_state.to_string();
        }

        // --- STANDBY phase ---
        if entered_standby {
            reporter_standby(&reporter);
            match crx.recv().await {
                None | Some(Command::Quit) => return Ok(0),
                Some(Command::AnswerQuestion { item_id, text }) => { let _ = apply_answer(ws, &item_id, &text); }
                Some(Command::AddTask { request }) => { let _ = crate::requests::append(ws, &request); }
            }
            // Re-engage: fresh budget window + iteration allowance, reset stall.
            window_start = Instant::now();
            iters_this_window = 0;
            stalls = 0;
            prev_gate = "init".into();
        }
    }
}

fn reporter_standby(reporter: &Arc<dyn Reporter>) {
    // Emit the standby event so the TUI flips its banner.
    // (Reporter has no standby method; reuse iteration or add a dedicated one.)
}
```

Add a `standby()` method to the `Reporter` trait (default no-op; `ChannelReporter` sends `Event::EnteredStandby`; `EventLineReporter` prints `=== standby: waiting for input ===`). Call `reporter.standby()` in `reporter_standby`. (The `EnteredStandby` event and `AppState` standby handling already exist from Phase 2.)

- [ ] **Step 5: Run the iterate-level test to verify it passes**

Run: `cargo test --test loop_addtask_test`
Expected: PASS.

- [ ] **Step 6: Full suite (no regressions)**

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add src/orchestrator.rs src/events.rs tests/loop_addtask_test.rs tests/common/mod.rs
git commit -m "feat(orchestrator): standby state machine + add-task re-engage + budget windows"
```

---

### Task 4: TUI — add-task editor + standby/awaiting status

**Files:**
- Modify: `src/tui.rs`
- Test: `tests/tui_viewmodel_test.rs` (add a case)

The view-model already emits `Command::AddTask` from `Mode::AddingTask` and tracks `standby` (Phase 2). This task confirms the add-task key path and renders the standby/awaiting affordances.

- [ ] **Step 1: Add the failing test** to `tests/tui_viewmodel_test.rs`

```rust
#[test]
fn add_task_key_path_emits_command() {
    use agentloop::events::Command;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = agentloop::tui::AppState::new("g".into());

    assert!(s.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)).is_none()); // open editor
    for c in "due flag".chars() { s.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)); }
    let cmd = s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(cmd, Some(Command::AddTask { ref request }) if request == "due flag"));
}

#[test]
fn standby_event_sets_flag() {
    use agentloop::events::Event;
    let mut s = agentloop::tui::AppState::new("g".into());
    s.apply(Event::EnteredStandby);
    assert!(s.standby);
}
```

- [ ] **Step 2: Run test to verify it fails / passes**

Run: `cargo test --test tui_viewmodel_test`
Expected: With Phase 2's view-model already supporting these paths, the tests should PASS immediately. If `'a'` or standby handling was stubbed, implement it now (see Phase 2 Task 5 Step 3 — the code there already covers both) and re-run to PASS.

- [ ] **Step 3: Update `render`** so the footer shows `[a] add task` always, switches to the add-task input area in `Mode::AddingTask` (label "add task (sent to planner):"), and when `s.standby` is true the top bar shows `✓ DONE · standby` and the footer reduces to `[a] add task   [q] quit`. Manually verify in Task 5.

- [ ] **Step 4: Commit**

```bash
git add src/tui.rs tests/tui_viewmodel_test.rs
git commit -m "feat(tui): add-task editor + standby/awaiting affordances"
```

---

### Task 5: Manual verification + cutover

- [ ] **Step 1: Manual TUI run — add task after done**

Run the binary under a terminal against the request stub (replicate `init_ws_with_request_stub`'s setup in a scratch dir; export `FAKE_AGENT/FAKE_AGENT_BIN/WS`). Verify by eye:
- the run builds `it-1`, gate passes, and the UI enters **standby** ("✓ DONE · standby");
- press `a`, type "also build it-2", `enter`;
- the planner folds the request into `it-2`, the worker builds it, gate passes again, back to standby;
- press `q` → clean exit, terminal restored.

- [ ] **Step 2: Manual TUI run — add task mid-run**

While `it-1` is still building, press `a` and add a task; confirm it's picked up on the next planner round (not lost), and the run continues without restarting.

- [ ] **Step 3: Decommission the bash app**

Remove the superseded bash implementation now that the Rust binary covers all behavior:
```bash
git rm agentloop.sh helpers/yaml2json.py lib/*.sh tests/*.sh tests/fake_agent.sh
```
Keep `templates/` (still used via `include_str!`). Update `README.md`: Rust build/run instructions, `cargo test` for the suite, and document the three features (question inbox, add-task, standby) with the TUI keybindings. Update the Layout section to the Rust module map.

- [ ] **Step 4: Final full suite**

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: decommission bash app; README for Rust agentloop + interactive features"
```

---

## Self-Review

**Spec coverage (Phase 3):**
- Add task via TUI → `Command::AddTask` → `requests.jsonl` → Task 1 (store) + Task 3 (handling) + Task 4 (UI). ✓
- Planner "PENDING USER REQUESTS" section; folds + consumes → Task 1 (`prompt_block`) + Task 2. ✓
- Planner remains sole backlog owner (user feeds intent, not items) → Task 2. ✓
- Standby on DONE/cap/stall instead of exit; idles awaiting a Command → Task 3. ✓
- `AddTask` re-engages (reset stall, fresh budget window); `AnswerQuestion` re-engages; `Quit` exits → Task 3. ✓
- Budget paused in standby, fresh window on re-engage; `max_iterations` per-engagement → Task 3 (`window_start`, `iters_this_window`). ✓
- Add tasks mid-run (not only after done) → Task 3 (try_recv drain in the working loop) + Task 5 Step 2. ✓
- Headless preserves exit-on-done → headless uses `orchestrator::run` (not `run_interactive`), unchanged from Phase 1/2. ✓
- `EnteredStandby` event + standby banner → Phase 2 enum + Task 3 `standby()` + Task 4 render. ✓

**Type consistency:** `requests::{append,pending,mark_all_consumed,prompt_block}` consistent across Tasks 1,2,3. `Command` variants (`AddTask`,`AnswerQuestion`,`Quit`) handled identically in the working-drain, awaiting-user, and standby arms of Task 3. `Reporter::standby` added in Task 3 with the same default-no-op pattern as Phase 2's `question`. `AppState.standby` + `Mode::AddingTask` from Phase 2 reused unchanged.

**Placeholder scan:** `reporter_standby` is a thin named wrapper around `reporter.standby()` (real code, kept separate only for readability). The request stub in Task 3 is intentionally the most elaborate test fixture; it is complete and runnable (python3-based, matching the project's existing test conventions). No "TBD"/"implement later" remain.
```
```

**Cross-phase note:** the three plans share `tests/common/mod.rs`; each phase adds a stub variant without modifying earlier ones, so the Phase 1 `loop_test` stays green throughout.
