use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::spawn;

pub fn architect_prompt(ws: &Path, task: &Value) -> String {
    let id = task["id"].as_str().unwrap_or("");
    let title = task["title"].as_str().unwrap_or("");
    let desc = task["desc"].as_str().unwrap_or("");
    let acc = task["acceptance"]
        .as_str()
        .unwrap_or("the business task is accepted");
    let task_dir = format!(".agentloop/state/tasks/{id}");
    format!(
        r#"You are the ARCHITECT for one business task in an autonomous app build. Working dir: {ws} (a git repo).

BUSINESS TASK:
  id: {id}
  title: {title}
  task: {desc}
  acceptance criteria: {acc}

Your job:
1. Inspect the application and decide the technical plan for this one business task.
2. Write {task_dir}/design.md with the implementation approach, files/components likely involved, constraints, and verification notes.
3. Write {task_dir}/builders.json with builder-sized implementation items.

OUTPUT CONTRACT — you MUST write valid files:
- {task_dir}/design.md must be non-empty.
- {task_dir}/builders.json must be valid JSON shaped like:
  {{"items":[{{"id","title","desc","deps":[],"status":"ready|in_progress|done|failed|blocked","attempts":0,"acceptance"}}]}}

Do not edit application source code. Do not implement the task. Do not write global backlog files."#,
        ws = ws.display(),
        id = id,
        title = title,
        desc = desc,
        acc = acc,
        task_dir = task_dir
    )
}

pub async fn architect_run(
    cfg: &Config,
    ws: &Path,
    task: &Value,
    log: &Path,
    t: Duration,
) -> Result<bool> {
    let prompt = architect_prompt(ws, task);
    spawn::agent_run(cfg, "architect", &prompt, ws, log, t).await?;
    if architect_output_valid(ws, task) {
        return Ok(true);
    }

    eprintln!("architect produced invalid task plan; re-prompting once");
    let retry = format!(
        "{prompt}\nNOTE: your previous task plan was invalid. Write a non-empty design.md and valid builders.json with an items array."
    );
    spawn::agent_run(cfg, "architect", &retry, ws, log, t).await?;
    Ok(architect_output_valid(ws, task))
}

fn architect_output_valid(ws: &Path, task: &Value) -> bool {
    let id = task["id"].as_str().unwrap_or("");
    if id.is_empty() {
        return false;
    }
    let dir = ws.join(".agentloop/state/tasks").join(id);
    let design = std::fs::read_to_string(dir.join("design.md")).unwrap_or_default();
    if design.trim().is_empty() {
        return false;
    }
    let builders = match std::fs::read_to_string(dir.join("builders.json")) {
        Ok(text) => text,
        Err(_) => return false,
    };
    matches!(
        serde_json::from_str::<Value>(&builders),
        Ok(v) if v.get("items").and_then(|items| items.as_array()).is_some()
    )
}
