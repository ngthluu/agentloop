use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::events::{Command, Reporter};
use crate::worktree::MergeOutcome;
use crate::{architect, customer, manager, spawn, state, task_state, worker, worktree};

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
    reporter.dispatch(
        &rid,
        &format!("resolve merge conflict — {label}"),
        &tool,
        &model,
        Some(&log),
    );

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

fn builder_owner(ws: &Path, builder_id: &str) -> Result<Option<String>> {
    let tasks_dir = ws.join(".agentloop/state/tasks");
    let Ok(entries) = std::fs::read_dir(&tasks_dir) else {
        return Ok(None);
    };

    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let task_id = entry.file_name().to_string_lossy().to_string();
        let Ok(builders) = task_state::read_builders(ws, &task_id) else {
            continue;
        };
        if task_state::item(&builders, builder_id).is_some() {
            return Ok(Some(task_id));
        }
    }
    Ok(None)
}

fn builder_item(ws: &Path, task_id: &str, builder_id: &str) -> Result<Option<Value>> {
    let builders = task_state::read_builders(ws, task_id)?;
    Ok(task_state::item(&builders, builder_id).cloned())
}

fn active_business_ids(bk: &Path, ws: &Path) -> Result<Vec<String>> {
    let backlog = state::read(bk)?;
    let ready: std::collections::HashSet<String> = state::ready_items(bk, ws, usize::MAX)?
        .into_iter()
        .collect();
    let empty = vec![];
    Ok(backlog["items"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|item| {
            let id = item["id"].as_str()?;
            match item["status"].as_str() {
                Some("in_progress") => Some(id.to_string()),
                Some("ready") if ready.contains(id) => Some(id.to_string()),
                _ => None,
            }
        })
        .collect())
}

fn customer_feedback(ws: &Path, task_id: &str) -> String {
    std::fs::read_to_string(task_state::customer_path(ws, task_id))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| {
            v.get("acceptance_notes")
                .or_else(|| v.get("summary"))
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "customer rejected the completed task".to_string())
}

fn task_blocked_on_builder_question(ws: &Path, task_id: &str) -> Result<bool> {
    let builders = match task_state::read_builders(ws, task_id) {
        Ok(builders) => builders,
        Err(_) => return Ok(false),
    };
    let empty = vec![];
    let has_question = builders["items"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|item| item["status"] == "blocked")
        .filter_map(|item| item["id"].as_str())
        .any(|id| crate::inbox::has_question(ws, id));
    Ok(has_question && task_state::ready_builders(ws, task_id, 1)?.is_empty())
}

fn user_blocked_business_count(bk: &Path, ws: &Path) -> Result<i64> {
    let backlog = state::read(bk)?;
    let empty = vec![];
    let mut count = 0i64;
    for item in backlog["items"].as_array().unwrap_or(&empty) {
        if !matches!(
            item["status"].as_str(),
            Some("ready") | Some("in_progress") | Some("blocked")
        ) {
            continue;
        }
        let Some(id) = item["id"].as_str() else {
            continue;
        };
        if item["status"] == "blocked" && crate::inbox::has_question(ws, id) {
            count += 1;
        } else if task_blocked_on_builder_question(ws, id)? {
            count += 1;
        }
    }
    Ok(count)
}

/// One iteration: manage, architect, dispatch builders, integrate, review. Returns merged count.
pub async fn iterate(cfg: &Config, ws: &Path, n: u32, reporter: &Arc<dyn Reporter>) -> Result<u32> {
    let sdir = ws.join(".agentloop/state");
    let ldir = ws.join(format!(".agentloop/logs/iter-{n}"));
    std::fs::create_dir_all(&ldir)?;
    std::fs::create_dir_all(ws.join(".agentloop/results"))?;
    let bk = sdir.join("backlog.json");
    let itimeout = Duration::from_secs(cfg.item_timeout_sec());
    let maxpar = cfg.max_parallel() as usize;
    let maxatt = cfg.max_attempts();

    let mrole = cfg.resolve_role("manager").unwrap_or_default();
    let mtool = cfg.role_field(&mrole, "tool").unwrap_or_default();
    let mmodel = cfg.role_field(&mrole, "model").unwrap_or_default();
    reporter.dispatch(
        "manager",
        "managing",
        &mtool,
        &mmodel,
        Some(&ldir.join("manager.log")),
    );
    let ok = manager::manager_run(cfg, ws, &ldir.join("manager.log"), itimeout).await?;
    if !ok {
        eprintln!("manager failed/invalid");
        anyhow::bail!("manager invalid");
    }
    reporter.status("manager", "done", &mtool, &mmodel);

    let ready_business = state::ready_items(&bk, ws, usize::MAX)?;
    for id in &ready_business {
        if task_state::builder_plan_valid(ws, id) {
            continue;
        }
        let backlog = state::read(&bk)?;
        let Some(task) = state::item(&backlog, id).cloned() else {
            continue;
        };

        let arole = cfg.resolve_role("architect").unwrap_or_default();
        let atool = cfg.role_field(&arole, "tool").unwrap_or_default();
        let amodel = cfg.role_field(&arole, "model").unwrap_or_default();
        let aid = format!("architect-{id}");
        let label = format!("architect {}", task["title"].as_str().unwrap_or(""));
        let log = ldir.join(format!("{aid}.log"));
        reporter.dispatch(&aid, &label, &atool, &amodel, Some(&log));
        let ok = architect::architect_run(cfg, ws, &task, &log, itimeout).await?;
        if ok && task_state::builder_plan_valid(ws, id) {
            state::set_status(&bk, id, "in_progress", "")?;
            reporter.status(&aid, "done", &atool, &amodel);
        } else {
            state::set_status(&bk, id, "ready", "architect produced invalid task plan")?;
            reporter.status(&aid, "failed", &atool, &amodel);
        }
    }

    let mut handles = Vec::new();
    let mut dispatched: Vec<(String, String)> = Vec::new();
    let active = active_business_ids(&bk, ws)?;
    for task_id in &active {
        if !task_state::builder_plan_valid(ws, task_id) {
            continue;
        }
        state::set_status(&bk, task_id, "in_progress", "")?;
        let remaining = maxpar.saturating_sub(dispatched.len());
        if remaining == 0 {
            break;
        }
        for id in task_state::ready_builders(ws, task_id, remaining)? {
            let Some(item) = builder_item(ws, task_id, &id)? else {
                continue;
            };
            let att = item.get("attempts").and_then(|a| a.as_u64()).unwrap_or(0) as u32;
            if att >= maxatt {
                task_state::set_builder_status(
                    ws,
                    task_id,
                    &id,
                    "failed",
                    &format!("exceeded max_attempts ({maxatt})"),
                )?;
                continue;
            }
            let backlog = state::read(&bk)?;
            let Some(parent) = state::item(&backlog, task_id).cloned() else {
                continue;
            };
            let wt = ws.join(format!(".agentloop/worktrees/{id}"));
            let _ = std::fs::remove_dir_all(&wt);
            worktree::remove(ws, &wt, &format!("item/{id}"));
            if worktree::create(ws, &format!("item/{id}"), &wt).is_err() {
                task_state::set_builder_status(
                    ws,
                    task_id,
                    &id,
                    "failed",
                    "worktree create failed",
                )?;
                continue;
            }
            task_state::set_builder_status(ws, task_id, &id, "in_progress", "")?;
            task_state::increment_builder_attempts(ws, task_id, &id)?;

            let rrole = cfg.resolve_role("builder").unwrap_or_default();
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
                if let Err(e) =
                    worker::builder_dispatch(&cfg2, &ws2, &parent, &item2, &wt, &log, itimeout)
                        .await
                {
                    eprintln!("builder {id2} dispatch error: {e:#}");
                }
                id2
            }));
            dispatched.push((task_id.clone(), id));
        }
    }

    // Await all builders. A panicked builder task surfaces here; its missing result
    // file then bounces the item back to ready during integration.
    for h in handles {
        if let Err(e) = h.await {
            eprintln!("builder task panicked: {e}");
        }
    }

    // Integrate sequentially based on each builder's result file.
    let mut merged = 0u32;
    for (task_id, id) in &dispatched {
        let rfile = ws.join(format!(".agentloop/results/{id}.json"));
        let result_value: Option<Value> = std::fs::read_to_string(&rfile)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let status = result_value
            .as_ref()
            .map(|v| v["status"].clone())
            .unwrap_or(Value::Null);
        let branch = format!("item/{id}");

        if status == "needs_input" {
            // Agent is blocked on a user decision. Surface the question; block the
            // item; do not merge. The question file lives in the main workspace
            // (.agentloop/questions/<id>.json), so it survives worktree removal.
            if let Ok(q) = crate::inbox::read_question(ws, id) {
                let label = builder_item(ws, task_id, id)
                    .ok()
                    .flatten()
                    .as_ref()
                    .and_then(|i| i["title"].as_str().map(String::from))
                    .unwrap_or_default();
                reporter.question(id, &label, &q.question, &q.context);
                task_state::set_builder_status(ws, task_id, id, "blocked", &q.question)?;
            } else {
                // Malformed/missing question file: treat as a normal non-done bounce.
                task_state::set_builder_status(
                    ws,
                    task_id,
                    id,
                    "ready",
                    "needs_input without a question file",
                )?;
                reporter.status(id, "bounced", "", "");
            }
            worktree::remove(ws, &ws.join(format!(".agentloop/worktrees/{id}")), &branch);
            let _ = std::fs::remove_file(&rfile);
            continue;
        }

        let result_done = status == "done";
        if result_done {
            if !worktree::has_commits_ahead(ws, &branch) {
                task_state::set_builder_status(
                    ws,
                    task_id,
                    id,
                    "ready",
                    "builder reported done but made no commits",
                )?;
                reporter.status(id, "bounced", "", "");
            } else {
                match worktree::merge_or_conflict(ws, &branch)? {
                    MergeOutcome::Merged => {
                        task_state::set_builder_status(ws, task_id, id, "done", "")?;
                        reporter.status(id, "merged", "", "");
                        merged += 1;
                    }
                    MergeOutcome::Conflict => {
                        let item_v = builder_item(ws, task_id, id)?.unwrap_or(Value::Null);
                        let label = item_v["title"].as_str().unwrap_or("").to_string();
                        if resolve_conflict(cfg, ws, id, &label, &item_v, &branch, n, reporter)
                            .await?
                        {
                            task_state::set_builder_status(ws, task_id, id, "done", "")?;
                            reporter.status(id, "merged", "", "");
                            merged += 1;
                        } else {
                            // The resolver itself is unbounded (it never consumes an
                            // attempt). But a failed resolve bounces the item back to
                            // `ready`, so the next builder re-dispatch consumes an attempt
                            // — that bound is deliberate, so a perpetually-conflicting
                            // item can't loop forever.
                            worktree::abort_merge(ws);
                            task_state::set_builder_status(
                                ws,
                                task_id,
                                id,
                                "ready",
                                "merge conflict; resolver failed",
                            )?;
                            reporter.status(id, "bounced", "", "");
                        }
                    }
                }
            }
        } else {
            task_state::set_builder_status(
                ws,
                task_id,
                id,
                "ready",
                "builder did not report done",
            )?;
            reporter.status(id, "failed", "", "");
        }
        worktree::remove(ws, &ws.join(format!(".agentloop/worktrees/{id}")), &branch);
        let _ = std::fs::remove_file(&rfile);
    }

    for task_id in active_business_ids(&bk, ws)? {
        if !task_state::builder_plan_valid(ws, &task_id)
            || !task_state::all_builders_done(ws, &task_id)?
        {
            continue;
        }
        if gate(ws) != 0 {
            continue;
        }
        let backlog = state::read(&bk)?;
        let Some(task) = state::item(&backlog, &task_id).cloned() else {
            continue;
        };
        let crole = cfg.resolve_role("customer").unwrap_or_default();
        let ctool = cfg.role_field(&crole, "tool").unwrap_or_default();
        let cmodel = cfg.role_field(&crole, "model").unwrap_or_default();
        let cid = format!("{task_id}-customer");
        let log = ldir.join(format!("{cid}.log"));
        reporter.dispatch(&cid, "customer review", &ctool, &cmodel, Some(&log));
        if customer::customer_run(cfg, ws, &task, &log, itimeout).await? {
            state::set_status(&bk, &task_id, "done", "")?;
            reporter.status(&cid, "approved", &ctool, &cmodel);
        } else {
            let feedback = customer_feedback(ws, &task_id);
            state::set_status(&bk, &task_id, "ready", &feedback)?;
            reporter.status(&cid, "rejected", &ctool, &cmodel);
        }
    }

    Ok(merged)
}

/// Apply a user's answer to a blocked item: persist it, consume the question,
/// flip the item blocked->ready so it is re-dispatched with the prior Q&A.
pub fn apply_answer(ws: &Path, item_id: &str, text: &str) -> Result<()> {
    let bk = ws.join(".agentloop/state/backlog.json");
    if let Some(task_id) = builder_owner(ws, item_id)? {
        let question = match crate::inbox::read_question(ws, item_id) {
            Ok(q) => q.question,
            Err(_) => builder_item(ws, &task_id, item_id)?
                .and_then(|i| i["notes"].as_str().map(String::from))
                .unwrap_or_default(),
        };
        crate::inbox::record_answer(ws, item_id, &question, text)?;
        let _ = crate::inbox::consume_question(ws, item_id);
        task_state::set_builder_status(ws, &task_id, item_id, "ready", "answered; re-dispatching")?;
        return Ok(());
    }

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

        // Only a genuine user-question block (not a manager dependency-block, which
        // ready_items now dispatches autonomously) should halt a headless run.
        let user_blocked = user_blocked_business_count(&bk, ws)?;
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
    let mut window_start = Instant::now(); // budget window; reset on re-engage
    let mut iters_this_window = 0u32; // max_iterations is per-engagement

    // Wait for the TUI to commit a goal (the goal-entry screen) before doing any work.
    // Nothing — no manager, no builders — runs until StartRun arrives.
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
                    Command::AnswerQuestion { item_id, text } => {
                        let _ = apply_answer(ws, &item_id, &text);
                    }
                    Command::AddTask { request } => {
                        let _ = crate::requests::append(ws, &request);
                    }
                    Command::StartRun { .. } => {}
                }
            }

            if iters_this_window >= maxit {
                eprintln!("STOP(window): max_iterations");
                break 'working true;
            }
            if window_start.elapsed() >= budget {
                eprintln!("STOP(window): budget");
                break 'working true;
            }

            n += 1;
            iters_this_window += 1;
            let merged = iterate(cfg, ws, n, &reporter).await?;
            let grc = gate(ws);
            let gate_state = if grc == 0 { "pass" } else { "fail" };
            let open = state::open_count(&bk)?;
            let user_blocked = user_blocked_business_count(&bk, ws)?;
            reporter.iteration(n, merged, gate_state, open);

            if gate_state == "pass" && open == 0 {
                break 'working true;
            }

            // Only user-question blocks remain (dependency-blocks are dispatched by
            // ready_items): block for a command (answer/add-task/quit).
            if open > 0 && open == user_blocked {
                match crx.recv().await {
                    None | Some(Command::Quit) => return Ok(0),
                    Some(Command::AnswerQuestion { item_id, text }) => {
                        let _ = apply_answer(ws, &item_id, &text);
                    }
                    Some(Command::AddTask { request }) => {
                        let _ = crate::requests::append(ws, &request);
                    }
                    Some(Command::StartRun { .. }) => {}
                }
                stalls = 0;
                prev_gate = gate_state.to_string();
                continue 'working;
            }

            if merged == 0 && gate_state == prev_gate {
                stalls += 1;
                if stalls >= 2 {
                    eprintln!("STOP: no progress (stall)");
                    break 'working true;
                }
            } else {
                stalls = 0;
            }
            prev_gate = gate_state.to_string();
        };

        // --- STANDBY phase ---
        if go_standby {
            reporter.standby();
            match crx.recv().await {
                None | Some(Command::Quit) => return Ok(0),
                Some(Command::AnswerQuestion { item_id, text }) => {
                    let _ = apply_answer(ws, &item_id, &text);
                }
                Some(Command::AddTask { request }) => {
                    let _ = crate::requests::append(ws, &request);
                }
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
