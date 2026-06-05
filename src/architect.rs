use anyhow::Result;
use serde_json::Value;
use std::collections::HashSet;
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
    let feedback = crate::task_state::redesign_feedback(ws, id);
    let feedback_block = if feedback.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n\nA PREVIOUS ATTEMPT AT THIS TASK WAS REJECTED. The FEATURE failed its gate or customer review — fix the product behavior. Produce a DIFFERENT technical plan whose builders change product code until the acceptance criteria are actually true. Do NOT respond with verification tooling, evidence gathering, or gate changes. Feedback:\n{feedback}\n"
        )
    };
    format!(
        r#"You are the ARCHITECT for one business task in an autonomous app build. Working dir: {ws} (a git repo).

BUSINESS TASK:
  id: {id}
  title: {title}
  task: {desc}
  acceptance criteria: {acc}{feedback_block}

Your job:
1. Inspect the application and design the complete technical solution for this one business task.
2. Write {task_dir}/design.md — a comprehensive technical spec covering the WHOLE business task:
   architecture, data flow, files/components, contracts between the pieces, edge cases, and which
   tests in the project's normal test suite will prove the acceptance criteria.
3. Write {task_dir}/builders.json with as FEW implementation items as possible — ideally ONE.
   Sizing rule: each item is the LARGEST coherent chunk of this task that one builder can
   implement in a single session without exhausting its context window. Builders ALWAYS
   delegate exploration and implementation to sub-agents, keeping only the plan, dispatches,
   and diff reviews in their own context — so size items like a whole feature or subsystem,
   never like a function or a file. Split into multiple items only when a single builder
   genuinely could not deliver the whole thing in one session.
4. Each item is a vertical slice: real product code, usually across multiple files, together
   with its tests in the project's normal test suite. Never plan micro-items; if an item
   would not change product behavior, it does not belong in the plan.
5. NEVER plan an item whose deliverable is verification tooling: no gate scripts, no standalone
   verify/audit/proof runners, no evidence or acceptance documents. Verification lives in the
   project's normal test suite and is written by the same builder as the feature code.
6. Give every builder item a globally unique id prefixed with this task id, such as {id}-b1, {id}-b2.

OUTPUT CONTRACT — you MUST write valid files:
- {task_dir}/design.md must be non-empty.
- {task_dir}/builders.json must be valid JSON shaped like:
  {{"items":[{{"id":"{id}-b1","title":"One substantial vertical slice","desc":"Feature code plus its tests","deps":[],"status":"ready","attempts":0,"acceptance":"User-observable behavior this slice makes true"}}]}}

Do not edit application source code. Do not implement the task. Do not write global backlog files."#,
        ws = ws.display(),
        id = id,
        title = title,
        desc = desc,
        acc = acc,
        task_dir = task_dir,
        feedback_block = feedback_block
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
        Ok(v) if builders_items_valid(&v, id)
    )
}

fn builders_items_valid(v: &Value, parent_id: &str) -> bool {
    let Some(items) = v.get("items").and_then(|items| items.as_array()) else {
        return false;
    };
    let mut seen = HashSet::new();
    !items.is_empty()
        && items.iter().all(|item| {
            let Some(id) = item.get("id").and_then(|id| id.as_str()) else {
                return false;
            };
            builder_item_valid(item, parent_id) && seen.insert(id.to_string())
        })
}

fn builder_item_valid(item: &Value, parent_id: &str) -> bool {
    non_empty_str(item, "id")
        && non_empty_str(item, "title")
        && non_empty_str(item, "desc")
        && non_empty_str(item, "acceptance")
        && item
            .get("id")
            .and_then(|id| id.as_str())
            .map(|id| id.starts_with(&format!("{parent_id}-")))
            .unwrap_or(false)
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
            r#"{"items":[{"id":"task-1-b1","title":"Parser","desc":"Parse CSV","deps":[],"status":"ready","attempts":0,"acceptance":"rows import"}]}"#,
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

    #[test]
    fn architect_output_rejects_duplicate_builder_ids() {
        let ws = tmp_ws("archdupe");
        let task = json!({"id":"task-1"});
        write_plan(
            &ws,
            "task-1",
            r#"{"items":[{"id":"task-1-b1","title":"Parser","desc":"Parse CSV","deps":[],"status":"ready","attempts":0,"acceptance":"rows import"},{"id":"task-1-b1","title":"Importer","desc":"Import rows","deps":["task-1-b1"],"status":"ready","attempts":0,"acceptance":"rows saved"}]}"#,
        );

        assert!(!architect_output_valid(&ws, &task));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn architect_output_rejects_non_prefixed_builder_ids() {
        let ws = tmp_ws("archprefix");
        let task = json!({"id":"task-1"});
        write_plan(
            &ws,
            "task-1",
            r#"{"items":[{"id":"builder-1","title":"Parser","desc":"Parse CSV","deps":[],"status":"ready","attempts":0,"acceptance":"rows import"}]}"#,
        );

        assert!(!architect_output_valid(&ws, &task));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn architect_prompt_demands_substantial_slices_and_forbids_verifier_items() {
        let ws = tmp_ws("archsize");
        let task = json!({"id":"task-1","title":"Login","desc":"Let users log in","acceptance":"user can log in"});

        let p = architect_prompt(&ws, &task);
        // Big spec covering the whole business task, maximal builder items.
        assert!(p.contains("comprehensive technical spec"));
        assert!(p.contains("vertical slice"));
        // Items are sized by one-builder-with-sub-agents capacity: as big as
        // possible without exhausting a single builder session's context window.
        assert!(p.contains("as FEW implementation items as possible"));
        assert!(p.contains("context window"));
        assert!(p.contains("sub-agents"));
        // Verification tooling must never be a builder deliverable: in a real run
        // 80% of all builder items became gate scripts / proof docs / runners.
        assert!(p.contains("NEVER plan an item whose deliverable is verification tooling"));
        assert!(p.contains("project's normal test suite"));

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn architect_redesign_feedback_steers_at_the_feature_not_the_gate() {
        let ws = tmp_ws("archsteer");
        let task = json!({"id":"task-1","title":"Login","desc":"Let users log in","acceptance":"user can log in"});
        crate::task_state::bump_redesign(&ws, "task-1", "verify gate failed for task-1").unwrap();

        let p = architect_prompt(&ws, &task);
        assert!(p.contains("fix the product behavior"));
        assert!(p.contains("Do NOT respond with verification tooling"));

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn architect_prompt_includes_redesign_feedback() {
        let ws = tmp_ws("archfeedback");
        let task = json!({"id":"task-1","title":"Login","desc":"Let users log in","acceptance":"user can log in"});

        // No feedback yet: prompt must not mention a prior attempt.
        let p0 = architect_prompt(&ws, &task);
        assert!(!p0.contains("PREVIOUS ATTEMPT"));

        // After a redesign is recorded, the feedback must appear in the prompt.
        crate::task_state::bump_redesign(&ws, "task-1", "verify.sh failed: missing logout route")
            .unwrap();
        let p1 = architect_prompt(&ws, &task);
        assert!(p1.contains("PREVIOUS ATTEMPT"));
        assert!(p1.contains("missing logout route"));

        let _ = std::fs::remove_dir_all(&ws);
    }
}
