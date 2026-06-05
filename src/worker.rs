use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::spawn;

pub fn builder_prompt(ws: &Path, parent: &Value, item: &Value) -> String {
    let parent_id = parent["id"].as_str().unwrap_or("");
    let parent_title = parent["title"].as_str().unwrap_or("");
    let parent_desc = parent["desc"].as_str().unwrap_or("");
    let parent_acc = parent["acceptance"]
        .as_str()
        .unwrap_or("the business task is accepted");
    let id = item["id"].as_str().unwrap_or("");
    let title = item["title"].as_str().unwrap_or("");
    let desc = item["desc"].as_str().unwrap_or("");
    let acc = item["acceptance"]
        .as_str()
        .unwrap_or("the change builds and tests pass");
    let prior = crate::inbox::prior_qa_block(ws, id).unwrap_or_default();
    let design_path = ws
        .join(".agentloop/state/tasks")
        .join(parent_id)
        .join("design.md");
    let design = std::fs::read_to_string(&design_path).unwrap_or_default();
    format!(
        r#"You are a BUILDER on an autonomous app build. You are in a git worktree of the project.
Implement exactly this builder item and nothing else.

BUSINESS TASK:
  id: {parent_id}
  title: {parent_title}
  task: {parent_desc}
  acceptance criteria: {parent_acc}

TECHNICAL DESIGN ({design_path}) — implement consistently with this:
{design}

BUILDER ITEM:
  id:    {id}
  title: {title}
  task:  {desc}
  done when: {acc}

Rules:
- ALWAYS use sub-agents: delegate codebase exploration, each implementation chunk, and
  build/test runs to sub-agents, keeping your own context for orchestration — the design,
  the plan, dispatching, and reviewing diffs. This is how you deliver a large item without
  exhausting your context window. If your environment has no sub-agent capability, emulate
  it: work one chunk at a time and never hold more files in context than the current chunk
  needs.
- Make focused commits in this worktree as you go.
- Prove your work by building the project and running its tests. Add or extend tests in the
  project's normal test suite alongside your feature code — that is where verification lives.
- NEVER create standalone verification scripts, runners, audits, or evidence/proof documents.
  Your deliverable is working product code and its tests, nothing else.
- Never create or edit .agentloop/verify.sh, and do not write under .agentloop/ except the
  result/question files named below.
- When finished, write {ws}/.agentloop/results/{id}.json:
  {{"status":"done|failed","summary":"one line","files_changed":["..."]}}
- Open decisions are yours: when you hit a product or technical choice, pick the option
  that best serves the business task and its acceptance criteria, note the decision in
  your result summary, and keep going. Nobody reviews questions live.
- Only as a last resort, if you truly cannot proceed, write {ws}/.agentloop/questions/{id}.json:
  {{"question":"<your question>","context":"<brief context>"}}
  and write the result file with {{"status":"needs_input","summary":"<what you need>"}} instead,
  then stop. An automatic reply will tell you to decide for yourself and you will be
  re-dispatched with that Q&A — so prefer deciding now.{prior}"#,
        parent_id = parent_id,
        parent_title = parent_title,
        parent_desc = parent_desc,
        parent_acc = parent_acc,
        design_path = design_path.display(),
        design = design,
        id = id,
        title = title,
        desc = desc,
        acc = acc,
        ws = ws.display(),
        prior = prior
    )
}

/// Prompt for the conflict-resolver agent. It runs in the MAIN workspace, which is
/// mid-merge with conflicts from `item/<id>`.
pub fn resolver_prompt(ws: &Path, item: &Value) -> String {
    let id = item["id"].as_str().unwrap_or("");
    let title = item["title"].as_str().unwrap_or("");
    let desc = item["desc"].as_str().unwrap_or("");
    format!(
        r#"You are a MERGE-CONFLICT RESOLVER on an autonomous app build. The git repo at
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
        ws = ws.display(),
        id = id,
        title = title,
        desc = desc
    )
}

pub async fn builder_dispatch(
    cfg: &Config,
    ws: &Path,
    parent: &Value,
    item: &Value,
    wt: &Path,
    log: &Path,
    t: Duration,
) -> Result<i32> {
    let prompt = builder_prompt(ws, parent, item);
    spawn::agent_run(cfg, "builder", &prompt, wt, log, t).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builder_prompt_routes_verification_into_the_project_test_suite() {
        let ws = std::env::temp_dir().join("worker-prompt-test");
        let parent = json!({"id":"task-1","title":"Login","desc":"d","acceptance":"a"});
        let item = json!({"id":"task-1-b1","title":"t","desc":"d","acceptance":"a"});

        let p = builder_prompt(&ws, &parent, &item);
        // Items are sized for one builder orchestrating sub-agents; the prompt
        // must mandate delegation so big items fit in the session's context.
        assert!(p.contains("ALWAYS use sub-agents"));
        assert!(p.contains("context window"));
        // Verification belongs in the project's normal test suite, not in
        // standalone verifier scripts / evidence docs (the dominant failure
        // mode observed: 58% of commits were verify churn).
        assert!(p.contains("project's normal test suite"));
        assert!(p.contains("NEVER create standalone verification scripts"));
        assert!(p.contains("Never create or edit .agentloop/verify.sh"));
    }
}
