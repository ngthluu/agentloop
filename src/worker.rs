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

/// Prompt for the conflict-resolver agent. It runs in the MAIN workspace, which is
/// mid-merge with conflicts from `item/<id>`.
pub fn resolver_prompt(ws: &Path, item: &Value) -> String {
    let id = item["id"].as_str().unwrap_or("");
    let title = item["title"].as_str().unwrap_or("");
    let desc = item["desc"].as_str().unwrap_or("");
    format!(r#"You are a MERGE-CONFLICT RESOLVER on an autonomous app build. The git repo at
{ws} is in the middle of merging branch item/{id} into the main branch and has conflicts.
Resolve every conflict so the result reflects the intent of BOTH sides for this item:

  id:    {id}
  title: {title}
  task:  {desc}

Steps:
- Inspect the conflicts (git status; git diff). Resolve all <<<<<<< ======= >>>>>>>
  markers, keeping a correct result that builds.
- `git add` the resolved files, then `git commit --no-edit` to complete the merge.
- Do not change unrelated files and do not start new work."#,
        ws = ws.display(), id = id, title = title, desc = desc)
}

/// Dispatch one item: returns agent_run's exit code; the result file is the source of truth.
pub async fn worker_dispatch(cfg: &Config, ws: &Path, item: &Value, wt: &Path, log: &Path, t: Duration) -> Result<i32> {
    let role = item["role"].as_str().unwrap_or("build");
    let prompt = worker_prompt(ws, item);
    spawn::agent_run(cfg, role, &prompt, wt, log, t).await
}
