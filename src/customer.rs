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
   {{"status":"approved","summary":"one line","acceptance_notes":"what passed or failed"}}
4. Write {result_path} with valid JSON:
   {{"status":"approved","summary":"same one line"}}

Use status "approved" in both files when the acceptance criteria pass.
Use status "rejected" in both files when the acceptance criteria fail.
The two files must use the same status and summary.

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
    let id = task["id"].as_str().unwrap_or("");
    if !id.is_empty() {
        // Archive (never delete) the previous round's review before re-running.
        let dir = crate::task_state::task_dir(ws, id).join("archive");
        let _ = crate::history::archive_file(
            &ws.join(".agentloop/state/tasks")
                .join(id)
                .join("customer.json"),
            &dir,
        );
        let _ = crate::history::archive_file(
            &ws.join(".agentloop/results")
                .join(format!("{id}-customer.json")),
            &dir,
        );
    }

    let prompt = customer_prompt(ws, task);
    spawn::agent_run(cfg, "customer", &prompt, ws, log, t).await?;
    if customer_output_approved(ws, task) {
        return Ok(true);
    }

    eprintln!("customer did not approve task; re-prompting once");
    let retry = format!(
        "{prompt}\nNOTE: your previous review did not approve the task. If the acceptance criteria are satisfied, write both review files with status=\"approved\"."
    );
    spawn::agent_run(cfg, "customer", &retry, ws, log, t).await?;
    Ok(customer_output_approved(ws, task))
}

fn customer_output_approved(ws: &Path, task: &Value) -> bool {
    let id = task["id"].as_str().unwrap_or("");
    if id.is_empty() {
        return false;
    }
    let customer_path = ws
        .join(".agentloop/state/tasks")
        .join(id)
        .join("customer.json");
    let result_path = ws
        .join(".agentloop/results")
        .join(format!("{id}-customer.json"));
    let customer_status = json_status(&customer_path);
    let result_status = json_status(&result_path);
    matches!(
        (customer_status.as_deref(), result_status.as_deref()),
        (Some("approved"), Some("approved"))
    )
}

fn json_status(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| {
            v.get("status")
                .and_then(|status| status.as_str())
                .map(str::to_string)
        })
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

    fn write_customer(ws: &Path, task_id: &str, status: &str) {
        let dir = ws.join(".agentloop/state/tasks").join(task_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("customer.json"),
            format!(r#"{{"status":"{status}","summary":"reviewed"}}"#),
        )
        .unwrap();
    }

    fn write_result(ws: &Path, task_id: &str, status: &str) {
        let dir = ws.join(".agentloop/results");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{task_id}-customer.json")),
            format!(r#"{{"status":"{status}","summary":"reviewed"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn customer_output_approved_only_when_both_files_approved() {
        let ws = tmp_ws("custapproved");
        let task = json!({"id":"task-1"});
        write_customer(&ws, "task-1", "approved");
        write_result(&ws, "task-1", "approved");

        assert!(customer_output_approved(&ws, &task));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn customer_output_false_when_result_missing() {
        let ws = tmp_ws("custmissing");
        let task = json!({"id":"task-1"});
        write_customer(&ws, "task-1", "approved");

        assert!(!customer_output_approved(&ws, &task));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn customer_output_false_when_files_contradict() {
        let ws = tmp_ws("custcontradict");
        let task = json!({"id":"task-1"});
        write_customer(&ws, "task-1", "approved");
        write_result(&ws, "task-1", "rejected");

        assert!(!customer_output_approved(&ws, &task));
        let _ = std::fs::remove_dir_all(&ws);
    }
}
