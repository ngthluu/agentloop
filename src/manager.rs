use anyhow::Result;
use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::{spawn, state};

pub fn manager_prompt(ws: &Path, max_attempts: u32) -> String {
    let st = ws.join(".agentloop/state");
    let goal = std::fs::read_to_string(st.join("goal.md")).unwrap_or_default();
    let master = std::fs::read_to_string(st.join("master.md")).unwrap_or_default();
    let backlog = std::fs::read_to_string(st.join("backlog.json")).unwrap_or_default();
    let requests = crate::requests::prompt_block(ws).unwrap_or_default();
    let stuck = state::failed_dep_report(&st.join("backlog.json"))
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| {
            format!(
                "\n\nSTUCK ITEMS — these depend on failed items and can never run; reshape or drop them this round:\n{s}"
            )
        })
        .unwrap_or_default();
    // EVERY failed item must be surfaced, dependents or not: failed items hold
    // the run open (DONE requires zero failed) but are not dispatchable, so a
    // failed leaf task the manager never hears about would park the run forever.
    let failed = state::failed_items_report(&st.join("backlog.json"))
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| {
            format!(
                "\n\nFAILED ITEMS — these hold the run open and will never be retried as-is. For each one, this round you MUST either reshape it into new smaller \"ready\" tasks or delete it from the backlog (record why in master.md):\n{s}"
            )
        })
        .unwrap_or_default();
    format!(
        r#"You are the MANAGER for an autonomous app build. Working dir: {ws} (a git repo).
You own business tasks only.

GOAL:
{goal}

CURRENT master.md:
{master}

CURRENT backlog.json:
{backlog}

Your job each round:
1. Fold pending user requests into the business backlog.
2. Add/split/refine business tasks so the GOAL is represented as user-visible outcomes with clear acceptance criteria.
3. Update business task sequencing, notes, readiness, blocked/failed states, and acceptance criteria as needed.
4. The orchestrator FAILS any item once its attempts reach {max_attempts} (the max_attempts cap).
   So for any item nearing attempts={max_attempts}, reshape it into smaller business outcomes or drop it.
5. Keep each item business-facing: describe what the user gets, not implementation details.
6. Statuses you may write are "ready", "blocked", and "failed". NEVER write "in_progress" — the orchestrator owns that transition.
7. Every id inside any "deps" array must be the id of another item in this backlog.json. Never invent ids and never copy task-local sub-items (ids like "task-3-b2", which live under .agentloop/state/tasks/) into this backlog; the orchestrator strips deps on unknown ids.
8. If an open item depends on a "failed" item it can never run: reshape or drop it this round.

Completion ownership:
- Do NOT create status="done" yourself.
- Leave customer-approved done tasks alone.
- A task may be status="done" only when .agentloop/state/tasks/<task-id>/customer.json exists with status="approved".
- The orchestrator marks tasks done after verify.sh passes and the customer approves.

OUTPUT CONTRACT — you MUST overwrite .agentloop/state/backlog.json with valid JSON:
{{"items":[{{"id":"task-1","title":"User-visible outcome","desc":"What the user needs","deps":[],"status":"ready","attempts":0,"acceptance":"Observable acceptance criteria"}}]}}
Also rewrite .agentloop/state/master.md as a human-readable status board.
Do not print the JSON to stdout; write the files.{requests}{stuck}{failed}"#,
        ws = ws.display(),
        goal = goal,
        master = master,
        backlog = backlog,
        max_attempts = max_attempts,
        requests = requests,
        stuck = stuck,
        failed = failed
    )
}

pub async fn manager_run(cfg: &Config, ws: &Path, log: &Path, t: Duration) -> Result<bool> {
    let bk = ws.join(".agentloop/state/backlog.json");
    let max_attempts = cfg.max_attempts();
    // Snapshot BEFORE building the prompt: only requests that could have been
    // in the prompt may be consumed afterwards. A request the user adds during
    // the minutes-long agent run must stay pending for the next round, not be
    // marked consumed without the manager ever seeing it.
    let req_snapshot = crate::requests::count(ws).unwrap_or(0);
    let prompt = manager_prompt(ws, max_attempts);
    spawn::agent_run(cfg, "manager", &prompt, ws, log, t).await?;
    if state::backlog_valid(&bk) {
        let _ = crate::requests::mark_consumed_first(ws, req_snapshot);
        return Ok(true);
    }

    eprintln!("manager produced invalid backlog.json; re-prompting once");
    let retry = format!(
        "{prompt}\nNOTE: your previous backlog.json was invalid JSON. Write valid JSON this time."
    );
    spawn::agent_run(cfg, "manager", &retry, ws, log, t).await?;
    let ok = state::backlog_valid(&bk);
    if ok {
        let _ = crate::requests::mark_consumed_first(ws, req_snapshot);
    }
    Ok(ok)
}
