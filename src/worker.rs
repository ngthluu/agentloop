use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::spawn;

pub fn worker_prompt(ws: &Path, item: &Value) -> String {
    let id = item["id"].as_str().unwrap_or("");
    let title = item["title"].as_str().unwrap_or("");
    let desc = item["desc"].as_str().unwrap_or("");
    let acc = item["acceptance"].as_str().unwrap_or("the change builds and tests pass");
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
  {{"status":"done|failed","summary":"one line","files_changed":["..."]}}"#,
        id = id, title = title, desc = desc, acc = acc, ws = ws.display())
}

/// Dispatch one item: returns agent_run's exit code; the result file is the source of truth.
pub async fn worker_dispatch(cfg: &Config, ws: &Path, item: &Value, wt: &Path, log: &Path, t: Duration) -> Result<i32> {
    let role = item["role"].as_str().unwrap_or("build");
    let prompt = worker_prompt(ws, item);
    spawn::agent_run(cfg, role, &prompt, wt, log, t).await
}
