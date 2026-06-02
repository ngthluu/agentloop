use anyhow::Result;
use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::{spawn, state};

pub fn planner_prompt(ws: &Path, max_attempts: u32) -> String {
    let st = ws.join(".agentloop/state");
    let goal = std::fs::read_to_string(st.join("goal.md")).unwrap_or_default();
    let master = std::fs::read_to_string(st.join("master.md")).unwrap_or_default();
    let backlog = std::fs::read_to_string(st.join("backlog.json")).unwrap_or_default();
    let requests = crate::requests::prompt_block(ws).unwrap_or_default();
    format!(r#"You are the PLANNER for an autonomous app build. Working dir: {ws} (a git repo).

GOAL:
{goal}

CURRENT master.md:
{master}

CURRENT backlog.json:
{backlog}

Your job each round:
1. Read worker results in .agentloop/results/ and the latest gate output in
   .agentloop/state/last_gate.txt (if present). Mark finished items status="done".
2. Add/split/refine items so the GOAL gets built. First round: scaffold the project
   and write an executable .agentloop/verify.sh that builds/tests the app (start simple).
3. The orchestrator FAILS any item once its attempts reach {max_attempts} (the max_attempts cap).
   So for any item nearing attempts={max_attempts}, redesign it (smaller/different) or drop it
   instead of re-queueing the same work.
4. Assign each open item a role from the config routing (planner|architect|build|fix|trivial),
   realistic deps (ids of items that must finish first), and a concrete acceptance string.

OUTPUT CONTRACT — you MUST overwrite .agentloop/state/backlog.json with valid JSON:
{{"items":[{{"id","title","desc","role","deps":[],"status":"ready|in_progress|done|failed|blocked","attempts":0,"acceptance"}}]}}
Also rewrite .agentloop/state/master.md as a human-readable status board.
Do not print the JSON to stdout; write the files.{requests}"#,
        ws = ws.display(), goal = goal, master = master, backlog = backlog, max_attempts = max_attempts, requests = requests)
}

/// Invoke the planner agent, then validate backlog.json (re-prompt once on invalid).
pub async fn planner_run(cfg: &Config, ws: &Path, log: &Path, t: Duration) -> Result<bool> {
    let bk = ws.join(".agentloop/state/backlog.json");
    let max_attempts = cfg.max_attempts();
    let prompt = planner_prompt(ws, max_attempts);
    spawn::agent_run(cfg, "planner", &prompt, ws, log, t).await?;
    if state::backlog_valid(&bk) {
        let _ = crate::requests::mark_all_consumed(ws);
        return Ok(true);
    }

    eprintln!("planner produced invalid backlog.json; re-prompting once");
    let retry = format!("{prompt}\nNOTE: your previous backlog.json was invalid JSON. Write valid JSON this time.");
    spawn::agent_run(cfg, "planner", &retry, ws, log, t).await?;
    let ok = state::backlog_valid(&bk);
    if ok {
        let _ = crate::requests::mark_all_consumed(ws);
    }
    Ok(ok)
}
