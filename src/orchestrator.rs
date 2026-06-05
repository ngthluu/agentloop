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

/// Canned reply for any question an agent raises: the loop is autonomous, so the
/// "user" always delegates the decision back to the agent.
pub const AUTO_ANSWER: &str = "Decide the best option for me — you decide. Pick whatever best serves the goal and the acceptance criteria, record your decision in the result summary, and continue.";

/// Auto-answer a raised question with [`AUTO_ANSWER`]: persist the Q&A, consume the
/// question file, and flip the asking item ready so it is re-dispatched with the
/// prior Q&A appended to its prompt.
pub fn auto_answer(ws: &Path, item_id: &str) -> Result<()> {
    let bk = ws.join(".agentloop/state/backlog.json");
    if let Some(task_id) = builder_owner(ws, item_id)? {
        let question = match crate::inbox::read_question(ws, item_id) {
            Ok(q) => q.question,
            Err(_) => builder_item(ws, &task_id, item_id)?
                .and_then(|i| i["notes"].as_str().map(String::from))
                .unwrap_or_default(),
        };
        crate::inbox::record_answer(ws, item_id, &question, AUTO_ANSWER)?;
        let _ = crate::inbox::consume_question(ws, item_id);
        task_state::set_builder_status(
            ws,
            &task_id,
            item_id,
            "ready",
            "auto-answered; re-dispatching",
        )?;
        return Ok(());
    }

    let question = match crate::inbox::read_question(ws, item_id) {
        Ok(q) => q.question,
        Err(_) => {
            let v = state::read(&bk)?;
            state::item(&v, item_id)
                .and_then(|i| i["notes"].as_str().map(String::from))
                .unwrap_or_default()
        }
    };
    crate::inbox::record_answer(ws, item_id, &question, AUTO_ANSWER)?;
    let _ = crate::inbox::consume_question(ws, item_id);
    state::set_status(&bk, item_id, "ready", "auto-answered; re-dispatching")?;
    Ok(())
}

/// Auto-answer every outstanding question file (e.g. left by an interrupted older
/// run) so the asking items re-enter the dispatchable set this round.
fn auto_answer_pending(ws: &Path) {
    let Ok(entries) = std::fs::read_dir(ws.join(".agentloop/questions")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if let Err(e) = auto_answer(ws, id) {
            eprintln!("auto-answer failed for {id}: {e:#}");
        }
    }
}

/// Spawn an unbounded resolver agent in the main workspace to resolve an in-progress
/// merge conflict for `id`, then complete the merge. Returns true if the merge is
/// resolved and committed.
#[allow(clippy::too_many_arguments)]
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
        reporter.status(
            &rid,
            "failed",
            &tool,
            &model,
            "resolver left unmerged paths",
        );
        return Ok(false);
    }
    if worktree::merge_in_progress(ws) && !worktree::commit_merge(ws) {
        reporter.status(
            &rid,
            "failed",
            &tool,
            &model,
            "resolver could not commit the merge",
        );
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
    Ok(true)
}

/// Default wall-clock cap for one verify.sh run (env-overridable via
/// AGENTLOOP_GATE_TIMEOUT_SECS). Agents all run under `item_timeout`, but the
/// gate used to run unbounded — a verify.sh that hangs (waits on a port, reads
/// stdin, infinite loop) hung the entire loop forever with no kill path.
const GATE_TIMEOUT_SECS: u64 = 1800;

/// Run verify.sh; capture output to last_gate.txt (latest run) and append it to
/// logs/gate.log (every run, forever); return its exit code (1 if absent,
/// 124 on timeout).
///
/// `scope` is the task id under acceptance, passed to verify.sh as $1 so the
/// script can run only that task's checks. The no-arg (None) run is the global
/// DONE gate. Without scoping, one task's flaky verifier failed the acceptance
/// gate of every other task and cascaded unrelated redesigns.
pub fn gate(ws: &Path, scope: Option<&str>) -> i32 {
    let gate = ws.join(".agentloop/verify.sh");
    let out = ws.join(".agentloop/state/last_gate.txt");
    let timeout = Duration::from_secs(crate::limits::env_secs(
        "AGENTLOOP_GATE_TIMEOUT_SECS",
        GATE_TIMEOUT_SECS,
    ));
    let (code, buf): (i32, Vec<u8>) = if gate.exists() {
        run_gate_script(ws, &gate, timeout, scope)
    } else {
        (1, b"no verify.sh yet".to_vec())
    };
    if let Err(e) = std::fs::write(&out, &buf) {
        // last_gate.txt is the manager/architect feedback signal; a silent write
        // failure would feed them stale gate output.
        eprintln!("gate: could not write {}: {e}", out.display());
    }
    append_gate_log(ws, code, &buf);
    code
}

/// Run verify.sh in its own process group with a wall-clock cap. stdout/stderr
/// are drained on threads (an undrained pipe would deadlock a chatty script).
/// On timeout the whole group gets SIGTERM, a 1s grace, then SIGKILL — same
/// discipline as agent spawns. The group is registered so quit/SIGINT
/// (kill_all_agents) reaps an in-flight gate too.
fn run_gate_script(
    ws: &Path,
    script: &Path,
    timeout: Duration,
    scope: Option<&str>,
) -> (i32, Vec<u8>) {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    let mut cmd = std::process::Command::new("/bin/bash");
    cmd.arg(script);
    if let Some(task_id) = scope {
        cmd.arg(task_id);
    }
    let child = cmd
        .current_dir(ws)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0) // own group: timeout/quit kills descendants too
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return (1, format!("verify.sh spawn failed: {e}").into_bytes()),
    };
    let pgid = child.id() as i32;
    spawn::register_pgid(pgid);

    let mut readers = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        readers.push(std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            buf
        }));
    }
    if let Some(mut err) = child.stderr.take() {
        readers.push(std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf
        }));
    }

    let deadline = Instant::now() + timeout;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(1),
            Ok(None) if Instant::now() >= deadline => {
                use nix::sys::signal::{killpg, Signal};
                use nix::unistd::Pid;
                let pg = Pid::from_raw(pgid);
                let _ = killpg(pg, Signal::SIGTERM);
                std::thread::sleep(Duration::from_secs(1));
                let _ = killpg(pg, Signal::SIGKILL);
                let _ = child.wait();
                break 124;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break 1,
        }
    };
    spawn::unregister_pgid(pgid);

    let mut buf = Vec::new();
    for r in readers {
        if let Ok(b) = r.join() {
            buf.extend_from_slice(&b);
        }
    }
    if code == 124 {
        buf.extend_from_slice(
            format!(
                "\nverify.sh timed out after {}s and was killed (cap: AGENTLOOP_GATE_TIMEOUT_SECS)",
                timeout.as_secs()
            )
            .as_bytes(),
        );
    }
    (code, buf)
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

/// Run the gate with TUI/event visibility. Full verify.sh runs take minutes,
/// and an unreported gate makes the loop look dead while it grinds — the job
/// row is the only signal that the run is still working. The gate itself runs
/// on the blocking pool so a long verify.sh never stalls a runtime worker.
async fn gate_reported(
    ws: &Path,
    reporter: &Arc<dyn Reporter>,
    label: &str,
    scope: Option<&str>,
) -> i32 {
    reporter.dispatch(
        "gate",
        label,
        "",
        "",
        Some(&ws.join(".agentloop/state/last_gate.txt")),
    );
    let ws2 = ws.to_path_buf();
    let scope2 = scope.map(str::to_string);
    let rc = tokio::task::spawn_blocking(move || gate(&ws2, scope2.as_deref()))
        .await
        .unwrap_or(1);
    if rc == 0 {
        reporter.status("gate", "done", "", "", "");
    } else {
        reporter.status(
            "gate",
            "failed",
            "",
            "",
            &format!("verify.sh rc={rc} (see .agentloop/logs/gate.log)"),
        );
    }
    rc
}

/// The run is DONE only when the gate passes and no open OR failed items
/// remain. Failed items are not dispatchable, but declaring DONE over them
/// would silently abandon work the manager is required to reshape or drop
/// (manager prompt rule 8) — so they hold the loop open for another round.
fn loop_done(gate_state: &str, open: i64, failed: i64) -> bool {
    gate_state == "pass" && open == 0 && failed == 0
}

/// No-progress detector. An iteration makes progress when it merges work or
/// changes any loop-relevant state (gate verdict, backlog/builder statuses or
/// attempts — all cap-bounded, so they cannot count as progress forever).
/// Two consecutive no-progress iterations (three identical in a row) = stalled.
struct StallTracker {
    stalls: u32,
    prev_gate: String,
    prev_fp: String,
}

impl StallTracker {
    fn new() -> Self {
        Self {
            stalls: 0,
            prev_gate: "init".into(),
            prev_fp: "init".into(),
        }
    }
    /// Record one iteration; returns true when the loop is stalled.
    fn observe(&mut self, merged: u32, gate_state: &str, fp: &str) -> bool {
        if merged == 0 && gate_state == self.prev_gate && fp == self.prev_fp {
            self.stalls += 1;
        } else {
            self.stalls = 0;
        }
        self.prev_gate = gate_state.to_string();
        self.prev_fp = fp.to_string();
        self.stalls >= 2
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
    let fb = std::fs::read_to_string(task_state::customer_path(ws, task_id))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| {
            v.get("acceptance_notes")
                .or_else(|| v.get("summary"))
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "customer rejected the completed task".to_string());
    // Same E2BIG budget as gate feedback: this string lands in backlog.json
    // notes / redesign.json and is inlined into manager+architect prompts, so a
    // customer that pastes a test dump would recreate the argv crash loop.
    crate::limits::clamp_str(&fb, FEEDBACK_MAX_BYTES as usize)
}

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

/// Cap on any agent-produced feedback embedded in a task note (gate output,
/// customer rejection notes). Notes are stored in backlog.json and inlined into
/// manager/architect prompts, so an unbounded string (a verbose verify.sh once
/// wrote 365KB) would push the spawn argv past the OS ARG_MAX and every
/// dispatch would die with E2BIG ("Argument list too long") — a crash loop,
/// since the bloated backlog persists across runs.
const FEEDBACK_MAX_BYTES: u64 = 8 * 1024;

/// Redesign feedback for a failed task-scoped gate run. The wording steers the
/// next architect pass at the FEATURE, not the gate: fed back verbatim, plain
/// "verify.sh failed" output taught architects to plan verifier scripts,
/// runners, and evidence documents instead of product code (observed: 80% of
/// all builder items across a real run were verification tooling).
fn gate_failure_feedback(ws: &Path, task_id: &str) -> String {
    let path = ws.join(".agentloop/state/last_gate.txt");
    let full_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let output = crate::limits::log_tail(&path, FEEDBACK_MAX_BYTES)
        .trim()
        .to_string();
    let header = format!(
        "verify gate failed for {task_id} — the feature does not behave as required. \
         Redesign must fix the product behavior; do not build verification tooling, \
         gate scripts, or evidence documents in response."
    );
    if output.is_empty() {
        header
    } else if full_len > FEEDBACK_MAX_BYTES {
        format!(
            "{header} (output truncated to the last {FEEDBACK_MAX_BYTES} bytes; full output in .agentloop/state/last_gate.txt):\n{output}"
        )
    } else {
        format!("{header}\n{output}")
    }
}

fn reopen_unapproved_done_tasks(bk: &Path, ws: &Path) -> Result<u32> {
    let backlog = state::read(bk)?;
    let empty = vec![];
    let ids: Vec<String> = backlog["items"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|item| item["status"] == "done")
        .filter_map(|item| item["id"].as_str())
        .filter(|id| !task_state::customer_approved(ws, id))
        .map(str::to_string)
        .collect();

    for id in &ids {
        state::set_status(
            bk,
            id,
            "ready",
            "done requires task-local customer approval; reopening",
        )?;
    }

    Ok(ids.len() as u32)
}

/// Deterministic liveness repairs, run right after the manager each round: the
/// backlog must never hold open work the orchestrator can never dispatch, or the
/// loop stalls/standbys with open items and nothing to do.
fn repair_backlog(bk: &Path, ws: &Path, max_redesigns: u32) -> Result<()> {
    // 0) Clamp oversized notes. A backlog poisoned by an unbounded note (e.g.
    //    written by a pre-cap binary) would E2BIG every spawn forever; this
    //    self-heals it instead of crash-looping across runs.
    for id in state::clamp_oversized_notes(bk, FEEDBACK_MAX_BYTES as usize * 2)? {
        eprintln!("repair: clamped oversized notes on {id}");
        crate::history::record(ws, "task", &id, "repair", "clamped oversized notes");
    }

    // 1) Deps on ids missing from the backlog can never be satisfied (e.g. a
    //    manager that leaked task-local sub-item ids) — drop them.
    for (id, dep) in state::strip_unknown_deps(bk)? {
        eprintln!("repair: dropped {id} dep on unknown id {dep}");
    }

    // 2) Only `ready` items reach the architect, so `in_progress` without a valid
    //    local plan would never get planned again (manager rewrite / stale resume)
    //    — flip it back to ready.
    let backlog = state::read(bk)?;
    let empty = vec![];
    for item in backlog["items"].as_array().unwrap_or(&empty) {
        let Some(id) = item["id"].as_str() else {
            continue;
        };
        if item["status"] == "in_progress" && !task_state::builder_plan_valid(ws, id) {
            state::set_status(
                bk,
                id,
                "ready",
                "in_progress without a valid plan; re-architecting",
            )?;
        }
    }

    // 3) A valid plan whose remaining builders can never dispatch (deps on failed
    //    or abandoned builders) deadlocks its parent — reopen it for redesign.
    for task_id in active_business_ids(bk, ws)? {
        if !task_state::builder_plan_valid(ws, &task_id) {
            continue;
        }
        // Builders left in_progress by an interrupted run are stale (nothing is
        // in flight when repairs run): re-queue them instead of reading their
        // absence from the dispatchable set as a deadlock.
        let _ = task_state::reset_stale_in_progress_builders(ws, &task_id)?;
        if task_state::all_builders_done(ws, &task_id)?
            || !task_state::ready_builders(ws, &task_id, 1)?.is_empty()
        {
            continue;
        }
        reopen_parent_for_redesign(
            bk,
            ws,
            &task_id,
            "builder plan deadlocked: no dispatchable builders remain",
            max_redesigns,
        )?;
    }
    Ok(())
}

/// One iteration: manage, architect, dispatch builders, integrate, review. Returns merged count.
pub async fn iterate(cfg: &Config, ws: &Path, n: u32, reporter: &Arc<dyn Reporter>) -> Result<u32> {
    let sdir = ws.join(".agentloop/state");
    let ldir = ws.join(format!(".agentloop/logs/iter-{n}"));
    std::fs::create_dir_all(&ldir)?;
    std::fs::create_dir_all(ws.join(".agentloop/results"))?;
    auto_answer_pending(ws);
    let bk = sdir.join("backlog.json");
    let itimeout = Duration::from_secs(cfg.item_timeout_sec());
    let maxpar = cfg.max_parallel() as usize;
    let maxatt = cfg.max_attempts();
    let maxred = cfg.max_redesigns();

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
        // A doubly-invalid manager round is recoverable: skip the iteration
        // instead of killing the whole autonomous run. If it persists the
        // stall detector ends the run with the failure visible.
        eprintln!("manager failed/invalid; skipping this iteration");
        reporter.status(
            "manager",
            "failed",
            &mtool,
            &mmodel,
            "invalid backlog twice; iteration skipped",
        );
        crate::history::record(
            ws,
            "agent",
            "manager",
            "invalid",
            "manager produced an invalid backlog twice; iteration skipped",
        );
        return Ok(0);
    }
    reporter.status("manager", "done", &mtool, &mmodel, "");
    let _ = reopen_unapproved_done_tasks(&bk, ws)?;
    repair_backlog(&bk, ws, maxred)?;

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
            reporter.status(&aid, "done", &atool, &amodel, "");
        } else {
            state::set_status(&bk, id, "ready", "architect produced invalid task plan")?;
            reporter.status(
                &aid,
                "failed",
                &atool,
                &amodel,
                "architect produced invalid task plan",
            );
        }
    }

    let mut handles = Vec::new();
    let mut dispatched: Vec<(String, String)> = Vec::new();
    let mut pending_redesign = std::collections::BTreeMap::<String, String>::new();
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
                let note =
                    format!("builder {id} exceeded max_attempts ({maxatt}); redesign required");
                task_state::set_builder_status(
                    ws,
                    task_id,
                    &id,
                    "failed",
                    &format!("exceeded max_attempts ({maxatt})"),
                )?;
                reporter.status(
                    &id,
                    "failed",
                    "",
                    "",
                    &format!("exceeded max_attempts ({maxatt})"),
                );
                if dispatched
                    .iter()
                    .any(|(dispatched_task_id, _)| dispatched_task_id == task_id)
                {
                    pending_redesign.insert(task_id.clone(), note);
                } else {
                    reopen_parent_for_redesign(&bk, ws, task_id, &note, maxred)?;
                }
                break;
            }
            let backlog = state::read(&bk)?;
            let Some(parent) = state::item(&backlog, task_id).cloned() else {
                continue;
            };
            let wt = ws.join(format!(".agentloop/worktrees/{id}"));
            // worktree::remove handles the ordering (git remove before rm,
            // then prune) so leftovers from a crashed run can't wedge re-adds.
            worktree::remove(ws, &wt, &format!("item/{id}"));
            if worktree::create(ws, &format!("item/{id}"), &wt).is_err() {
                let note = format!("builder {id} failed before dispatch: worktree create failed");
                task_state::set_builder_status(
                    ws,
                    task_id,
                    &id,
                    "failed",
                    "worktree create failed",
                )?;
                reporter.status(
                    &id,
                    "failed",
                    "",
                    "",
                    "worktree create failed before dispatch",
                );
                if dispatched
                    .iter()
                    .any(|(dispatched_task_id, _)| dispatched_task_id == task_id)
                {
                    pending_redesign.insert(task_id.clone(), note);
                } else {
                    reopen_parent_for_redesign(&bk, ws, task_id, &note, maxred)?;
                }
                break;
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

    // Loop-owned tracked changes under .agentloop/ (the manager rewrites
    // verify.sh in the main tree and never commits) must not read as a dirty
    // tree below — that bounced every finished merge until the redesign caps
    // blew. Commit them now; user files are untouched.
    worktree::commit_agentloop_changes(ws);

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
            let _ = crate::history::archive_file(&rfile, &ldir);
            continue;
        }

        let result_done = status == "done";
        if result_done {
            if !worktree::has_commits_ahead(ws, &branch) {
                // A builder that verified its slice and found the acceptance
                // criteria already hold has nothing to commit — designs say
                // "leave it alone instead of churning". That is a first-class
                // done when declared explicitly; the task-scoped gate still
                // judges it. An UNdeclared no-commit done stays a bounce (the
                // anti-laziness guard).
                let declared_no_changes = result_value
                    .as_ref()
                    .map(|v| v["no_changes"] == Value::Bool(true))
                    .unwrap_or(false);
                if declared_no_changes {
                    task_state::set_builder_status(
                        ws,
                        task_id,
                        id,
                        "done",
                        "no changes needed (verified)",
                    )?;
                    reporter.status(
                        id,
                        "done",
                        "",
                        "",
                        "no changes needed; existing code already satisfies the item",
                    );
                } else {
                    task_state::set_builder_status(
                        ws,
                        task_id,
                        id,
                        "ready",
                        "builder reported done but made no commits",
                    )?;
                    reporter.status(id, "bounced", "", "", "reported done but made no commits");
                }
            } else if worktree::is_dirty(ws) {
                // Never merge into a dirty user tree: the merge (or the
                // permission-skipping resolver agent after a conflict) could
                // clobber or silently commit the user's uncommitted work.
                // The dirty tree says nothing about the builder's work, so
                // the attempt is refunded — an external condition must not
                // churn the item into max_attempts -> spurious redesign.
                task_state::set_builder_status(
                    ws,
                    task_id,
                    id,
                    "ready",
                    "workspace has uncommitted changes; merge skipped — commit or stash them",
                )?;
                task_state::decrement_builder_attempts(ws, task_id, id)?;
                reporter.status(
                    id,
                    "bounced",
                    "",
                    "",
                    "workspace dirty (uncommitted changes); merge skipped",
                );
            } else {
                match worktree::merge_or_conflict(ws, &branch)? {
                    MergeOutcome::Merged => {
                        task_state::set_builder_status(ws, task_id, id, "done", "")?;
                        reporter.status(id, "merged", "", "", "");
                        merged += 1;
                    }
                    MergeOutcome::Conflict => {
                        let item_v = builder_item(ws, task_id, id)?.unwrap_or(Value::Null);
                        let label = item_v["title"].as_str().unwrap_or("").to_string();
                        if resolve_conflict(cfg, ws, id, &label, &item_v, &branch, n, reporter)
                            .await?
                        {
                            task_state::set_builder_status(ws, task_id, id, "done", "")?;
                            reporter.status(id, "merged", "", "", "");
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
                            reporter.status(
                                id,
                                "bounced",
                                "",
                                "",
                                "merge conflict; resolver failed",
                            );
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
            reporter.status(
                id,
                "failed",
                "",
                "",
                "did not report done (missing/invalid result file or status != done)",
            );
        }
        worktree::remove(ws, &ws.join(format!(".agentloop/worktrees/{id}")), &branch);
        let _ = crate::history::archive_file(&rfile, &ldir);
    }

    for (task_id, note) in pending_redesign {
        reopen_parent_for_redesign(&bk, ws, &task_id, &note, maxred)?;
    }

    for task_id in active_business_ids(&bk, ws)? {
        if !task_state::builder_plan_valid(ws, &task_id)
            || !task_state::all_builders_done(ws, &task_id)?
        {
            continue;
        }
        // Task-scoped acceptance run: verify.sh gets the task id as $1 so this
        // task is judged on its own checks — a flaky verifier belonging to some
        // other task must not burn this one's redesign budget.
        let label = format!("verify gate · {task_id}");
        if gate_reported(ws, reporter, &label, Some(&task_id)).await != 0 {
            let feedback = gate_failure_feedback(ws, &task_id);
            reopen_parent_for_redesign(&bk, ws, &task_id, &feedback, maxred)?;
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
            task_state::reset_redesign(ws, &task_id);
            reporter.status(&cid, "approved", &ctool, &cmodel, "");
        } else {
            let feedback = customer_feedback(ws, &task_id);
            reopen_parent_for_redesign(&bk, ws, &task_id, &feedback, maxred)?;
            reporter.status(&cid, "rejected", &ctool, &cmodel, &feedback);
        }
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
    let mut n = 0u32;
    let mut stall = StallTracker::new();

    while n < maxit {
        n += 1;
        if spawn::shutdown_requested() {
            eprintln!("STOP: shutdown requested");
            return Ok(1);
        }
        if start.elapsed() >= budget {
            eprintln!("STOP: time budget exceeded");
            return Ok(1);
        }

        let merged = iterate(cfg, ws, n, &reporter).await?;
        let _ = reopen_unapproved_done_tasks(&bk, ws)?;

        let grc = gate_reported(ws, &reporter, &format!("verify gate · iter {n}"), None).await;
        let gate_state = if grc == 0 { "pass" } else { "fail" };
        let open = state::open_count(&bk)?;
        let failed = state::failed_count(&bk)?;
        reporter.iteration(n, merged, gate_state, open);

        if loop_done(gate_state, open, failed) {
            eprintln!("DONE");
            return Ok(0);
        }

        let fp = state::progress_fingerprint(&bk, ws);
        if stall.observe(merged, gate_state, &fp) {
            eprintln!(
                "STOP: no progress for 2 stalls (3 consecutive iterations); {open} open / {failed} failed remain"
            );
            return Ok(1);
        }
    }
    eprintln!("STOP: max_iterations reached");
    Ok(1)
}

/// Interactive driver with a standby state machine. DONE/cap/stall transitions to
/// standby (idle, awaiting a command) instead of exiting. AddTask re-engages with a
/// fresh budget window; Quit exits. Tasks can also be added mid-run.
pub async fn run_interactive(
    cfg: &Config,
    ws: &Path,
    reporter: Arc<dyn Reporter>,
    crx: &mut mpsc::UnboundedReceiver<Command>,
) -> Result<i32> {
    // Routing edits (Command::SetRole) apply mid-run: work on a local mutable
    // copy of the config. Caps are read once — SetRole only touches routing.
    let mut cfg = cfg.clone();
    let bk = ws.join(".agentloop/state/backlog.json");
    let maxit = cfg.max_iterations();
    let budget = Duration::from_secs(cfg.total_budget_sec());

    let mut n = 0u32;
    let mut stall = StallTracker::new();
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
            // Stray add-task before the run starts: ignore.
            Some(Command::AddTask { .. }) => {}
            Some(Command::SetRole { role, tool, model, effort }) => {
                crate::config::apply_role(&mut cfg, &role, &tool, &model, &effort);
            }
        }
    }

    'outer: loop {
        // --- WORKING phase ---
        // Breaks with the human-readable reason the loop parked (shown in the
        // TUI status bar so "why did it stop?" is answerable at a glance).
        let standby_reason: String = 'working: loop {
            // Drain any queued commands without blocking (mid-run answers/add-task/quit).
            while let Ok(cmd) = crx.try_recv() {
                match cmd {
                    Command::Quit => return Ok(0),
                    Command::AddTask { request } => {
                        if let Err(e) = crate::requests::append(ws, &request) {
                            reporter.status(
                                "addtask",
                                "failed",
                                "",
                                "",
                                &format!("could not queue request: {e:#}"),
                            );
                        }
                    }
                    Command::StartRun { .. } => {}
                    Command::SetRole { role, tool, model, effort } => {
                        crate::config::apply_role(&mut cfg, &role, &tool, &model, &effort);
                    }
                }
            }
            // Quit/SIGINT must stop the loop at the next boundary even if the
            // Quit command itself was lost (e.g. signal path).
            if spawn::shutdown_requested() {
                return Ok(0);
            }

            let open = state::open_count(&bk).unwrap_or(0);
            let failed = state::failed_count(&bk).unwrap_or(0);
            if iters_this_window >= maxit {
                eprintln!("STOP(window): max_iterations");
                break 'working format!("max_iterations ({maxit}) · {open} open / {failed} failed");
            }
            if window_start.elapsed() >= budget {
                eprintln!("STOP(window): budget");
                break 'working format!("budget exhausted · {open} open / {failed} failed");
            }

            n += 1;
            iters_this_window += 1;
            let merged = iterate(&cfg, ws, n, &reporter).await?;
            let _ = reopen_unapproved_done_tasks(&bk, ws)?;
            let grc =
                gate_reported(ws, &reporter, &format!("verify gate · iter {n}"), None).await;
            let gate_state = if grc == 0 { "pass" } else { "fail" };
            let open = state::open_count(&bk)?;
            let failed = state::failed_count(&bk)?;
            reporter.iteration(n, merged, gate_state, open);

            if loop_done(gate_state, open, failed) {
                break 'working "all tasks done · gate passing".into();
            }

            let fp = state::progress_fingerprint(&bk, ws);
            if stall.observe(merged, gate_state, &fp) {
                eprintln!("STOP: no progress (stall)");
                break 'working format!("no progress (stall) · {open} open / {failed} failed");
            }
        };

        // --- STANDBY phase ---
        reporter.standby(&standby_reason);
        loop {
            match crx.recv().await {
                None | Some(Command::Quit) => return Ok(0),
                Some(Command::AddTask { request }) => {
                    if let Err(e) = crate::requests::append(ws, &request) {
                        reporter.status(
                            "addtask",
                            "failed",
                            "",
                            "",
                            &format!("could not queue request: {e:#}"),
                        );
                    }
                    break;
                }
                // The goal was already committed when the run first started;
                // standby StartRun is a re-engage only (same as pre-restructure).
                Some(Command::StartRun { .. }) => break,
                // Routing edits don't re-engage the loop; keep waiting for work.
                Some(Command::SetRole { role, tool, model, effort }) => {
                    crate::config::apply_role(&mut cfg, &role, &tool, &model, &effort);
                }
            }
        }
        // Re-engage: fresh budget window + iteration allowance, reset stall.
        window_start = Instant::now();
        iters_this_window = 0;
        stall = StallTracker::new();
        continue 'outer;
    }
}

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
        std::fs::write(
            sdir.join("backlog.json"),
            serde_json::to_vec(&backlog).unwrap(),
        )
        .unwrap();
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

    #[test]
    fn gate_failure_feedback_is_bounded_for_huge_gate_output() {
        let ws = tmp_ws("orch-feedback-cap");
        std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
        // A verbose verify.sh (xcodebuild-style) once wrote 365KB to
        // last_gate.txt; embedded unbounded into task notes it pushed
        // backlog.json past ARG_MAX and every agent spawn died with E2BIG.
        let huge = format!(
            "{}FINAL FAILURE LINE",
            "line of verify output\n".repeat(20_000)
        );
        std::fs::write(ws.join(".agentloop/state/last_gate.txt"), &huge).unwrap();

        let note = gate_failure_feedback(&ws, "task-1");

        assert!(note.starts_with("verify gate failed for task-1"));
        assert!(
            note.len() <= 16 * 1024,
            "note must stay bounded, got {} bytes",
            note.len()
        );
        assert!(
            note.contains("FINAL FAILURE LINE"),
            "the tail of the output (where failures land) is kept"
        );
        assert!(
            note.contains("last_gate.txt"),
            "truncated note points at the full gate output"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn customer_feedback_is_bounded_for_huge_review_notes() {
        let ws = tmp_ws("orch-customer-cap");
        let tdir = ws.join(".agentloop/state/tasks/task-1");
        std::fs::create_dir_all(&tdir).unwrap();
        // A customer that pastes a full test dump into acceptance_notes would
        // recreate the E2BIG argv crash loop via backlog notes/redesign.json.
        let huge = format!("STARTS HERE {}", "review detail\n".repeat(20_000));
        std::fs::write(
            tdir.join("customer.json"),
            serde_json::to_vec(&json!({"status":"rejected","acceptance_notes": huge})).unwrap(),
        )
        .unwrap();

        let fb = customer_feedback(&ws, "task-1");

        assert!(fb.len() <= 9 * 1024, "bounded, got {} bytes", fb.len());
        assert!(fb.starts_with("STARTS HERE"));
        assert!(fb.contains("[truncated"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn gate_passes_scope_to_verify_sh_as_arg1() {
        let ws = tmp_ws("orch-gate-scope");
        std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
        // The per-task acceptance gate is scoped: verify.sh receives the task id
        // as $1 so one task's flaky verifier can no longer fail (and redesign)
        // an unrelated task. The no-arg run stays the global DONE gate.
        std::fs::write(
            ws.join(".agentloop/verify.sh"),
            "#!/bin/bash\necho \"scope=${1:-GLOBAL}\"\nexit 0\n",
        )
        .unwrap();

        assert_eq!(gate(&ws, Some("task-7")), 0);
        let scoped = std::fs::read_to_string(ws.join(".agentloop/state/last_gate.txt")).unwrap();
        assert!(scoped.contains("scope=task-7"), "got: {scoped}");

        assert_eq!(gate(&ws, None), 0);
        let global = std::fs::read_to_string(ws.join(".agentloop/state/last_gate.txt")).unwrap();
        assert!(global.contains("scope=GLOBAL"), "got: {global}");

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn gate_failure_feedback_blames_the_scoped_task_and_demands_product_fixes() {
        let ws = tmp_ws("orch-feedback-scope");
        std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
        std::fs::write(ws.join(".agentloop/state/last_gate.txt"), "assertion failed").unwrap();

        let note = gate_failure_feedback(&ws, "task-7");

        assert!(note.contains("task-7"), "feedback names the gated task");
        assert!(
            note.contains("fix the product behavior"),
            "feedback steers the redesign toward the feature, got: {note}"
        );
        assert!(
            note.contains("do not build verification tooling"),
            "feedback forbids gate-repair plans, got: {note}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn gate_times_out_and_kills_a_hung_verify_sh() {
        let ws = tmp_ws("orch-gate-timeout");
        std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
        // A verify.sh that hangs (port wait, stdin read, infinite loop) used to
        // hang the whole loop forever — there was no gate timeout at all.
        std::fs::write(ws.join(".agentloop/verify.sh"), "#!/bin/bash\nsleep 600\n").unwrap();
        std::env::set_var("AGENTLOOP_GATE_TIMEOUT_SECS", "1");

        let start = std::time::Instant::now();
        let rc = gate(&ws, None);
        std::env::remove_var("AGENTLOOP_GATE_TIMEOUT_SECS");

        assert_eq!(rc, 124, "timeout reported like timeout(1)");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "killed promptly, took {:?}",
            start.elapsed()
        );
        let last = std::fs::read_to_string(ws.join(".agentloop/state/last_gate.txt")).unwrap();
        assert!(last.contains("timed out"), "feedback names the timeout");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn repair_backlog_clamps_poisoned_notes() {
        let ws = tmp_ws("orch-clamp-notes");
        setup(&ws);
        let bk = ws.join(".agentloop/state/backlog.json");
        // Simulate a backlog bloated by a pre-cap binary: a 365KB-style note.
        state::set_status(&bk, "task-1", "in_progress", &"x".repeat(300_000)).unwrap();

        repair_backlog(&bk, &ws, 3).unwrap();

        let v = state::read(&bk).unwrap();
        let notes = state::item(&v, "task-1").unwrap()["notes"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            notes.len() <= 2 * FEEDBACK_MAX_BYTES as usize + 100,
            "self-healed, got {} bytes",
            notes.len()
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn repair_backlog_leaves_healthy_tasks_alone() {
        let ws = tmp_ws("orch-healthy");
        setup(&ws);
        let bk = ws.join(".agentloop/state/backlog.json");

        repair_backlog(&bk, &ws, 3).unwrap();

        let v = state::read(&bk).unwrap();
        assert_eq!(state::item(&v, "task-1").unwrap()["status"], "in_progress");
        assert!(task_state::builders_path(&ws, "task-1").exists());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn repair_backlog_flips_in_progress_without_plan_to_ready() {
        let ws = tmp_ws("orch-noplan");
        setup(&ws);
        std::fs::remove_file(ws.join(".agentloop/state/tasks/task-1/builders.json")).unwrap();
        let bk = ws.join(".agentloop/state/backlog.json");

        repair_backlog(&bk, &ws, 3).unwrap();

        let v = state::read(&bk).unwrap();
        let task = state::item(&v, "task-1").unwrap();
        assert_eq!(task["status"], "ready");
        assert!(task["notes"].as_str().unwrap().contains("re-architecting"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn repair_backlog_reopens_deadlocked_builder_plan() {
        let ws = tmp_ws("orch-deadlock");
        setup(&ws);
        // Remaining ready builder deps on a failed one: never dispatchable.
        std::fs::write(
            ws.join(".agentloop/state/tasks/task-1/builders.json"),
            r#"{"items":[
                {"id":"task-1-b1","title":"t","desc":"d","deps":[],"status":"failed","attempts":3,"acceptance":"a"},
                {"id":"task-1-b2","title":"t","desc":"d","deps":["task-1-b1"],"status":"ready","attempts":0,"acceptance":"a"}
            ]}"#,
        )
        .unwrap();
        let bk = ws.join(".agentloop/state/backlog.json");

        repair_backlog(&bk, &ws, 3).unwrap();

        let v = state::read(&bk).unwrap();
        assert_eq!(state::item(&v, "task-1").unwrap()["status"], "ready");
        assert!(
            !task_state::builders_path(&ws, "task-1").exists(),
            "deadlocked plan is invalidated for redesign"
        );
        assert_eq!(
            task_state::read_redesign(&ws, "task-1").0,
            1,
            "deadlock consumes a redesign"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn gate_appends_every_run_to_gate_log() {
        let ws = tmp_ws("orch-gatelog");
        std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();

        assert_eq!(gate(&ws, None), 1); // no verify.sh yet
        std::fs::write(ws.join(".agentloop/verify.sh"), "#!/bin/bash\nexit 0\n").unwrap();
        assert_eq!(gate(&ws, None), 0);

        let log = std::fs::read_to_string(ws.join(".agentloop/logs/gate.log")).unwrap();
        assert_eq!(log.matches("=== ").count(), 2, "both runs recorded");
        assert!(log.contains("rc=1") && log.contains("rc=0"));
        assert!(
            ws.join(".agentloop/state/last_gate.txt").exists(),
            "latest-run file still maintained"
        );

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn repair_backlog_requeues_stale_in_progress_builders_instead_of_redesigning() {
        let ws = tmp_ws("orch-stale");
        setup(&ws);
        // One builder done, one left in_progress by an interrupted run.
        std::fs::write(
            ws.join(".agentloop/state/tasks/task-1/builders.json"),
            r#"{"items":[
                {"id":"task-1-b1","title":"t","desc":"d","deps":[],"status":"done","attempts":1,"acceptance":"a"},
                {"id":"task-1-b2","title":"t","desc":"d","deps":[],"status":"in_progress","attempts":1,"acceptance":"a"}
            ]}"#,
        )
        .unwrap();
        let bk = ws.join(".agentloop/state/backlog.json");

        repair_backlog(&bk, &ws, 3).unwrap();

        let builders = task_state::read_builders(&ws, "task-1").unwrap();
        assert_eq!(
            task_state::item(&builders, "task-1-b2").unwrap()["status"],
            "ready",
            "stale builder is re-queued, not redesigned away"
        );
        assert!(
            task_state::builders_path(&ws, "task-1").exists(),
            "plan with done work is kept"
        );
        assert_eq!(
            task_state::read_redesign(&ws, "task-1").0,
            0,
            "no redesign is burned for a crash orphan"
        );
        let v = state::read(&bk).unwrap();
        assert_eq!(state::item(&v, "task-1").unwrap()["status"], "in_progress");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn loop_done_requires_pass_and_no_open_and_no_failed() {
        assert!(loop_done("pass", 0, 0));
        assert!(!loop_done("fail", 0, 0));
        assert!(!loop_done("pass", 1, 0));
        assert!(
            !loop_done("pass", 0, 2),
            "failed tasks must keep the loop alive for a manager reshape round"
        );
    }

    #[test]
    fn stall_tracker_stalls_only_when_nothing_changes() {
        let mut t = StallTracker::new();
        // First iteration always counts as progress (init baselines).
        assert!(!t.observe(0, "fail", "fp-a"));
        // Two identical no-merge iterations in a row -> stalled.
        assert!(!t.observe(0, "fail", "fp-a"));
        assert!(t.observe(0, "fail", "fp-a"));
    }

    #[test]
    fn stall_tracker_counts_state_changes_as_progress() {
        let mut t = StallTracker::new();
        assert!(!t.observe(0, "fail", "fp-a"));
        assert!(!t.observe(0, "fail", "fp-a"));
        // Backlog/builder state changed (e.g. manager re-scoped a failed task):
        // progress even with zero merges.
        assert!(!t.observe(0, "fail", "fp-b"));
        assert!(!t.observe(0, "fail", "fp-b"));
        // Gate verdict flip is progress too.
        assert!(!t.observe(0, "pass", "fp-b"));
        // A merge always resets the counter.
        assert!(!t.observe(0, "pass", "fp-b"));
        assert!(!t.observe(1, "pass", "fp-b"));
        assert!(!t.observe(0, "pass", "fp-b"));
        assert!(t.observe(0, "pass", "fp-b"));
    }
}
