use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::spawn;

pub fn customer_prompt(ws: &Path, task: &Value) -> String {
    let id = task["id"].as_str().unwrap_or("");
    let title = task["title"].as_str().unwrap_or("");
    let desc = task["desc"].as_str().unwrap_or("");
    let acc = task["acceptance"]
        .as_str()
        .unwrap_or("the business task is accepted");
    let customer_path = format!(".agentloop/state/tasks/{id}/customer.json");
    let result_path = format!(".agentloop/results/{id}-customer.json");
    format!(
        r#"You are the SILLY CUSTOMER reviewing one completed business task. Working dir: {ws} (a git repo).

BUSINESS TASK:
  id: {id}
  title: {title}
  task: {desc}
  acceptance criteria: {acc}

Your job is acceptance-criteria-only review:
1. Inspect the app behavior and relevant results for this task.
2. Decide whether the acceptance criteria are satisfied from a user's perspective.
3. Write {customer_path} with valid JSON:
   {{"status":"approved|rejected","summary":"one line","acceptance_notes":"what passed or failed"}}
4. Write {result_path} with valid JSON:
   {{"status":"done|failed","summary":"one line","files_changed":[]}}

Do not review unrelated implementation quality. Do not add new requirements. Do not edit application source code."#,
        ws = ws.display(),
        id = id,
        title = title,
        desc = desc,
        acc = acc,
        customer_path = customer_path,
        result_path = result_path
    )
}

pub async fn customer_run(
    cfg: &Config,
    ws: &Path,
    task: &Value,
    log: &Path,
    t: Duration,
) -> Result<bool> {
    let prompt = customer_prompt(ws, task);
    spawn::agent_run(cfg, "customer", &prompt, ws, log, t).await?;
    if customer_output_approved(ws, task) {
        return Ok(true);
    }

    eprintln!("customer did not approve task; re-prompting once");
    let retry = format!(
        "{prompt}\nNOTE: your previous review did not approve the task. If the acceptance criteria are satisfied, write customer.json with status=\"approved\"."
    );
    spawn::agent_run(cfg, "customer", &retry, ws, log, t).await?;
    Ok(customer_output_approved(ws, task))
}

fn customer_output_approved(ws: &Path, task: &Value) -> bool {
    let id = task["id"].as_str().unwrap_or("");
    if id.is_empty() {
        return false;
    }
    let path = ws
        .join(".agentloop/state/tasks")
        .join(id)
        .join("customer.json");
    matches!(
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|v| v.get("status").and_then(|status| status.as_str()).map(str::to_string)),
        Some(status) if status == "approved"
    )
}
