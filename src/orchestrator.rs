use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::events::Reporter;
use crate::{planner, state, worker, worktree};

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
    reporter.dispatch("planner", "planning", &ptool, &pmodel);
    let ok = planner::planner_run(cfg, ws, &ldir.join("planner.log"), itimeout).await?;
    if !ok {
        eprintln!("planner failed/invalid");
        anyhow::bail!("planner invalid");
    }
    reporter.status("planner", "done", &ptool, &pmodel);

    let ready = state::ready_items(&bk, maxpar)?;
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
        reporter.dispatch(&id, &label, &tool, &model);

        let cfg2 = cfg.clone();
        let ws2 = ws.to_path_buf();
        let log = ldir.join(format!("item-{id}.log"));
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
        let result_done = std::fs::read_to_string(&rfile)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| v["status"] == "done")
            .unwrap_or(false);
        let branch = format!("item/{id}");

        if result_done {
            if !worktree::has_commits_ahead(ws, &branch) {
                state::set_status(&bk, id, "ready", "worker reported done but made no commits")?;
                reporter.status(id, "bounced", "", "");
            } else if worktree::merge(ws, &branch)? {
                state::set_status(&bk, id, "done", "")?;
                reporter.status(id, "merged", "", "");
                merged += 1;
            } else {
                state::set_status(&bk, id, "ready", "merge conflict; replan")?;
                reporter.status(id, "bounced", "", "");
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
