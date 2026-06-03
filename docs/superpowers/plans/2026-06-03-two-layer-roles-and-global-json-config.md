# Two-Layer Roles And Global JSON Config Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace planner-owned build items with a two-layer manager/architect/builder/customer workflow and move routing config to global JSON.

**Architecture:** Keep `.agentloop/state/backlog.json` as the business backlog owned by `manager`. Store per-business-task technical plans in `.agentloop/state/tasks/<task-id>/`, dispatch architect-authored builder subitems through existing worktree/merge mechanics, and add a per-task customer approval gate before a business task can become done. Config is resolved globally as JSON and spawn injects fixed permission flags by tool.

**Tech Stack:** Rust 2021, tokio, clap, serde/serde_json, anyhow, existing fake-agent integration tests, git worktrees.

---

## File Structure

- Modify `Cargo.toml`: remove `serde_yaml` after tests are migrated to JSON.
- Modify `src/config.rs`: parse JSON, provide default global config JSON, resolve global config path, create default global config when appropriate, remove `Role.flags`.
- Modify `src/cli.rs`: remove `templates/config.yaml`, stop bootstrapping `.agentloop/config.yaml`, update help text, resolve config path through `Config`.
- Delete `templates/config.yaml`.
- Modify `src/spawn.rs`: always inject `--dangerously-skip-permissions` for `claude` and `--yolo` for `codex`; remove configurable flags.
- Create `src/manager.rs`: former planner prompt/run contract, renamed and constrained to business tasks only.
- Create `src/architect.rs`: prompt/run/validation for one business task's task-local `design.md` and `builders.json`.
- Modify `src/worker.rs`: builder prompt uses parent business task plus task-local design/subitem; keep resolver prompt.
- Create `src/customer.rs`: prompt/run/validation for silly-customer AC approval.
- Create `src/task_state.rs`: task-local builder plan helpers and customer approval helpers.
- Modify `src/state.rs`: keep business backlog helpers; tolerate legacy `role`; completion checks stay business-layer only.
- Modify `src/orchestrator.rs`: run manager, architect missing plans, dispatch builder subitems, merge builder branches, run gate/customer, mark business tasks done/retry.
- Modify `src/lib.rs`: export new modules and remove `planner`.
- Modify `tests/*.rs` and `tests/common/mod.rs`: migrate YAML config construction to JSON, update fake stubs to manager/architect/builder/customer prompts, add task-state and customer tests.
- Modify `README.md`: document global JSON config and role pipeline.

---

### Task 1: Global JSON Config

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/config.rs`
- Modify: `src/cli.rs`
- Delete: `templates/config.yaml`
- Test: `tests/config_test.rs`
- Test: `tests/cli_bootstrap_test.rs`

- [ ] **Step 1: Write failing config tests**

Replace `tests/config_test.rs` with tests for JSON loading, YAML rejection, and default global config creation using an env override:

```rust
use agentloop::config::Config;
use std::sync::atomic::{AtomicU32, Ordering};

static CFG_CTR: AtomicU32 = AtomicU32::new(0);

fn temp_dir(prefix: &str) -> std::path::PathBuf {
    let n = CFG_CTR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const SAMPLE_JSON: &str = r#"{
  "caps": { "max_iterations": 7, "max_parallel": 2, "item_timeout_sec": 30, "total_budget_sec": 300, "max_attempts": 3 },
  "routing": {
    "manager": { "tool": "claude", "model": "opus", "effort": "high" },
    "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" }
  },
  "defaults": { "role": "builder" }
}"#;

#[test]
fn loads_json_and_resolves_roles() {
    let dir = temp_dir("alcfg-json");
    let path = dir.join("config.json");
    std::fs::write(&path, SAMPLE_JSON).unwrap();

    let cfg = Config::load(&path).unwrap();

    assert_eq!(cfg.resolve_role("manager").as_deref(), Some("manager"));
    assert_eq!(cfg.resolve_role("nonexistent").as_deref(), Some("builder"));
    assert_eq!(cfg.role_field("manager", "tool").as_deref(), Some("claude"));
    assert_eq!(cfg.role_field("manager", "model").as_deref(), Some("opus"));
    assert_eq!(cfg.role_field("manager", "flags"), None);
    assert_eq!(cfg.max_iterations(), 7);
    assert_eq!(cfg.max_parallel(), 2);
    assert_eq!(cfg.max_attempts(), 3);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn yaml_config_fails_with_migration_message() {
    let dir = temp_dir("alcfg-yaml");
    let path = dir.join("config.yaml");
    std::fs::write(&path, "routing:\n  manager: { tool: claude }\n").unwrap();

    let err = Config::load(&path).unwrap_err().to_string();
    assert!(err.contains("config must be JSON"), "{err}");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ensure_default_creates_global_json() {
    let dir = temp_dir("alcfg-default");
    let path = dir.join("agentloop").join("config.json");

    let resolved = Config::ensure_default_config(&path).unwrap();
    assert_eq!(resolved, path);
    assert!(path.exists());

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"manager\""));
    assert!(text.contains("\"architect\""));
    assert!(text.contains("\"customer\""));
    assert!(!text.contains("\"flags\""));

    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.resolve_role("builder").as_deref(), Some("builder"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn caps_default_when_absent() {
    let dir = temp_dir("alcfg-empty");
    let path = dir.join("config.json");
    std::fs::write(&path, r#"{"routing":{},"defaults":{}}"#).unwrap();

    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.max_iterations(), 25);
    assert_eq!(cfg.item_timeout_sec(), 1200);
    assert_eq!(cfg.resolve_role("anything"), None);

    let _ = std::fs::remove_dir_all(dir);
}
```

Update `tests/cli_bootstrap_test.rs` so bootstrap no longer creates workspace config:

```rust
use agentloop::cli;

#[test]
fn bootstrap_creates_state_and_git_without_workspace_config() {
    let ws = std::env::temp_dir().join(format!(
        "alboot-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    cli::bootstrap_workspace(&ws, "build a todo app").unwrap();

    assert!(ws.join(".git").exists(), "git repo initialized");
    assert!(ws.join(".agentloop/state/goal.md").exists());
    assert_eq!(
        std::fs::read_to_string(ws.join(".agentloop/state/goal.md"))
            .unwrap()
            .trim(),
        "build a todo app"
    );
    assert!(ws.join(".agentloop/state/backlog.json").exists());
    assert!(ws.join(".agentloop/state/master.md").exists());
    assert!(!ws.join(".agentloop/config.yaml").exists());
    assert!(!ws.join(".agentloop/config.json").exists());
    let gi = std::fs::read_to_string(ws.join(".gitignore")).unwrap();
    assert!(gi.contains(".agentloop/"));
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test config_test cli_bootstrap_test
```

Expected: compile or test failure because `ensure_default_config` does not exist, config still parses YAML, and `bootstrap_workspace` still takes a config parameter and creates `.agentloop/config.yaml`.

- [ ] **Step 3: Implement JSON config**

In `Cargo.toml`, remove:

```toml
serde_yaml = "0.9"
```

In `src/config.rs`, remove `flags` from `Role`, switch to JSON loading, add default JSON and path helpers:

```rust
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_JSON: &str = r#"{
  "caps": {
    "max_iterations": 25,
    "max_parallel": 3,
    "item_timeout_sec": 1200,
    "total_budget_sec": 21600,
    "max_attempts": 3
  },
  "routing": {
    "manager": { "tool": "claude", "model": "opus", "effort": "high" },
    "architect": { "tool": "claude", "model": "opus", "effort": "high" },
    "builder": { "tool": "codex", "model": "gpt-5.5", "effort": "high" },
    "customer": { "tool": "claude", "model": "sonnet", "effort": "medium" },
    "resolver": { "tool": "claude", "model": "sonnet", "effort": "medium" }
  },
  "defaults": { "role": "builder" }
}
"#;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Caps {
    pub max_iterations: Option<u32>,
    pub max_parallel: Option<u32>,
    pub item_timeout_sec: Option<u64>,
    pub total_budget_sec: Option<u64>,
    pub max_attempts: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Role {
    pub tool: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Defaults {
    pub role: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub caps: Caps,
    #[serde(default)]
    pub routing: BTreeMap<String, Role>,
    #[serde(default)]
    pub defaults: Defaults,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| {
            if looks_like_yaml(&text) {
                anyhow::anyhow!(
                    "config must be JSON; migrate config.yaml to config.json ({})",
                    path.display()
                )
            } else {
                anyhow::anyhow!("parse config JSON {}: {e}", path.display())
            }
        })
    }

    pub fn default_config_path() -> PathBuf {
        if let Ok(path) = std::env::var("AGENTLOOP_CONFIG") {
            if !path.trim().is_empty() {
                return PathBuf::from(path);
            }
        }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        #[cfg(target_os = "macos")]
        {
            return home.join("Library/Application Support/agentloop/config.json");
        }
        #[cfg(not(target_os = "macos"))]
        {
            if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
                if !xdg.trim().is_empty() {
                    return PathBuf::from(xdg).join("agentloop/config.json");
                }
            }
            home.join(".config/agentloop/config.json")
        }
    }

    pub fn ensure_default_config(path: &Path) -> Result<PathBuf> {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, DEFAULT_CONFIG_JSON)?;
        Ok(path.to_path_buf())
    }

    pub fn resolve_role(&self, role: &str) -> Option<String> {
        if self.routing.contains_key(role) {
            Some(role.to_string())
        } else {
            self.defaults.role.clone()
        }
    }

    pub fn role_field(&self, role: &str, field: &str) -> Option<String> {
        let r = self.routing.get(role)?;
        let v = match field {
            "tool" => r.tool.clone(),
            "model" => r.model.clone(),
            "effort" => r.effort.clone(),
            "flags" => None,
            _ => None,
        };
        v.filter(|s| !s.is_empty())
    }

    pub fn max_iterations(&self) -> u32 { self.caps.max_iterations.unwrap_or(25) }
    pub fn max_parallel(&self) -> u32 { self.caps.max_parallel.unwrap_or(3) }
    pub fn item_timeout_sec(&self) -> u64 { self.caps.item_timeout_sec.unwrap_or(1200) }
    pub fn total_budget_sec(&self) -> u64 { self.caps.total_budget_sec.unwrap_or(21600) }
    pub fn max_attempts(&self) -> u32 { self.caps.max_attempts.unwrap_or(3) }
}

fn looks_like_yaml(text: &str) -> bool {
    let trimmed = text.trim_start();
    !trimmed.starts_with('{') && !trimmed.starts_with('[') && trimmed.contains(':')
}
```

In `src/cli.rs`, remove `TEMPLATE_CONFIG`, change `bootstrap_workspace` signature, and resolve config in `run`:

```rust
/// config.json path (default: global config; see README)
#[arg(long)]
config: Option<PathBuf>,
```

```rust
pub fn bootstrap_workspace(ws: &Path, goal: &str) -> Result<()> {
    // same workspace/state/git setup as today, but no config path creation
    // function ends with Ok(()) instead of Ok(cfg_path)
}
```

```rust
bootstrap_workspace(&ws, goal_arg.as_deref().unwrap_or(""))?;
let cfg_path = match args.config.as_deref() {
    Some(path) => {
        if !path.exists() {
            bail!("config path does not exist: {}", path.display());
        }
        path.to_path_buf()
    }
    None => Config::ensure_default_config(&Config::default_config_path())?,
};
```

Delete `templates/config.yaml`.

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test config_test cli_bootstrap_test
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/config.rs src/cli.rs tests/config_test.rs tests/cli_bootstrap_test.rs templates/config.yaml
git commit -m "feat: use global JSON config"
```

---

### Task 2: Fixed Permission Flags In Spawn

**Files:**
- Modify: `src/spawn.rs`
- Modify: `tests/spawn_test.rs`

- [ ] **Step 1: Write failing spawn argv tests**

Update the test config helper in `tests/spawn_test.rs` to JSON:

```rust
fn cfg() -> Config {
    serde_json::from_str(r#"{
      "routing": {
        "manager": { "tool": "claude", "model": "opus", "effort": "high" },
        "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" }
      },
      "defaults": { "role": "builder" }
    }"#).unwrap()
}
```

Replace `claude_argv` and `codex_argv`:

```rust
#[test]
fn claude_argv_injects_skip_permissions() {
    let a = spawn::build_argv(&cfg(), "manager", "HELLO").unwrap();
    assert_eq!(a, vec![
        "claude","-p","HELLO",
        "--output-format","stream-json","--verbose",
        "--model","opus","--effort","high",
        "--dangerously-skip-permissions",
    ]);
}

#[test]
fn codex_argv_injects_yolo() {
    let a = spawn::build_argv(&cfg(), "builder", "DOIT").unwrap();
    assert_eq!(a, vec![
        "codex","exec","DOIT",
        "-m","gpt-5",
        "-c","model_reasoning_effort=high",
        "--yolo",
    ]);
}
```

Update `unknown_tool_errors`:

```rust
#[test]
fn unknown_tool_errors() {
    let c: Config = serde_json::from_str(r#"{"routing":{"x":{"tool":"nope"}},"defaults":{}}"#).unwrap();
    assert!(spawn::build_argv(&c, "x", "p").is_err());
}
```

Update async tests to use `"manager"` instead of `"planner"` when calling `agent_run`.

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test spawn_test
```

Expected: FAIL because spawn still reads configurable flags and does not inject fixed flags.

- [ ] **Step 3: Implement fixed flags**

In `src/spawn.rs`, remove:

```rust
let flags = cfg.role_field(&rrole, "flags");
```

Delete the block that splits configured flags. Add fixed flags after model/effort:

```rust
match tool.as_str() {
    "claude" => argv.push("--dangerously-skip-permissions".into()),
    "codex" => argv.push("--yolo".into()),
    _ => {}
}
```

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test spawn_test
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/spawn.rs tests/spawn_test.rs
git commit -m "feat: inject agent permission flags"
```

---

### Task 3: Role Prompt Modules

**Files:**
- Create: `src/manager.rs`
- Create: `src/architect.rs`
- Create: `src/customer.rs`
- Modify: `src/worker.rs`
- Modify: `src/lib.rs`
- Delete: `src/planner.rs`
- Test: `tests/planner_worker_test.rs`
- Test: `tests/worker_prompt_test.rs`

- [ ] **Step 1: Replace prompt tests**

Rename `tests/planner_worker_test.rs` to `tests/roles_prompt_test.rs` and use this content:

```rust
use agentloop::{architect, customer, manager, worker};
use serde_json::json;
use std::path::PathBuf;

fn ws_with_state() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "alroles-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let st = dir.join(".agentloop/state");
    std::fs::create_dir_all(&st).unwrap();
    std::fs::write(st.join("goal.md"), "build a thing").unwrap();
    std::fs::write(st.join("master.md"), "# status").unwrap();
    std::fs::write(st.join("backlog.json"), r#"{"items":[]}"#).unwrap();
    dir
}

#[test]
fn manager_prompt_is_business_only() {
    let ws = ws_with_state();
    let p = manager::manager_prompt(&ws, 3);
    assert!(p.contains("You are the MANAGER"));
    assert!(p.contains("business tasks only"));
    assert!(p.contains("backlog.json"));
    assert!(p.contains("master.md"));
    assert!(!p.contains("design.md"), "manager must not own technical design");
    assert!(!p.contains("builders.json"), "manager must not emit builder subitems");
    let _ = std::fs::remove_dir_all(ws);
}

#[test]
fn manager_prompt_includes_pending_requests() {
    let ws = ws_with_state();
    agentloop::requests::append(&ws, "add a --due flag").unwrap();
    let p = manager::manager_prompt(&ws, 3);
    assert!(p.contains("PENDING USER REQUESTS"));
    assert!(p.contains("add a --due flag"));
    let _ = std::fs::remove_dir_all(ws);
}

#[test]
fn architect_prompt_writes_task_plan() {
    let ws = ws_with_state();
    let task = json!({
        "id": "task-1",
        "title": "Checkout",
        "desc": "User can pay",
        "deps": [],
        "status": "ready",
        "attempts": 0,
        "acceptance": "Payment succeeds"
    });
    let p = architect::architect_prompt(&ws, &task);
    assert!(p.contains("You are the ARCHITECT"));
    assert!(p.contains("task-1"));
    assert!(p.contains(".agentloop/state/tasks/task-1/design.md"));
    assert!(p.contains(".agentloop/state/tasks/task-1/builders.json"));
    assert!(p.contains("Do not edit application source code"));
    let _ = std::fs::remove_dir_all(ws);
}

#[test]
fn builder_prompt_uses_parent_task_and_design() {
    let ws = ws_with_state();
    std::fs::create_dir_all(ws.join(".agentloop/state/tasks/task-1")).unwrap();
    std::fs::write(ws.join(".agentloop/state/tasks/task-1/design.md"), "Use SQLite.").unwrap();
    let parent = json!({"id":"task-1","title":"Checkout","desc":"User can pay","acceptance":"Payment succeeds"});
    let item = json!({"id":"task-1-b1","title":"DB","desc":"Add schema","acceptance":"Schema exists"});
    let p = worker::builder_prompt(&ws, &parent, &item);
    assert!(p.contains("You are a BUILDER"));
    assert!(p.contains("BUSINESS TASK"));
    assert!(p.contains("Checkout"));
    assert!(p.contains("TECHNICAL DESIGN"));
    assert!(p.contains("Use SQLite."));
    assert!(p.contains(".agentloop/results/task-1-b1.json"));
    let _ = std::fs::remove_dir_all(ws);
}

#[test]
fn customer_prompt_is_ac_only() {
    let ws = ws_with_state();
    std::fs::write(ws.join(".agentloop/state/last_gate.txt"), "ok").unwrap();
    let task = json!({"id":"task-1","title":"Checkout","desc":"User can pay","acceptance":"Payment succeeds"});
    let p = customer::customer_prompt(&ws, &task);
    assert!(p.contains("You are the SILLY CUSTOMER"));
    assert!(p.contains("acceptance criteria"));
    assert!(p.contains("Payment succeeds"));
    assert!(p.contains(".agentloop/state/tasks/task-1/customer.json"));
    assert!(p.contains(".agentloop/results/task-1-customer.json"));
    let _ = std::fs::remove_dir_all(ws);
}
```

Keep `tests/worker_prompt_test.rs` for resolver, but update imports if `worker_prompt` is renamed.

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test roles_prompt_test worker_prompt_test
```

Expected: compile failure because new modules and prompt functions do not exist.

- [ ] **Step 3: Implement `src/manager.rs`**

Move planner run logic into manager with a business-only prompt:

```rust
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
    format!(r#"You are the MANAGER for an autonomous app build. Working dir: {ws} (a git repo).
You manage business tasks only. Do not write technical designs, do not create builders.json,
and do not decompose work into implementation subitems.

GOAL:
{goal}

CURRENT master.md:
{master}

CURRENT business backlog.json:
{backlog}

Your job each round:
1. Read previous results in .agentloop/results/, latest gate output in
   .agentloop/state/last_gate.txt, and customer feedback in .agentloop/state/tasks/*/customer.json.
2. Maintain .agentloop/state/backlog.json as business tasks only: id, title, desc,
   deps, status, attempts, and acceptance.
3. Maintain .agentloop/state/master.md as a human-readable business status board.
4. For any task nearing attempts={max_attempts}, rewrite it into clearer business
   acceptance criteria or mark it failed instead of repeating the same request.
5. Do not mark a business task done unless customer approval exists for that task.

OUTPUT CONTRACT — overwrite .agentloop/state/backlog.json with valid JSON:
{{"items":[{{"id":"task-1","title":"...","desc":"...","deps":[],"status":"ready|in_progress|done|failed|blocked","attempts":0,"acceptance":"..."}}]}}
Also rewrite .agentloop/state/master.md.
Do not print the JSON to stdout; write the files.{requests}"#,
        ws = ws.display(),
        goal = goal,
        master = master,
        backlog = backlog,
        max_attempts = max_attempts,
        requests = requests)
}

pub async fn manager_run(cfg: &Config, ws: &Path, log: &Path, t: Duration) -> Result<bool> {
    let bk = ws.join(".agentloop/state/backlog.json");
    let prompt = manager_prompt(ws, cfg.max_attempts());
    spawn::agent_run(cfg, "manager", &prompt, ws, log, t).await?;
    if state::backlog_valid(&bk) {
        let _ = crate::requests::mark_all_consumed(ws);
        return Ok(true);
    }
    eprintln!("manager produced invalid backlog.json; re-prompting once");
    let retry = format!("{prompt}\nNOTE: your previous backlog.json was invalid JSON. Write valid JSON this time.");
    spawn::agent_run(cfg, "manager", &retry, ws, log, t).await?;
    let ok = state::backlog_valid(&bk);
    if ok {
        let _ = crate::requests::mark_all_consumed(ws);
    }
    Ok(ok)
}
```

- [ ] **Step 4: Implement `src/architect.rs`**

```rust
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::{spawn, task_state};

pub fn architect_prompt(ws: &Path, task: &Value) -> String {
    let id = task["id"].as_str().unwrap_or("");
    let title = task["title"].as_str().unwrap_or("");
    let desc = task["desc"].as_str().unwrap_or("");
    let acc = task["acceptance"].as_str().unwrap_or("");
    let dir = ws.join(".agentloop/state/tasks").join(id);
    format!(r#"You are the ARCHITECT for one business task in an autonomous app build.
Working dir: {ws} (a git repo).

BUSINESS TASK:
  id: {id}
  title: {title}
  description: {desc}
  acceptance criteria: {acc}

Create the technical design for this business task, then split it into N builder
subitems that can run in parallel when dependencies allow.

Write:
- {design}
- {builders}

builders.json contract:
{{"items":[{{"id":"{id}-b1","title":"...","desc":"...","deps":[],"status":"ready","attempts":0,"acceptance":"..."}}]}}

Rules:
- Do not edit application source code.
- Make builder subitems focused and independently verifiable.
- Include concrete acceptance criteria for each builder subitem.
- Do not print JSON to stdout; write the files."#,
        ws = ws.display(),
        id = id,
        title = title,
        desc = desc,
        acc = acc,
        design = dir.join("design.md").display(),
        builders = dir.join("builders.json").display())
}

pub async fn architect_run(cfg: &Config, ws: &Path, task: &Value, log: &Path, t: Duration) -> Result<bool> {
    let id = task["id"].as_str().unwrap_or("");
    task_state::ensure_task_dir(ws, id)?;
    let prompt = architect_prompt(ws, task);
    spawn::agent_run(cfg, "architect", &prompt, ws, log, t).await?;
    if task_state::builder_plan_valid(ws, id) {
        return Ok(true);
    }
    let retry = format!("{prompt}\nNOTE: builders.json was invalid. Write valid JSON with an items array.");
    spawn::agent_run(cfg, "architect", &retry, ws, log, t).await?;
    Ok(task_state::builder_plan_valid(ws, id))
}
```

- [ ] **Step 5: Implement builder and customer prompts**

In `src/worker.rs`, rename `worker_prompt` to `builder_prompt` and include parent task:

```rust
pub fn builder_prompt(ws: &Path, parent: &Value, item: &Value) -> String {
    let parent_id = parent["id"].as_str().unwrap_or("");
    let parent_title = parent["title"].as_str().unwrap_or("");
    let parent_desc = parent["desc"].as_str().unwrap_or("");
    let parent_acc = parent["acceptance"].as_str().unwrap_or("");
    let id = item["id"].as_str().unwrap_or("");
    let title = item["title"].as_str().unwrap_or("");
    let desc = item["desc"].as_str().unwrap_or("");
    let acc = item["acceptance"].as_str().unwrap_or("the change builds and tests pass");
    let prior = crate::inbox::prior_qa_block(ws, id).unwrap_or_default();
    let design = std::fs::read_to_string(
        ws.join(".agentloop/state/tasks").join(parent_id).join("design.md")
    ).unwrap_or_default();
    format!(r#"You are a BUILDER on an autonomous app build. You are in a git worktree.

BUSINESS TASK:
  id: {parent_id}
  title: {parent_title}
  description: {parent_desc}
  acceptance criteria: {parent_acc}

TECHNICAL DESIGN:
{design}

IMPLEMENTATION SUBITEM:
  id:    {id}
  title: {title}
  task:  {desc}
  done when: {acc}

Rules:
- Implement exactly this subitem and nothing else.
- Make focused commits in this worktree as you go.
- Verify your work against the subitem acceptance criteria before finishing.
- When finished, write {ws}/.agentloop/results/{id}.json:
  {{"status":"done|failed","summary":"one line","files_changed":["..."]}}
- If blocked needing a user decision, write {ws}/.agentloop/questions/{id}.json:
  {{"question":"<your question>","context":"<brief context>"}}
  and write the result file with {{"status":"needs_input","summary":"<what you need>"}} instead.{prior}"#,
        parent_id = parent_id,
        parent_title = parent_title,
        parent_desc = parent_desc,
        parent_acc = parent_acc,
        design = design,
        id = id,
        title = title,
        desc = desc,
        acc = acc,
        ws = ws.display(),
        prior = prior)
}
```

Add a new dispatch function:

```rust
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
```

Create `src/customer.rs`:

```rust
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::{spawn, task_state};

pub fn customer_prompt(ws: &Path, task: &Value) -> String {
    let id = task["id"].as_str().unwrap_or("");
    let title = task["title"].as_str().unwrap_or("");
    let desc = task["desc"].as_str().unwrap_or("");
    let acc = task["acceptance"].as_str().unwrap_or("");
    let gate = std::fs::read_to_string(ws.join(".agentloop/state/last_gate.txt")).unwrap_or_default();
    format!(r#"You are the SILLY CUSTOMER for one business task.
You only care whether the acceptance criteria are satisfied. Ignore internal architecture,
code style, test strategy, and implementation details unless they affect the acceptance criteria.

BUSINESS TASK:
  id: {id}
  title: {title}
  description: {desc}
  acceptance criteria: {acc}

LATEST VERIFY OUTPUT:
{gate}

Write {customer_json}:
{{"status":"approved","summary":"..."}}
or:
{{"status":"rejected","summary":"...","missing_acceptance":["..."]}}

Also write {result_json} with the same status and summary.
Do not print JSON to stdout; write the files."#,
        id = id,
        title = title,
        desc = desc,
        acc = acc,
        gate = gate,
        customer_json = ws.join(".agentloop/state/tasks").join(id).join("customer.json").display(),
        result_json = ws.join(".agentloop/results").join(format!("{id}-customer.json")).display())
}

pub async fn customer_run(cfg: &Config, ws: &Path, task: &Value, log: &Path, t: Duration) -> Result<bool> {
    let id = task["id"].as_str().unwrap_or("");
    task_state::ensure_task_dir(ws, id)?;
    let prompt = customer_prompt(ws, task);
    spawn::agent_run(cfg, "customer", &prompt, ws, log, t).await?;
    Ok(task_state::customer_approved(ws, id))
}
```

Update `src/lib.rs`:

```rust
pub mod architect;
pub mod customer;
pub mod manager;
pub mod task_state;
```

Remove:

```rust
pub mod planner;
```

Delete `src/planner.rs`.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test roles_prompt_test worker_prompt_test
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/manager.rs src/architect.rs src/customer.rs src/worker.rs src/lib.rs tests/roles_prompt_test.rs tests/worker_prompt_test.rs
git rm src/planner.rs tests/planner_worker_test.rs
git commit -m "feat: add manager architect customer prompts"
```

---

### Task 4: Task-Local Builder State

**Files:**
- Create: `src/task_state.rs`
- Modify: `src/state.rs`
- Test: `tests/task_state_test.rs`
- Test: `tests/state_test.rs`

- [ ] **Step 1: Write failing task state tests**

Create `tests/task_state_test.rs`:

```rust
use agentloop::task_state;
use serde_json::json;

fn ws() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "altask-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join(".agentloop/state/tasks/task-1")).unwrap();
    dir
}

#[test]
fn validates_builder_plan() {
    let ws = ws();
    let dir = ws.join(".agentloop/state/tasks/task-1");
    std::fs::write(dir.join("design.md"), "Design").unwrap();
    std::fs::write(dir.join("builders.json"), r#"{"items":[{"id":"task-1-b1","title":"T","desc":"D","deps":[],"status":"ready","attempts":0,"acceptance":"A"}]}"#).unwrap();

    assert!(task_state::builder_plan_valid(&ws, "task-1"));
    assert_eq!(task_state::ready_builders(&ws, "task-1", 4).unwrap(), vec!["task-1-b1"]);

    let _ = std::fs::remove_dir_all(ws);
}

#[test]
fn builder_deps_must_be_done() {
    let ws = ws();
    let dir = ws.join(".agentloop/state/tasks/task-1");
    std::fs::write(dir.join("design.md"), "Design").unwrap();
    std::fs::write(dir.join("builders.json"), r#"{"items":[
      {"id":"task-1-b1","title":"T1","desc":"D","deps":[],"status":"done","attempts":1,"acceptance":"A"},
      {"id":"task-1-b2","title":"T2","desc":"D","deps":["task-1-b1"],"status":"ready","attempts":0,"acceptance":"A"}
    ]}"#).unwrap();

    assert_eq!(task_state::ready_builders(&ws, "task-1", 4).unwrap(), vec!["task-1-b2"]);

    let _ = std::fs::remove_dir_all(ws);
}

#[test]
fn customer_approval_is_read_from_task_local_state() {
    let ws = ws();
    assert!(!task_state::customer_approved(&ws, "task-1"));
    task_state::write_customer(&ws, "task-1", &json!({"status":"approved","summary":"ok"})).unwrap();
    assert!(task_state::customer_approved(&ws, "task-1"));
    task_state::write_customer(&ws, "task-1", &json!({"status":"rejected","summary":"missing","missing_acceptance":["A"]})).unwrap();
    assert!(!task_state::customer_approved(&ws, "task-1"));
    let _ = std::fs::remove_dir_all(ws);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test task_state_test state_test
```

Expected: compile failure because `task_state` does not exist.

- [ ] **Step 3: Implement task state helpers**

Create `src/task_state.rs`:

```rust
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn task_dir(ws: &Path, task_id: &str) -> PathBuf {
    ws.join(".agentloop/state/tasks").join(task_id)
}

pub fn ensure_task_dir(ws: &Path, task_id: &str) -> Result<PathBuf> {
    let dir = task_dir(ws, task_id);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn builders_path(ws: &Path, task_id: &str) -> PathBuf {
    task_dir(ws, task_id).join("builders.json")
}

pub fn customer_path(ws: &Path, task_id: &str) -> PathBuf {
    task_dir(ws, task_id).join("customer.json")
}

pub fn read_builders(ws: &Path, task_id: &str) -> Result<Value> {
    let path = builders_path(ws, task_id);
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

pub fn write_builders(ws: &Path, task_id: &str, v: &Value) -> Result<()> {
    let dir = ensure_task_dir(ws, task_id)?;
    std::fs::write(dir.join("builders.json"), serde_json::to_vec_pretty(v)?)?;
    Ok(())
}

pub fn builder_plan_valid(ws: &Path, task_id: &str) -> bool {
    let design_ok = task_dir(ws, task_id)
        .join("design.md")
        .metadata()
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    if !design_ok {
        return false;
    }
    matches!(read_builders(ws, task_id), Ok(v) if v.get("items").map(|i| i.is_array()).unwrap_or(false))
}

pub fn item<'a>(v: &'a Value, id: &str) -> Option<&'a Value> {
    v["items"].as_array()?.iter().find(|i| i["id"] == json!(id))
}

pub fn ready_builders(ws: &Path, task_id: &str, max_parallel: usize) -> Result<Vec<String>> {
    let v = read_builders(ws, task_id)?;
    let empty = vec![];
    let items = v["items"].as_array().unwrap_or(&empty);
    let done: HashSet<&str> = items
        .iter()
        .filter(|i| i["status"] == "done")
        .filter_map(|i| i["id"].as_str())
        .collect();
    let mut out = Vec::new();
    for it in items {
        let id = match it["id"].as_str() { Some(i) => i, None => continue };
        let dispatchable = match it["status"].as_str() {
            Some("ready") => true,
            Some("blocked") => !crate::inbox::has_question(ws, id),
            _ => false,
        };
        if !dispatchable {
            continue;
        }
        let deps_ok = match it.get("deps").and_then(|d| d.as_array()) {
            Some(deps) => deps.iter().all(|d| d.as_str().map(|s| done.contains(s)).unwrap_or(false)),
            None => true,
        };
        if deps_ok {
            out.push(id.to_string());
        }
    }
    out.truncate(max_parallel);
    Ok(out)
}

pub fn open_builder_count(ws: &Path, task_id: &str) -> Result<i64> {
    let v = read_builders(ws, task_id)?;
    let empty = vec![];
    Ok(v["items"].as_array().unwrap_or(&empty).iter()
        .filter(|i| matches!(i["status"].as_str(), Some("ready") | Some("in_progress") | Some("blocked")))
        .count() as i64)
}

pub fn all_builders_done(ws: &Path, task_id: &str) -> Result<bool> {
    let v = read_builders(ws, task_id)?;
    let empty = vec![];
    let items = v["items"].as_array().unwrap_or(&empty);
    Ok(!items.is_empty() && items.iter().all(|i| i["status"] == "done"))
}

pub fn set_builder_status(ws: &Path, task_id: &str, builder_id: &str, status: &str, note: &str) -> Result<()> {
    let mut v = read_builders(ws, task_id)?;
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if it["id"] == json!(builder_id) {
                it["status"] = json!(status);
                if !note.is_empty() {
                    it["notes"] = json!(note);
                }
            }
        }
    }
    write_builders(ws, task_id, &v)
}

pub fn increment_builder_attempts(ws: &Path, task_id: &str, builder_id: &str) -> Result<()> {
    let mut v = read_builders(ws, task_id)?;
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if it["id"] == json!(builder_id) {
                let cur = it.get("attempts").and_then(|a| a.as_u64()).unwrap_or(0);
                it["attempts"] = json!(cur + 1);
            }
        }
    }
    write_builders(ws, task_id, &v)
}

pub fn write_customer(ws: &Path, task_id: &str, v: &Value) -> Result<()> {
    let dir = ensure_task_dir(ws, task_id)?;
    std::fs::write(dir.join("customer.json"), serde_json::to_vec_pretty(v)?)?;
    Ok(())
}

pub fn customer_approved(ws: &Path, task_id: &str) -> bool {
    std::fs::read_to_string(customer_path(ws, task_id))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(|s| s == "approved"))
        .unwrap_or(false)
}
```

In `src/state.rs`, keep existing backlog helpers. Update comments from planner dependency-blocking to manager/business wording, but keep behavior for legacy blocked-without-question state.

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test task_state_test state_test
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/task_state.rs src/state.rs src/lib.rs tests/task_state_test.rs tests/state_test.rs
git commit -m "feat: add task-local builder state"
```

---

### Task 5: Two-Layer Orchestrator

**Files:**
- Modify: `src/orchestrator.rs`
- Modify: `src/events.rs` only if labels need role wording updates
- Test: `tests/loop_test.rs`
- Test: `tests/loop_addtask_test.rs`
- Test: `tests/loop_needs_input_test.rs`
- Test: `tests/loop_resolver_test.rs`
- Test: `tests/common/mod.rs`

- [ ] **Step 1: Update fake-agent integration stub**

In `tests/common/mod.rs`, change stub prompt matching from `PLANNER`/`WORKER` to `MANAGER`/`ARCHITECT`/`BUILDER`/`SILLY CUSTOMER`. The minimal happy-path stub should:

```bash
case "$prompt" in
  *MANAGER*)
    if [ ! -s "$ws_state/backlog.json" ] || grep -q '"items":\[\]' "$ws_state/backlog.json"; then
      echo '{"items":[{"id":"task-1","title":"f","desc":"d","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/backlog.json"
      printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$WS/.agentloop/verify.sh"; chmod +x "$WS/.agentloop/verify.sh"
    fi
    echo "# updated" > "$ws_state/master.md"
    ;;
  *ARCHITECT*)
    mkdir -p "$ws_state/tasks/task-1"
    echo "Build made.txt." > "$ws_state/tasks/task-1/design.md"
    echo '{"items":[{"id":"task-1-b1","title":"make file","desc":"create made.txt","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/tasks/task-1/builders.json"
    ;;
  *BUILDER*)
    echo made > "$PWD/made.txt"; git add -A; git commit -qm "builder" 2>/dev/null
    echo '{"status":"done","summary":"made file","files_changed":["made.txt"]}' > "$res/task-1-b1.json"
    ;;
  *SILLY\ CUSTOMER*)
    mkdir -p "$ws_state/tasks/task-1"
    echo '{"status":"approved","summary":"ok"}' > "$ws_state/tasks/task-1/customer.json"
    echo '{"status":"approved","summary":"ok"}' > "$res/task-1-customer.json"
    ;;
esac
```

Update config helpers in loop tests to JSON with roles `manager`, `architect`, `builder`, `customer`, `resolver`.

- [ ] **Step 2: Write customer rejection test**

Add `tests/loop_customer_test.rs`:

```rust
mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::{orchestrator, state};
use std::sync::Arc;

fn cfg() -> Config {
    serde_json::from_str(r#"{
      "caps": { "max_iterations": 3, "max_parallel": 2, "item_timeout_sec": 30, "total_budget_sec": 300, "max_attempts": 3 },
      "routing": {
        "manager": { "tool": "claude", "model": "opus", "effort": "high" },
        "architect": { "tool": "claude", "model": "opus", "effort": "high" },
        "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" },
        "customer": { "tool": "claude", "model": "sonnet", "effort": "medium" },
        "resolver": { "tool": "claude", "model": "sonnet", "effort": "medium" }
      },
      "defaults": { "role": "builder" }
    }"#).unwrap()
}

#[tokio::test]
async fn customer_rejection_keeps_business_task_open() {
    let ws = common::init_ws_with_rejecting_customer_stub();
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);

    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);
    orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();

    let bk = ws.join(".agentloop/state/backlog.json");
    let v = state::read(&bk).unwrap();
    let task = state::item(&v, "task-1").unwrap();
    assert_eq!(task["status"], "ready");
    assert!(task["notes"].as_str().unwrap_or("").contains("missing"));
    assert_eq!(state::open_count(&bk).unwrap(), 1);

    for k in ["FAKE_AGENT","FAKE_AGENT_BIN","WS"] {
        std::env::remove_var(k);
    }
    let _ = std::fs::remove_dir_all(&ws);
}
```

Add `init_ws_with_rejecting_customer_stub` to `tests/common/mod.rs` by copying the happy path stub and making the `SILLY CUSTOMER` case write:

```bash
echo '{"status":"rejected","summary":"missing card payment","missing_acceptance":["card payment"]}' > "$ws_state/tasks/task-1/customer.json"
echo '{"status":"rejected","summary":"missing card payment"}' > "$res/task-1-customer.json"
```

- [ ] **Step 3: Run tests to verify failure**

Run:

```bash
cargo test loop_test loop_customer_test loop_addtask_test loop_needs_input_test loop_resolver_test
```

Expected: compile/test failure because orchestrator still calls planner and worker items directly.

- [ ] **Step 4: Implement orchestration helpers**

In `src/orchestrator.rs`, change imports:

```rust
use crate::{architect, customer, manager, spawn, state, task_state, worker, worktree};
```

Replace planner dispatch with manager dispatch:

```rust
let mrole = cfg.resolve_role("manager").unwrap_or_default();
let mtool = cfg.role_field(&mrole, "tool").unwrap_or_default();
let mmodel = cfg.role_field(&mrole, "model").unwrap_or_default();
reporter.dispatch("manager", "managing business tasks", &mtool, &mmodel, Some(&ldir.join("manager.log")));
let ok = manager::manager_run(cfg, ws, &ldir.join("manager.log"), itimeout).await?;
if !ok {
    eprintln!("manager failed/invalid");
    anyhow::bail!("manager invalid");
}
reporter.status("manager", "done", &mtool, &mmodel);
```

Add a small struct near the top:

```rust
#[derive(Clone)]
struct BuilderDispatch {
    task_id: String,
    builder_id: String,
    parent: Value,
    item: Value,
}
```

After manager runs, select business tasks:

```rust
let business_ready = state::ready_items(&bk, ws, maxpar)?;
let mut builder_ready: Vec<BuilderDispatch> = Vec::new();
for task_id in business_ready {
    let v = state::read(&bk)?;
    let parent = match state::item(&v, &task_id) {
        Some(i) => i.clone(),
        None => continue,
    };
    if !task_state::builder_plan_valid(ws, &task_id) {
        state::set_status(&bk, &task_id, "in_progress", "architect designing")?;
        let log = ldir.join(format!("architect-{task_id}.log"));
        let arole = cfg.resolve_role("architect").unwrap_or_default();
        let atool = cfg.role_field(&arole, "tool").unwrap_or_default();
        let amodel = cfg.role_field(&arole, "model").unwrap_or_default();
        reporter.dispatch(&format!("architect-{task_id}"), "technical design", &atool, &amodel, Some(&log));
        if architect::architect_run(cfg, ws, &parent, &log, itimeout).await? {
            reporter.status(&format!("architect-{task_id}"), "done", &atool, &amodel);
            state::set_status(&bk, &task_id, "ready", "architect plan ready")?;
        } else {
            reporter.status(&format!("architect-{task_id}"), "failed", &atool, &amodel);
            state::set_status(&bk, &task_id, "ready", "architect produced invalid plan")?;
            continue;
        }
    }
    for builder_id in task_state::ready_builders(ws, &task_id, maxpar)? {
        let plan = task_state::read_builders(ws, &task_id)?;
        if let Some(item) = task_state::item(&plan, &builder_id) {
            builder_ready.push(BuilderDispatch {
                task_id: task_id.clone(),
                builder_id,
                parent: parent.clone(),
                item: item.clone(),
            });
        }
    }
}
builder_ready.truncate(maxpar);
```

Dispatch `builder_ready` using `worker::builder_dispatch` instead of `worker_dispatch`, with worktree branch names based on builder id.

In integration, update builder status in task-local `builders.json` using `task_state::set_builder_status` and `task_state::increment_builder_attempts` instead of business `backlog.json`. On successful merge, mark builder done and increment `merged`.

After builder integration, evaluate each business task with a valid plan:

```rust
let business = state::read(&bk)?;
let items = business["items"].as_array().cloned().unwrap_or_default();
for parent in items {
    let task_id = match parent["id"].as_str() { Some(id) => id.to_string(), None => continue };
    if parent["status"] == "done" || !task_state::builder_plan_valid(ws, &task_id) {
        continue;
    }
    if task_state::all_builders_done(ws, &task_id).unwrap_or(false) {
        let grc = gate(ws);
        if grc != 0 {
            state::set_status(&bk, &task_id, "ready", "verify.sh failed after builders")?;
            continue;
        }
        let log = ldir.join(format!("customer-{task_id}.log"));
        let crole = cfg.resolve_role("customer").unwrap_or_default();
        let ctool = cfg.role_field(&crole, "tool").unwrap_or_default();
        let cmodel = cfg.role_field(&crole, "model").unwrap_or_default();
        reporter.dispatch(&format!("customer-{task_id}"), "customer acceptance", &ctool, &cmodel, Some(&log));
        let approved = customer::customer_run(cfg, ws, &parent, &log, itimeout).await?;
        if approved {
            reporter.status(&format!("customer-{task_id}"), "approved", &ctool, &cmodel);
            state::set_status(&bk, &task_id, "done", "")?;
        } else {
            reporter.status(&format!("customer-{task_id}"), "rejected", &ctool, &cmodel);
            let feedback = std::fs::read_to_string(task_state::customer_path(ws, &task_id)).unwrap_or_else(|_| "customer rejected".into());
            state::set_status(&bk, &task_id, "ready", &feedback)?;
        }
    }
}
```

Update run completion check so `DONE` still requires `gate_state == "pass"` and `state::open_count(&bk)? == 0`. Because business tasks can only be marked done by customer, this implies per-task approval. Add a helper later only if tests show stale approvals can pass incorrectly.

- [ ] **Step 5: Run focused loop tests**

Run:

```bash
cargo test loop_test loop_customer_test loop_addtask_test loop_needs_input_test loop_resolver_test
```

Expected: PASS after adapting stubs.

- [ ] **Step 6: Commit**

```bash
git add src/orchestrator.rs src/events.rs tests/common/mod.rs tests/loop_test.rs tests/loop_customer_test.rs tests/loop_addtask_test.rs tests/loop_needs_input_test.rs tests/loop_resolver_test.rs
git commit -m "feat: orchestrate two-layer task pipeline"
```

---

### Task 6: Repository-Wide Rename Cleanup

**Files:**
- Modify: all `src/*.rs`
- Modify: all `tests/*.rs`
- Modify: `README.md`

- [ ] **Step 1: Search for stale names**

Run:

```bash
rg -n "planner|Planner|PLANNER|worker_prompt|worker_dispatch|config\\.yaml|serde_yaml|flags" src tests README.md Cargo.toml templates
```

Expected before cleanup: any remaining hits are either resolver comments or old docs/tests that must be updated.

- [ ] **Step 2: Fix code/test references**

Apply these substitutions only where they refer to the old primary role:

```text
planner -> manager
Planner -> Manager
PLANNER -> MANAGER
build role -> builder role
worker prompt -> builder prompt
config.yaml -> config.json
```

Do not rename `resolver_prompt`. Do not rename generic English uses of "planning" if the sentence still makes sense.

- [ ] **Step 3: Run compile check**

Run:

```bash
cargo test --no-run
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src tests README.md Cargo.toml Cargo.lock templates
git commit -m "refactor: rename planner workflow to manager"
```

---

### Task 7: README And Final Verification

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/specs/2026-06-03-two-layer-roles-and-global-json-config-design.md` only if implementation reveals a necessary correction

- [ ] **Step 1: Update README behavior docs**

In `README.md`, update the intro:

```markdown
Autonomous app builder. Give it one goal; it manages a business backlog, creates
per-task technical designs, spawns `claude`/`codex` builders in parallel git
worktrees, integrates their work, runs acceptance checks, and asks a silly customer
agent to approve each business task by AC.
```

Update options:

```markdown
- `--config <path>` — config.json path (default: global config; `$AGENTLOOP_CONFIG`
  or the platform config directory)
```

Update How it works routing:

```markdown
- **Routing:** edit the global `config.json` to map each role to a tool/model/effort.
  Roles are `manager`, `architect`, `builder`, `customer`, and `resolver`.
  Permission flags are not configurable: `claude` always receives
  `--dangerously-skip-permissions`, and `codex` always receives `--yolo`.
```

Update state:

```markdown
- **State:** `.agentloop/state/backlog.json` is the manager-owned business backlog.
  Per-task technical state lives in `.agentloop/state/tasks/<task-id>/`.
```

Remove the layout line for `templates/config.yaml`.

- [ ] **Step 2: Full test suite**

Run:

```bash
cargo test
```

Expected: PASS.

- [ ] **Step 3: Final stale-reference scan**

Run:

```bash
rg -n "templates/config.yaml|config\\.yaml|serde_yaml|planner|PLANNER|worker_prompt|worker_dispatch|flags" README.md src tests Cargo.toml docs/superpowers/specs/2026-06-03-two-layer-roles-and-global-json-config-design.md
```

Expected: no stale references except historical context in the design spec and acceptable resolver/worker generic wording.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/superpowers/specs/2026-06-03-two-layer-roles-and-global-json-config-design.md
git commit -m "docs: update roles and config"
```

---

## Self-Review

Spec coverage:

- Global JSON config: Task 1.
- Remove YAML template and workspace config: Task 1 and Task 7.
- Remove configurable flags and inject tool permission flags: Task 2.
- Manager replacing planner as business-only owner: Task 3, Task 5, Task 6.
- Architect per business task with design and builder split: Task 3, Task 4, Task 5.
- Builder subitems in two-layer state: Task 3, Task 4, Task 5.
- Silly customer per business task: Task 3, Task 4, Task 5.
- Goal completion requiring customer approval: Task 5.
- Docs/tests: Task 6 and Task 7.

Placeholder scan: no placeholder markers or unspecified test steps remain. The only flexible area is exact loop-test stub adaptation, but the required prompt cases and JSON outputs are provided.

Type consistency:

- Config helper names are `Config::load`, `Config::default_config_path`, and `Config::ensure_default_config`.
- Role prompt names are `manager_prompt`, `architect_prompt`, `builder_prompt`, and `customer_prompt`.
- Role run names are `manager_run`, `architect_run`, `builder_dispatch`, and `customer_run`.
- Task-local helper names are `builder_plan_valid`, `ready_builders`, `all_builders_done`, `set_builder_status`, `increment_builder_attempts`, `write_customer`, and `customer_approved`.
