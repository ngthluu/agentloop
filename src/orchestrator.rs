use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::events::{Command, Reporter};
use crate::worktree::MergeOutcome;
use crate::{planner, spawn, state, worker, worktree};

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
    branch: &str,
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
    if let Err(e) = spawn::agent_run(cfg, "resolver", &prompt, ws, &log, NO_TIMEOUT).await {
        eprintln!("resolver spawn error for {id}: {e:#}");
    }

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
    // Guard against a resolver that aborted instead of completing the merge: the
    // branch's commits must now be contained in HEAD.
    if worktree::has_commits_ahead(ws, branch) {
        reporter.status(&rid, "failed", &tool, &model);
        return Ok(false);
    }
    reporter.status(&rid, "merged", &tool, &model);
    Ok(true)
}

/// Run verify.sh; capture output to last_gate.txt; return its exit code (1 if absent).
pub fn gate(ws: &Path) -> i32 {
    let gate = ws.join(".agentloop/verify.sh");
    let out = ws.join(".agentloop/state/last_gate.txt");
    if gate.exists() {
        let result = std::process::Command::new("/bin/bash")
            .arg(&gate)
            .current_dir(ws)
            .output();
        match result {
            Ok(o) => {
                let mut buf = o.stdout.clone();
                buf.extend_from_slice(&o.stderr);
                let _ = std::fs::write(&out, &buf);
                o.status.code().unwrap_or(1)
            }
            Err(_) => {
                let _ = std::fs::write(&out, "verify.sh spawn failed");
                1
            }
        }
    } else {
        let _ = std::fs::write(&out, "no verify.sh yet");
        1
    }
}

/// One iteration: plan, select, dispatch in parallel, integrate. Returns merged count.
pub async fn iterate(cfg: &Config, ws: &Path, n: u32, reporter: &Arc<dyn Reporter>) -> Result<u32> {
    let sdir = ws.join(".agentloop/state");
    let ldir = ws.join(format!(".agentloop/logs/iter-{n}"));
    std::fs::create_dir_all(&ldir)?;
    std::fs::create_dir_all(ws.join(".agentloop/results"))?;
    let bk = sdir.join("backlog.json");
    let itimeout = Duration::from_secs(cfg.item_timeout_sec());
    let maxpar = cfg.max_parallel() as usize;
    let maxatt = cfg.max_attempts();

    // Planner (tracked, awaited).
    let prole = cfg.resolve_role("planner").unwrap_or_default();
    let ptool = cfg.role_field(&prole, "tool").unwrap_or_default();
    let pmodel = cfg.role_field(&prole, "model").unwrap_or_default();
    reporter.dispatch("planner", "planning", &ptool, &pmodel, Some(&ldir.join("planner.log")));
    let ok = planner::planner_run(cfg, ws, &ldir.join("planner.log"), itimeout).await?;
    if !ok {
        eprintln!("planner failed/invalid");
        anyhow::bail!("planner invalid");
    }
    reporter.status("planner", "done", &ptool, &pmodel);

    let ready = state::ready_items(&bk, ws, maxpar)?;
    if ready.is_empty() {
        return Ok(0);
    }

    // Dispatch each ready item in its own worktree, concurrently.
    let mut handles = Vec::new();
    let mut dispatched: Vec<String> = Vec::new();
    for id in ready {
        let v = state::read(&bk)?;
        let item = match state::item(&v, &id) {
            Some(i) => i.clone(),
            None => continue,
        };
        let att = item
            .get("attempts")
            .and_then(|a| a.as_u64())
            .unwrap_or(0) as u32;
        if att >= maxatt {
            state::set_status(
                &bk,
                &id,
                "failed",
                &format!("exceeded max_attempts ({maxatt})"),
            )?;
            continue;
        }
        let wt = ws.join(format!(".agentloop/worktrees/{id}"));
        let _ = std::fs::remove_dir_all(&wt);
        worktree::remove(ws, &wt, &format!("item/{id}"));
        if worktree::create(ws, &format!("item/{id}"), &wt).is_err() {
            state::set_status(&bk, &id, "failed", "worktree create failed")?;
            continue;
        }
        state::set_status(&bk, &id, "in_progress", "")?;
        state::increment_attempts(&bk, &id)?;

        let role = item["role"].as_str().unwrap_or("build").to_string();
        let rrole = cfg.resolve_role(&role).unwrap_or_default();
        let tool = cfg.role_field(&rrole, "tool").unwrap_or_default();
        let model = cfg.role_field(&rrole, "model").unwrap_or_default();
        let label = item["title"].as_str().unwrap_or("").to_string();
        let log = ldir.join(format!("item-{id}.log"));
        reporter.dispatch(&id, &label, &tool, &model, Some(&log));

        let cfg2 = cfg.clone();
        let ws2 = ws.to_path_buf();
        let item2: Value = item.clone();
        let id2 = id.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = worker::worker_dispatch(&cfg2, &ws2, &item2, &wt, &log, itimeout).await {
                eprintln!("worker {id2} dispatch error: {e:#}");
            }
            id2
        }));
        dispatched.push(id);
    }

    // Await all workers. A panicked worker task surfaces here; its missing result
    // file then bounces the item back to ready during integration.
    for h in handles {
        if let Err(e) = h.await {
            eprintln!("worker task panicked: {e}");
        }
    }

    // Integrate sequentially based on each worker's result file.
    let mut merged = 0u32;
    for id in &dispatched {
        let rfile = ws.join(format!(".agentloop/results/{id}.json"));
        let result_value: Option<Value> = std::fs::read_to_string(&rfile)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let status = result_value.as_ref().map(|v| v["status"].clone()).unwrap_or(Value::Null);
        let branch = format!("item/{id}");

        if status == "needs_input" {
            // Agent is blocked on a user decision. Surface the question; block the
            // item; do not merge. The question file lives in the main workspace
            // (.agentloop/questions/<id>.json), so it survives worktree removal.
            if let Ok(q) = crate::inbox::read_question(ws, id) {
                let label = state::read(&bk)
                    .ok()
                    .as_ref()
                    .and_then(|v| state::item(v, id))
                    .and_then(|i| i["title"].as_str().map(String::from))
                    .unwrap_or_default();
                reporter.question(id, &label, &q.question, &q.context);
                state::set_status(&bk, id, "blocked", &q.question)?;
            } else {
                // Malformed/missing question file: treat as a normal non-done bounce.
                state::set_status(&bk, id, "ready", "needs_input without a question file")?;
                reporter.status(id, "bounced", "", "");
            }
            worktree::remove(ws, &ws.join(format!(".agentloop/worktrees/{id}")), &branch);
            let _ = std::fs::remove_file(&rfile);
            continue;
        }

        let result_done = status == "done";
        if result_done {
            if !worktree::has_commits_ahead(ws, &branch) {
                state::set_status(&bk, id, "ready", "worker reported done but made no commits")?;
                reporter.status(id, "bounced", "", "");
            } else {
                match worktree::merge_or_conflict(ws, &branch)? {
                    MergeOutcome::Merged => {
                        state::set_status(&bk, id, "done", "")?;
                        reporter.status(id, "merged", "", "");
                        merged += 1;
                    }
                    MergeOutcome::Conflict => {
                        let (label, item_v) = state::read(&bk)
                            .ok()
                            .map(|v| {
                                let it = state::item(&v, id).cloned();
                                let label = it
                                    .as_ref()
                                    .and_then(|i| i["title"].as_str().map(String::from))
                                    .unwrap_or_default();
                                (label, it.unwrap_or(Value::Null))
                            })
                            .unwrap_or_default();
                        if resolve_conflict(cfg, ws, id, &label, &item_v, &branch, n, reporter).await? {
                            state::set_status(&bk, id, "done", "")?;
                            reporter.status(id, "merged", "", "");
                            merged += 1;
                        } else {
                            // The resolver itself is unbounded (it never consumes an
                            // attempt). But a failed resolve bounces the item back to
                            // `ready`, so the next WORKER re-dispatch consumes an attempt
                            // — that bound is deliberate, so a perpetually-conflicting
                            // item can't loop forever.
                            worktree::abort_merge(ws);
                            state::set_status(&bk, id, "ready", "merge conflict; resolver failed")?;
                            reporter.status(id, "bounced", "", "");
                        }
                    }
                }
            }
        } else {
            state::set_status(&bk, id, "ready", "worker did not report done")?;
            reporter.status(id, "failed", "", "");
        }
        worktree::remove(ws, &ws.join(format!(".agentloop/worktrees/{id}")), &branch);
        let _ = std::fs::remove_file(&rfile);
    }
    Ok(merged)
}

/// Apply a user's answer to a blocked item: persist it, consume the question,
/// flip the item blocked->ready so it is re-dispatched with the prior Q&A.
pub fn apply_answer(ws: &Path, item_id: &str, text: &str) -> Result<()> {
    let bk = ws.join(".agentloop/state/backlog.json");
    // The question text: prefer the live question file; fall back to the item's notes
    // (where the needs_input handler stashed it) if the file was already consumed.
    let question = match crate::inbox::read_question(ws, item_id) {
        Ok(q) => q.question,
        Err(_) => {
            let v = state::read(&bk)?;
            state::item(&v, item_id)
                .and_then(|i| i["notes"].as_str().map(String::from))
                .unwrap_or_default()
        }
    };
    crate::inbox::record_answer(ws, item_id, &question, text)?;
    let _ = crate::inbox::consume_question(ws, item_id);
    state::set_status(&bk, item_id, "ready", "answered; re-dispatching")?;
    Ok(())
}

/// Drive iterations until DONE (0), cap/stall (1), or hard error (Err).
pub async fn run(cfg: &Config, ws: &Path, reporter: Arc<dyn Reporter>) -> Result<i32> {
    let sdir = ws.join(".agentloop/state");
    let bk = sdir.join("backlog.json");
    let maxit = cfg.max_iterations();
    let budget = Duration::from_secs(cfg.total_budget_sec());
    let start = Instant::now();
    let (mut n, mut stalls) = (0u32, 0u32);
    let mut prev_gate = String::from("init");

    while n < maxit {
        n += 1;
        if start.elapsed() >= budget {
            eprintln!("STOP: time budget exceeded");
            return Ok(1);
        }

        let merged = iterate(cfg, ws, n, &reporter).await?;

        let grc = gate(ws);
        let gate_state = if grc == 0 { "pass" } else { "fail" };
        let open = state::open_count(&bk)?;
        reporter.iteration(n, merged, gate_state, open);

        if gate_state == "pass" && open == 0 {
            eprintln!("DONE");
            return Ok(0);
        }

        // Only a genuine user-question block (not a planner dependency-block, which
        // ready_items now dispatches autonomously) should halt a headless run.
        let user_blocked = state::user_blocked_count(&bk, ws)?;
        if open > 0 && open == user_blocked {
            eprintln!("STOP: blocked on user input (headless)");
            return Ok(1);
        }

        if merged == 0 && gate_state == prev_gate {
            stalls += 1;
            if stalls >= 2 {
                eprintln!("STOP: no progress for 2 stalls (3 consecutive iterations)");
                return Ok(1);
            }
        } else {
            stalls = 0;
        }
        prev_gate = gate_state.to_string();
    }
    eprintln!("STOP: max_iterations reached");
    Ok(1)
}

/// Interactive driver with a standby state machine. DONE/cap/stall transitions to
/// standby (idle, awaiting a command) instead of exiting. AddTask / AnswerQuestion
/// re-engage with a fresh budget window; Quit exits. Tasks can also be added mid-run.
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
    let mut window_start = Instant::now();   // budget window; reset on re-engage
    let mut iters_this_window = 0u32;        // max_iterations is per-engagement

    // Wait for the TUI to commit a goal (the goal-entry screen) before doing any work.
    // Nothing — no planner, no workers — runs until StartRun arrives.
    loop {
        match crx.recv().await {
            None | Some(Command::Quit) => return Ok(0),
            Some(Command::StartRun { goal }) => {
                if let Err(e) = crate::cli::commit_goal(ws, &goal) {
                    eprintln!("failed to commit goal: {e:#}");
                }
                break;
            }
            // Stray answer/add-task before the run starts: ignore.
            Some(Command::AnswerQuestion { .. }) | Some(Command::AddTask { .. }) => {}
        }
    }

    'outer: loop {
        // --- WORKING phase ---
        let go_standby = 'working: loop {
            // Drain any queued commands without blocking (mid-run answers/add-task/quit).
            while let Ok(cmd) = crx.try_recv() {
                match cmd {
                    Command::Quit => return Ok(0),
                    Command::AnswerQuestion { item_id, text } => { let _ = apply_answer(ws, &item_id, &text); }
                    Command::AddTask { request } => { let _ = crate::requests::append(ws, &request); }
                    Command::StartRun { .. } => {}
                }
            }

            if iters_this_window >= maxit { eprintln!("STOP(window): max_iterations"); break 'working true; }
            if window_start.elapsed() >= budget { eprintln!("STOP(window): budget"); break 'working true; }

            n += 1;
            iters_this_window += 1;
            let merged = iterate(cfg, ws, n, &reporter).await?;
            let grc = gate(ws);
            let gate_state = if grc == 0 { "pass" } else { "fail" };
            let open = state::open_count(&bk)?;
            let user_blocked = state::user_blocked_count(&bk, ws)?;
            reporter.iteration(n, merged, gate_state, open);

            if gate_state == "pass" && open == 0 { break 'working true; }

            // Only user-question blocks remain (dependency-blocks are dispatched by
            // ready_items): block for a command (answer/add-task/quit).
            if open > 0 && open == user_blocked {
                match crx.recv().await {
                    None | Some(Command::Quit) => return Ok(0),
                    Some(Command::AnswerQuestion { item_id, text }) => { let _ = apply_answer(ws, &item_id, &text); }
                    Some(Command::AddTask { request }) => { let _ = crate::requests::append(ws, &request); }
                    Some(Command::StartRun { .. }) => {}
                }
                stalls = 0;
                prev_gate = gate_state.to_string();
                continue 'working;
            }

            if merged == 0 && gate_state == prev_gate {
                stalls += 1;
                if stalls >= 2 { eprintln!("STOP: no progress (stall)"); break 'working true; }
            } else { stalls = 0; }
            prev_gate = gate_state.to_string();
        };

        // --- STANDBY phase ---
        if go_standby {
            reporter.standby();
            match crx.recv().await {
                None | Some(Command::Quit) => return Ok(0),
                Some(Command::AnswerQuestion { item_id, text }) => { let _ = apply_answer(ws, &item_id, &text); }
                Some(Command::AddTask { request }) => { let _ = crate::requests::append(ws, &request); }
                Some(Command::StartRun { .. }) => {}
            }
            // Re-engage: fresh budget window + iteration allowance, reset stall.
            window_start = Instant::now();
            iters_this_window = 0;
            stalls = 0;
            prev_gate = "init".into();
        }
        continue 'outer;
    }
}
