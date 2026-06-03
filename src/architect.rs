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
  {{"items":[{{"id":"builder-1","title":"Implement one focused slice","desc":"Specific implementation work","deps":[],"status":"ready","attempts":0,"acceptance":"How to verify this slice"}}]}}

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
    if let Some(id) = task["id"].as_str().filter(|id| !id.is_empty()) {
        std::fs::create_dir_all(ws.join(".agentloop/state/tasks").join(id))?;
    }
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
        Ok(v) if builders_items_valid(&v)
    )
}

fn builders_items_valid(v: &Value) -> bool {
    let Some(items) = v.get("items").and_then(|items| items.as_array()) else {
        return false;
    };
    !items.is_empty() && items.iter().all(builder_item_valid)
}

fn builder_item_valid(item: &Value) -> bool {
    non_empty_str(item, "id")
        && non_empty_str(item, "title")
        && non_empty_str(item, "desc")
        && non_empty_str(item, "acceptance")
        && item.get("deps").and_then(|deps| deps.as_array()).is_some()
        && matches!(
            item.get("status").and_then(|status| status.as_str()),
            Some("ready" | "in_progress" | "done" | "failed" | "blocked")
        )
        && item
            .get("attempts")
            .and_then(|attempts| attempts.as_u64())
            .is_some()
}

fn non_empty_str(item: &Value, key: &str) -> bool {
    item.get(key)
        .and_then(|value| value.as_str())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
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

    fn write_plan(ws: &Path, task_id: &str, builders: &str) {
        let dir = ws.join(".agentloop/state/tasks").join(task_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("design.md"), "Use the existing importer flow.").unwrap();
        std::fs::write(dir.join("builders.json"), builders).unwrap();
    }

    #[test]
    fn architect_output_accepts_valid_plan() {
        let ws = tmp_ws("archvalid");
        let task = json!({"id":"task-1"});
        write_plan(
            &ws,
            "task-1",
            r#"{"items":[{"id":"b-1","title":"Parser","desc":"Parse CSV","deps":[],"status":"ready","attempts":0,"acceptance":"rows import"}]}"#,
        );

        assert!(architect_output_valid(&ws, &task));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn architect_output_rejects_empty_items() {
        let ws = tmp_ws("archempty");
        let task = json!({"id":"task-1"});
        write_plan(&ws, "task-1", r#"{"items":[]}"#);

        assert!(!architect_output_valid(&ws, &task));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn architect_output_rejects_missing_fields() {
        let ws = tmp_ws("archmissing");
        let task = json!({"id":"task-1"});
        write_plan(
            &ws,
            "task-1",
            r#"{"items":[{"id":"b-1","title":"Parser","desc":"Parse CSV","deps":[],"status":"ready","attempts":0}]}"#,
        );

        assert!(!architect_output_valid(&ws, &task));
        let _ = std::fs::remove_dir_all(&ws);
    }
}
