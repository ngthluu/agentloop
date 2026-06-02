# agentloop Rust Port — Phase 1 (behavior-parity headless port) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the bash `agentloop` orchestrator with a single Rust binary that reproduces today's behavior exactly — same CLI, same `.agentloop/` on-disk contract, same prompts — validated against ported fake-agent tests.

**Architecture:** A `tokio` async binary built as a library (`agentloop`) plus two bins (`agentloop`, `fake_agent`). Modules mirror the bash libs one-to-one. The orchestrator runs the plan→dispatch→integrate→gate loop; in Phase 1 progress is emitted as plain stderr event-lines through a `Reporter` trait so Phases 2–3 can swap in a TUI without reworking the loop. State lives in `.agentloop/` exactly as today (`state/backlog.json`, `state/master.md`, `state/goal.md`, `config.yaml`, `results/`, `logs/`, `worktrees/`).

**Tech Stack:** Rust 2021, `tokio` (rt-multi-thread, process, time, macros), `clap` (derive), `serde`/`serde_json`/`serde_yaml`, `anyhow`, `command-group` (+ `nix`) for process-group spawn/kill, `chrono` for timestamps. Git operations shell out to the `git` CLI. Bash + git + jq remain test-time deps (the loop integration test reuses a shell stub agent, mirroring the existing suite).

---

## File Structure

```
Cargo.toml                 crate manifest: [lib] agentloop + [[bin]] agentloop + src/bin/fake_agent.rs
src/lib.rs                 pub mod config, state, spawn, worktree, events, planner, worker, orchestrator, cli
src/main.rs                bin "agentloop": tokio main -> cli::run()
src/bin/fake_agent.rs      bin "fake_agent": echoes argv, honors FAKE_SLEEP/FAKE_EXIT (replaces tests/fake_agent.sh)
src/config.rs              Config structs, load(), resolve_role(), role_field(), cap accessors
src/state.rs               Value-based backlog read/mutate: valid, ready_items, open_count, set_status, increment_attempts
src/spawn.rs               build_argv(), run_with_timeout(), agent_run() (+ FAKE_AGENT hook)
src/worktree.rs            create(), merge(), remove() — git shell-out
src/events.rs              Reporter trait + EventLineReporter (stderr event lines)
src/planner.rs             planner_prompt(), planner_run()
src/worker.rs              worker_prompt(), worker_dispatch()
src/orchestrator.rs        gate(), iterate(), run() — the loop
src/cli.rs                 clap Args, bootstrap_workspace(), run()
templates/config.yaml      embedded via include_str! (already exists; keep file)
templates/master.md        embedded via include_str! (already exists; keep file)
tests/config_test.rs
tests/state_test.rs
tests/spawn_test.rs
tests/worktree_test.rs
tests/planner_worker_test.rs
tests/loop_test.rs         integration: scripted shell stub agent, asserts DONE + merged + open==0 + "dispatch" line
tests/common/mod.rs        temp-workspace + git-init helpers shared by integration tests
```

Each module owns one responsibility and is small enough to hold in context. State mutations stay `serde_json::Value`-based (like the bash `jq` calls) so planner-authored fields are never dropped on rewrite.

---

### Task 1: Cargo scaffold (lib + two bins), compiles green

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `src/bin/fake_agent.rs`

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "agentloop"
version = "0.1.0"
edition = "2021"

[lib]
name = "agentloop"
path = "src/lib.rs"

[[bin]]
name = "agentloop"
path = "src/main.rs"

[[bin]]
name = "fake_agent"
path = "src/bin/fake_agent.rs"

[dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros", "process", "time", "fs", "io-util", "signal"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
anyhow = "1"
command-group = { version = "5", features = ["with-tokio"] }
nix = { version = "0.29", features = ["signal", "process"] }
chrono = { version = "0.4", default-features = false, features = ["clock"] }
```

- [ ] **Step 2: Write `src/lib.rs` with empty module declarations**

```rust
pub mod cli;
pub mod config;
pub mod events;
pub mod orchestrator;
pub mod planner;
pub mod spawn;
pub mod state;
pub mod worker;
pub mod worktree;
```

Create each referenced file as an empty stub (e.g. `// config` ) so the crate compiles. They get filled in later tasks.

- [ ] **Step 3: Write `src/main.rs`**

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agentloop::cli::run().await
}
```

For now make `src/cli.rs` contain a minimal stub so it compiles:

```rust
use anyhow::Result;
pub async fn run() -> Result<()> {
    Ok(())
}
```

- [ ] **Step 4: Write `src/bin/fake_agent.rs`** (Rust replacement for `tests/fake_agent.sh`)

```rust
// Stand-in for claude/codex when FAKE_AGENT=1. Echoes its argv so tests can
// assert command construction. Honors FAKE_SLEEP (secs) and FAKE_EXIT (code).
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("FAKE_ARGS: {}", args.join(" "));
    if let Ok(s) = std::env::var("FAKE_SLEEP") {
        if let Ok(secs) = s.parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_secs(secs));
        }
    }
    let code = std::env::var("FAKE_EXIT").ok().and_then(|c| c.parse::<i32>().ok()).unwrap_or(0);
    std::process::exit(code);
}
```

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: compiles with warnings about unused stubs, no errors.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/
git commit -m "chore: scaffold agentloop rust crate (lib + agentloop + fake_agent bins)"
```

---

### Task 2: config.rs — load YAML, resolve role, read fields/caps

**Files:**
- Modify: `src/config.rs`
- Test: `tests/config_test.rs`

- [ ] **Step 1: Write the failing test** in `tests/config_test.rs`

```rust
use agentloop::config::Config;
use std::io::Write;

fn write_cfg(body: &str) -> tempfile_path::TempCfg {
    let dir = std::env::temp_dir().join(format!("alcfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("config.yaml");
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    tempfile_path::TempCfg { path: p }
}

mod tempfile_path {
    pub struct TempCfg { pub path: std::path::PathBuf }
}

const SAMPLE: &str = r#"
caps: { max_iterations: 7, max_parallel: 2, item_timeout_sec: 30, total_budget_sec: 300, max_attempts: 3 }
routing:
  planner: { tool: claude, model: opus, effort: high, flags: "--dangerously-skip-permissions" }
  build:   { tool: codex,  model: gpt-5, effort: high, flags: "" }
defaults: { role: build }
"#;

#[test]
fn loads_and_resolves() {
    let c = write_cfg(SAMPLE);
    let cfg = Config::load(&c.path).unwrap();

    assert_eq!(cfg.resolve_role("planner").as_deref(), Some("planner"));
    assert_eq!(cfg.resolve_role("nonexistent").as_deref(), Some("build")); // -> defaults.role
    assert_eq!(cfg.role_field("planner", "tool").as_deref(), Some("claude"));
    assert_eq!(cfg.role_field("planner", "model").as_deref(), Some("opus"));
    assert_eq!(cfg.role_field("build", "flags"), None); // empty string -> None
    assert_eq!(cfg.max_iterations(), 7);
    assert_eq!(cfg.max_parallel(), 2);
    assert_eq!(cfg.max_attempts(), 3);
}

#[test]
fn caps_default_when_absent() {
    let c = write_cfg("routing: {}\ndefaults: {}\n");
    let cfg = Config::load(&c.path).unwrap();
    assert_eq!(cfg.max_iterations(), 25);
    assert_eq!(cfg.item_timeout_sec(), 1200);
    assert_eq!(cfg.resolve_role("anything"), None); // no defaults.role
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test config_test`
Expected: FAIL — `Config` has no `load`/`resolve_role` etc.

- [ ] **Step 3: Implement `src/config.rs`**

```rust
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

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
    pub flags: Option<String>,
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
        serde_yaml::from_str(&text).context("parse config yaml")
    }

    /// Role to actually use: the role if present in routing, else defaults.role.
    pub fn resolve_role(&self, role: &str) -> Option<String> {
        if self.routing.contains_key(role) {
            Some(role.to_string())
        } else {
            self.defaults.role.clone()
        }
    }

    /// A role's field, or None if absent or empty (mirrors jq `// empty`).
    pub fn role_field(&self, role: &str, field: &str) -> Option<String> {
        let r = self.routing.get(role)?;
        let v = match field {
            "tool" => r.tool.clone(),
            "model" => r.model.clone(),
            "effort" => r.effort.clone(),
            "flags" => r.flags.clone(),
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test config_test`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs tests/config_test.rs
git commit -m "feat(config): serde_yaml config load + role/cap resolution"
```

---

### Task 3: state.rs — backlog read/mutate (Value-based, atomic)

**Files:**
- Modify: `src/state.rs`
- Test: `tests/state_test.rs`

This ports `lib/state.sh` and must satisfy the same assertions as `tests/test_state.sh`.

- [ ] **Step 1: Write the failing test** in `tests/state_test.rs`

```rust
use agentloop::state;
use std::io::Write;
use std::path::PathBuf;

fn tmp_backlog(body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("alstate-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("backlog.json");
    std::fs::File::create(&p).unwrap().write_all(body.as_bytes()).unwrap();
    p
}
fn rand_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

const BK: &str = r#"{ "items": [
  {"id":"it-1","status":"done","deps":[]},
  {"id":"it-2","status":"ready","deps":["it-1"]},
  {"id":"it-3","status":"ready","deps":["it-2"]},
  {"id":"it-4","status":"ready","deps":[]},
  {"id":"it-5","status":"ready"}
]}"#;

#[test]
fn valid_and_invalid() {
    let p = tmp_backlog(BK);
    assert!(state::backlog_valid(&p));
    let bad = tmp_backlog("not json");
    assert!(!state::backlog_valid(&bad));
}

#[test]
fn ready_respects_deps_and_parallel() {
    let p = tmp_backlog(BK);
    // it-2 (dep it-1 done), it-4 (empty deps), it-5 (no deps key); NOT it-3 (dep it-2 ready)
    assert_eq!(state::ready_items(&p, 10).unwrap(), vec!["it-2", "it-4", "it-5"]);
    assert_eq!(state::ready_items(&p, 1).unwrap(), vec!["it-2"]);
}

#[test]
fn open_count_counts_open_states() {
    let p = tmp_backlog(BK);
    // ready+in_progress+blocked = it-2,it-3,it-4,it-5 = 4
    assert_eq!(state::open_count(&p).unwrap(), 4);
}

#[test]
fn set_status_and_notes() {
    let p = tmp_backlog(BK);
    state::set_status(&p, "it-2", "done", "merged ok").unwrap();
    let v = state::read(&p).unwrap();
    let it2 = v["items"].as_array().unwrap().iter().find(|i| i["id"] == "it-2").unwrap();
    assert_eq!(it2["status"], "done");
    assert_eq!(it2["notes"], "merged ok");
    // empty note preserves existing notes
    state::set_status(&p, "it-2", "done", "").unwrap();
    let v = state::read(&p).unwrap();
    let it2 = v["items"].as_array().unwrap().iter().find(|i| i["id"] == "it-2").unwrap();
    assert_eq!(it2["notes"], "merged ok");
}

#[test]
fn increment_attempts() {
    let p = tmp_backlog(BK);
    state::increment_attempts(&p, "it-3").unwrap();
    let v = state::read(&p).unwrap();
    let it3 = v["items"].as_array().unwrap().iter().find(|i| i["id"] == "it-3").unwrap();
    assert_eq!(it3["attempts"], 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test state_test`
Expected: FAIL — `state` functions undefined.

- [ ] **Step 3: Implement `src/state.rs`**

```rust
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

pub fn read(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// Atomic write: temp file in the same dir, then rename.
fn write_atomic(path: &Path, v: &Value) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(".state.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn backlog_valid(path: &Path) -> bool {
    matches!(read(path), Ok(v) if v.get("items").map(|i| i.is_array()).unwrap_or(false))
}

pub fn ready_items(path: &Path, max_parallel: usize) -> Result<Vec<String>> {
    let v = read(path)?;
    let empty = vec![];
    let items = v["items"].as_array().unwrap_or(&empty);
    let done: HashSet<&str> = items.iter()
        .filter(|i| i["status"] == "done")
        .filter_map(|i| i["id"].as_str())
        .collect();
    let mut out = Vec::new();
    for it in items {
        if it["status"] != "ready" { continue; }
        let deps_ok = match it.get("deps").and_then(|d| d.as_array()) {
            Some(deps) => deps.iter().all(|d| d.as_str().map(|s| done.contains(s)).unwrap_or(false)),
            None => true, // missing deps key == no deps
        };
        if deps_ok {
            if let Some(id) = it["id"].as_str() { out.push(id.to_string()); }
        }
    }
    out.truncate(max_parallel);
    Ok(out)
}

pub fn open_count(path: &Path) -> Result<i64> {
    let v = read(path)?;
    let empty = vec![];
    let n = v["items"].as_array().unwrap_or(&empty).iter()
        .filter(|i| matches!(i["status"].as_str(), Some("ready") | Some("in_progress") | Some("blocked")))
        .count();
    Ok(n as i64)
}

pub fn set_status(path: &Path, id: &str, status: &str, note: &str) -> Result<()> {
    let mut v = read(path)?;
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if it["id"] == json!(id) {
                it["status"] = json!(status);
                if !note.is_empty() { it["notes"] = json!(note); }
            }
        }
    }
    write_atomic(path, &v)
}

pub fn increment_attempts(path: &Path, id: &str) -> Result<()> {
    let mut v = read(path)?;
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if it["id"] == json!(id) {
                let cur = it.get("attempts").and_then(|a| a.as_u64()).unwrap_or(0);
                it["attempts"] = json!(cur + 1);
            }
        }
    }
    write_atomic(path, &v)
}

/// Convenience accessor used by the orchestrator.
pub fn item<'a>(v: &'a Value, id: &str) -> Option<&'a Value> {
    v["items"].as_array()?.iter().find(|i| i["id"] == json!(id))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test state_test`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/state.rs tests/state_test.rs
git commit -m "feat(state): value-based backlog read/mutate with atomic writes"
```

---

### Task 4: spawn.rs — argv construction, timeout, process-group kill

**Files:**
- Modify: `src/spawn.rs`
- Test: `tests/spawn_test.rs`

Ports `lib/spawn.sh`. Mirrors `tests/test_spawn.sh` assertions on argv + the `FAKE_AGENT` hook.

- [ ] **Step 1: Write the failing test** in `tests/spawn_test.rs`

```rust
use agentloop::config::Config;
use agentloop::spawn;
use std::path::PathBuf;
use std::time::Duration;

fn cfg() -> Config {
    serde_yaml::from_str(r#"
routing:
  planner: { tool: claude, model: opus, effort: high, flags: "--flag-a --flag-b" }
  build:   { tool: codex,  model: gpt-5, effort: high, flags: "" }
defaults: { role: build }
"#).unwrap()
}

#[test]
fn claude_argv() {
    let a = spawn::build_argv(&cfg(), "planner", "HELLO").unwrap();
    assert_eq!(a, vec![
        "claude","-p","HELLO","--model","opus","--effort","high","--flag-a","--flag-b"
    ]);
}

#[test]
fn codex_argv() {
    let a = spawn::build_argv(&cfg(), "build", "DOIT").unwrap();
    assert_eq!(a, vec![
        "codex","exec","DOIT","-m","gpt-5","-c","model_reasoning_effort=high"
    ]);
}

#[test]
fn unknown_tool_errors() {
    let c: Config = serde_yaml::from_str("routing: { x: { tool: nope } }\ndefaults: {}\n").unwrap();
    assert!(spawn::build_argv(&c, "x", "p").is_err());
}

#[tokio::test]
async fn fake_agent_runs_and_logs_argv() {
    // Use the compiled fake_agent bin; it echoes "FAKE_ARGS: ..." to the log.
    let bin = env!("CARGO_BIN_EXE_fake_agent");
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", bin);

    let dir = std::env::temp_dir().join(format!("alspawn-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let log: PathBuf = dir.join("agent.log");

    let rc = spawn::agent_run(&cfg(), "planner", "HELLO", &dir, &log, Duration::from_secs(10)).await.unwrap();
    assert_eq!(rc, 0);
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.contains("FAKE_ARGS:"), "log: {logged}");
    assert!(logged.contains("--model"), "real argv passed to fake: {logged}");
    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
}

#[tokio::test]
async fn timeout_returns_124() {
    let bin = env!("CARGO_BIN_EXE_fake_agent");
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", bin);
    std::env::set_var("FAKE_SLEEP", "10");

    let dir = std::env::temp_dir().join(format!("altimeout-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("agent.log");

    let rc = spawn::agent_run(&cfg(), "planner", "P", &dir, &log, Duration::from_secs(1)).await.unwrap();
    assert_eq!(rc, 124);
    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("FAKE_SLEEP");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test spawn_test`
Expected: FAIL — `spawn::build_argv`/`agent_run` undefined.

- [ ] **Step 3: Implement `src/spawn.rs`**

```rust
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::time::Duration;

use crate::config::Config;

/// Build the real claude/codex argv for a role (prompt included), mirroring lib/spawn.sh.
pub fn build_argv(cfg: &Config, role: &str, prompt: &str) -> Result<Vec<String>> {
    let rrole = cfg.resolve_role(role).context("no resolvable role")?;
    let tool = cfg.role_field(&rrole, "tool").context("role has no tool")?;
    let model = cfg.role_field(&rrole, "model");
    let effort = cfg.role_field(&rrole, "effort");
    let flags = cfg.role_field(&rrole, "flags");

    let mut argv: Vec<String> = Vec::new();
    match tool.as_str() {
        "claude" => argv.extend(["claude".into(), "-p".into(), prompt.into()]),
        "codex" => argv.extend(["codex".into(), "exec".into(), prompt.into()]),
        other => bail!("agent_run: unknown tool [{other}]"),
    }
    if let Some(m) = model {
        if tool == "codex" { argv.push("-m".into()); argv.push(m); }
        else { argv.push("--model".into()); argv.push(m); }
    }
    if let Some(e) = effort {
        if tool == "codex" { argv.push("-c".into()); argv.push(format!("model_reasoning_effort={e}")); }
        else { argv.push("--effort".into()); argv.push(e); }
    }
    if let Some(f) = flags {
        for tok in f.split_whitespace() { argv.push(tok.to_string()); }
    }
    Ok(argv)
}

/// Run argv with a wall-clock cap. Returns the exit code, or 124 on timeout.
/// The child is its own process group; on timeout the whole group is signalled
/// (SIGTERM, brief grace, SIGKILL) so descendant claude/codex processes die too.
pub async fn run_with_timeout(argv: &[String], cwd: &Path, log: &Path, t: Duration) -> Result<i32> {
    use command_group::AsyncCommandGroup;
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    let file = std::fs::File::create(log).with_context(|| format!("create log {}", log.display()))?;
    let err = file.try_clone()?;

    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(cwd).stdout(file).stderr(err);

    let mut child = cmd.group_spawn().context("spawn agent process group")?;
    let pgid = child.id().map(|p| Pid::from_raw(p as i32));

    match tokio::time::timeout(t, child.wait()).await {
        Ok(status) => Ok(status?.code().unwrap_or(-1)),
        Err(_) => {
            if let Some(pg) = pgid {
                let _ = killpg(pg, Signal::SIGTERM);
                tokio::time::sleep(Duration::from_secs(1)).await;
                let _ = killpg(pg, Signal::SIGKILL);
            }
            let _ = child.wait().await;
            Ok(124)
        }
    }
}

/// Resolve a role and run the matching CLI (or fake) in cwd, capped by timeout.
pub async fn agent_run(cfg: &Config, role: &str, prompt: &str, cwd: &Path, log: &Path, t: Duration) -> Result<i32> {
    let mut argv = build_argv(cfg, role, prompt)?;
    // In fake mode, prepend the stub; it receives the genuine real argv.
    if std::env::var("FAKE_AGENT").as_deref() == Ok("1") {
        let bin = std::env::var("FAKE_AGENT_BIN").context("FAKE_AGENT=1 but FAKE_AGENT_BIN unset")?;
        argv.insert(0, bin);
    }
    run_with_timeout(&argv, cwd, log, t).await
}
```

> Note for executor: `command-group` v5 exposes `AsyncCommandGroup::group_spawn` returning an `AsyncGroupChild` with `.id()` and `.wait()`. If the installed version's method names differ, adjust the two `use`/call sites only — the timeout/signal logic is unchanged. `child.id()` returns the group leader pid, which equals the pgid.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test spawn_test`
Expected: PASS (5 tests). The timeout test takes ~2s.

- [ ] **Step 5: Commit**

```bash
git add src/spawn.rs tests/spawn_test.rs
git commit -m "feat(spawn): argv build + process-group timeout + FAKE_AGENT hook"
```

---

### Task 5: worktree.rs — git worktree create/merge/remove

**Files:**
- Modify: `src/worktree.rs`
- Test: `tests/worktree_test.rs`

- [ ] **Step 1: Write the failing test** in `tests/worktree_test.rs`

```rust
use agentloop::worktree;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(repo).args(args).status().unwrap().success();
    assert!(ok, "git {:?} failed", args);
}

fn init_repo() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("alwt-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("seed.txt"), "seed").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "init"]);
    dir
}

#[test]
fn create_merge_remove_roundtrip() {
    let repo = init_repo();
    let wt = repo.join(".agentloop/worktrees/it-1");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();

    worktree::create(&repo, "item/it-1", &wt).unwrap();
    assert!(wt.join("seed.txt").exists());

    // make a commit in the worktree
    std::fs::write(wt.join("made.txt"), "x").unwrap();
    git(&wt, &["add", "-A"]);
    git(&wt, &["commit", "-qm", "work"]);

    assert!(worktree::merge(&repo, "item/it-1").unwrap());
    assert!(repo.join("made.txt").exists(), "merged file present on main");

    worktree::remove(&repo, &wt, "item/it-1");
    assert!(!wt.exists());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test worktree_test`
Expected: FAIL — `worktree::create` undefined.

- [ ] **Step 3: Implement `src/worktree.rs`**

```rust
use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> Result<bool> {
    let status = Command::new("git").arg("-C").arg(repo).args(args).status()?;
    Ok(status.success())
}

pub fn create(repo: &Path, branch: &str, path: &Path) -> Result<()> {
    let p = path.to_str().unwrap();
    if git(repo, &["worktree", "add", "-q", "-b", branch, p, "HEAD"])? {
        Ok(())
    } else {
        bail!("worktree add failed for {branch}")
    }
}

/// Merge branch into repo's current branch. On conflict, abort and return false.
pub fn merge(repo: &Path, branch: &str) -> Result<bool> {
    if git(repo, &["merge", "--no-edit", "-q", branch])? {
        Ok(true)
    } else {
        let _ = git(repo, &["merge", "--abort"]);
        Ok(false)
    }
}

pub fn remove(repo: &Path, path: &Path, branch: &str) {
    let p = path.to_str().unwrap_or("");
    let _ = git(repo, &["worktree", "remove", "--force", p]);
    let _ = git(repo, &["branch", "-D", branch]);
}

/// Whether `branch` has commits ahead of HEAD (used to detect "claimed done but no commits").
pub fn has_commits_ahead(repo: &Path, branch: &str) -> bool {
    let out = Command::new("git").arg("-C").arg(repo)
        .args(["log", "--oneline", &format!("HEAD..{branch}")])
        .output();
    match out {
        Ok(o) => !String::from_utf8_lossy(&o.stdout).trim().is_empty(),
        Err(_) => false,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test worktree_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/worktree.rs tests/worktree_test.rs
git commit -m "feat(worktree): git worktree create/merge/remove + ahead check"
```

---

### Task 6: events.rs — Reporter trait + stderr event-line reporter

**Files:**
- Modify: `src/events.rs`

No dedicated unit test (it's an output seam; covered by the loop integration test in Task 8). This is a single short step.

- [ ] **Step 1: Implement `src/events.rs`**

```rust
use chrono::Local;

/// Progress sink. Phase 1 uses EventLineReporter (stderr lines, mirroring the
/// non-TTY behavior of lib/progress.sh). Phases 2-3 add a TUI implementation.
pub trait Reporter: Send + Sync {
    /// A job (planner or worker) has been dispatched.
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str);
    /// A job changed status (done/failed/merged/bounced/...).
    fn status(&self, id: &str, status: &str, tool: &str, model: &str);
    /// End-of-iteration summary line.
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64);
}

pub struct EventLineReporter;

fn hms() -> String { Local::now().format("%H:%M:%S").to_string() }

impl Reporter for EventLineReporter {
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str) {
        eprintln!("{}  dispatch {:<10} {}/{}  {}", hms(), id, tool, model, label);
    }
    fn status(&self, id: &str, status: &str, tool: &str, model: &str) {
        eprintln!("{}  {:<9} {:<10} {}/{}", hms(), status, id, tool, model);
    }
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64) {
        eprintln!("iter {n}: merged={merged} gate={gate} open={open}");
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add src/events.rs
git commit -m "feat(events): Reporter trait + stderr EventLineReporter"
```

---

### Task 7: planner.rs + worker.rs — prompts and dispatch

**Files:**
- Modify: `src/planner.rs`
- Modify: `src/worker.rs`
- Test: `tests/planner_worker_test.rs`

Prompt text is copied verbatim from `lib/planner.sh` / `lib/worker.sh`. Tests assert the prompts contain the key contract markers (used by the loop test's stub and by real agents).

- [ ] **Step 1: Write the failing test** in `tests/planner_worker_test.rs`

```rust
use agentloop::{planner, worker};
use serde_json::json;
use std::path::PathBuf;

fn ws_with_state() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("alpw-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let st = dir.join(".agentloop/state");
    std::fs::create_dir_all(&st).unwrap();
    std::fs::write(st.join("goal.md"), "build a thing").unwrap();
    std::fs::write(st.join("master.md"), "# status").unwrap();
    std::fs::write(st.join("backlog.json"), r#"{"items":[]}"#).unwrap();
    dir
}

#[test]
fn planner_prompt_has_contract() {
    let ws = ws_with_state();
    let p = planner::planner_prompt(&ws, 3);
    assert!(p.contains("You are the PLANNER"));
    assert!(p.contains("build a thing"));            // goal embedded
    assert!(p.contains("backlog.json"));             // output contract
    assert!(p.contains("max_attempts"));             // cap mention
}

#[test]
fn worker_prompt_has_contract() {
    let item = json!({
        "id": "it-9", "title": "T", "desc": "D", "role": "build", "acceptance": "A"
    });
    let p = worker::worker_prompt(std::path::Path::new("/ws"), &item);
    assert!(p.contains("You are a WORKER"));
    assert!(p.contains("it-9"));
    assert!(p.contains("A"));                         // acceptance
    assert!(p.contains(".agentloop/results/it-9.json"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test planner_worker_test`
Expected: FAIL — functions undefined.

- [ ] **Step 3: Implement `src/planner.rs`**

```rust
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
Do not print the JSON to stdout; write the files."#,
        ws = ws.display(), goal = goal, master = master, backlog = backlog, max_attempts = max_attempts)
}

/// Invoke the planner agent, then validate backlog.json (re-prompt once on invalid).
pub async fn planner_run(cfg: &Config, ws: &Path, log: &Path, t: Duration) -> Result<bool> {
    let bk = ws.join(".agentloop/state/backlog.json");
    let max_attempts = cfg.max_attempts();
    let prompt = planner_prompt(ws, max_attempts);
    spawn::agent_run(cfg, "planner", &prompt, ws, log, t).await?;
    if state::backlog_valid(&bk) { return Ok(true); }

    eprintln!("planner produced invalid backlog.json; re-prompting once");
    let retry = format!("{prompt}\nNOTE: your previous backlog.json was invalid JSON. Write valid JSON this time.");
    spawn::agent_run(cfg, "planner", &retry, ws, log, t).await?;
    Ok(state::backlog_valid(&bk))
}
```

- [ ] **Step 4: Implement `src/worker.rs`**

```rust
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test planner_worker_test`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add src/planner.rs src/worker.rs tests/planner_worker_test.rs
git commit -m "feat(planner,worker): verbatim prompts + async dispatch"
```

---

### Task 8: orchestrator.rs — the loop (gate, iterate, run) + integration test

**Files:**
- Modify: `src/orchestrator.rs`
- Test: `tests/loop_test.rs`
- Create: `tests/common/mod.rs`

Ports `lib/loop.sh`. Phase 1 dispatches ready items concurrently (tokio tasks), then integrates sequentially — same outcomes as the bash loop. Progress flows through `Reporter`.

- [ ] **Step 1: Write the failing integration test** in `tests/loop_test.rs`

This mirrors `tests/test_loop.sh`: a scripted shell stub acts as planner (seed one item, then mark done) and worker (create a file + write its result). It asserts DONE, the merged file on main, zero open items, and that a "dispatch" event line was emitted.

```rust
mod common;
use agentloop::config::Config;
use agentloop::events::EventLineReporter;
use agentloop::orchestrator;
use std::path::Path;

#[tokio::test]
async fn loop_runs_to_done() {
    let ws = common::init_ws_with_stub();

    let cfg: Config = serde_yaml::from_str(r#"
caps: { max_iterations: 5, max_parallel: 2, item_timeout_sec: 30, total_budget_sec: 300, max_attempts: 3 }
routing:
  planner: { tool: claude, model: opus, effort: high, flags: "" }
  build:   { tool: codex,  model: gpt-5, effort: high, flags: "" }
defaults: { role: build }
"#).unwrap();

    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);

    let rc = orchestrator::run(&cfg, &ws, &EventLineReporter).await.unwrap();
    assert_eq!(rc, 0, "loop reports DONE");
    assert_eq!(std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(), "made");
    assert_eq!(agentloop::state::open_count(&ws.join(".agentloop/state/backlog.json")).unwrap(), 0);

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
    let _ = std::fs::remove_dir_all(&ws);
}

#[allow(dead_code)]
fn _path_marker(_: &Path) {}
```

And `tests/common/mod.rs` (creates the workspace + writes the scripted stub, identical in spirit to test_loop.sh):

```rust
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(repo: &Path, args: &[&str]) {
    assert!(Command::new("git").arg("-C").arg(repo).args(args).status().unwrap().success());
}

pub fn init_ws_with_stub() -> PathBuf {
    let ws = std::env::temp_dir().join(format!("alloop-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let st = ws.join(".agentloop/state");
    std::fs::create_dir_all(&st).unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/results")).unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/logs")).unwrap();
    git(&ws, &["init", "-q"]);
    git(&ws, &["config", "user.email", "t@t"]);
    git(&ws, &["config", "user.name", "t"]);
    std::fs::write(ws.join("seed.txt"), "seed").unwrap();
    git(&ws, &["add", "-A"]);
    git(&ws, &["commit", "-qm", "init"]);
    std::fs::write(st.join("goal.md"), "make one file").unwrap();
    std::fs::write(st.join("master.md"), "# status").unwrap();
    std::fs::write(st.join("backlog.json"), r#"{"items":[]}"#).unwrap();

    // Scripted stub, ported from tests/test_loop.sh. $1 is the real tool ("claude"/"codex").
    let stub = r#"#!/bin/bash
tool="$1"; shift
ws_state="$WS/.agentloop/state"; res="$WS/.agentloop/results"
prompt="$*"
case "$prompt" in
  *PLANNER*)
    n=$(cat "$WS/.plan_n" 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > "$WS/.plan_n"
    if [ "$n" -eq 1 ]; then
      echo '{"items":[{"id":"it-1","title":"f","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/backlog.json"
      printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$WS/.agentloop/verify.sh"; chmod +x "$WS/.agentloop/verify.sh"
    else
      if [ -f "$res/it-1.json" ]; then
        python3 -c "import json,sys; p='$ws_state/backlog.json'; d=json.load(open(p)); [i.__setitem__('status','done') for i in d['items']]; json.dump(d,open(p,'w'))"
      fi
    fi
    echo "# updated" > "$ws_state/master.md"
    ;;
  *WORKER*)
    echo made > "$PWD/made.txt"; git add -A; git commit -qm "worker" 2>/dev/null
    echo '{"status":"done","summary":"made file","files_changed":["made.txt"]}' > "$res/it-1.json"
    ;;
esac
exit 0
"#;
    let stub_path = ws.join("stub.sh");
    std::fs::write(&stub_path, stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    ws
}
```

> The stub uses `python3` instead of `jq` for the "mark done" mutation to avoid a `jq` dependency in tests; behavior matches the original `jq` line.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test loop_test`
Expected: FAIL — `orchestrator::run` undefined.

- [ ] **Step 3: Implement `src/orchestrator.rs`**

```rust
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::events::Reporter;
use crate::{planner, state, worker, worktree};

/// Run verify.sh; capture output to last_gate.txt; return its exit code (1 if absent).
pub fn gate(ws: &Path) -> i32 {
    let gate = ws.join(".agentloop/verify.sh");
    let out = ws.join(".agentloop/state/last_gate.txt");
    if gate.exists() {
        let result = std::process::Command::new("/bin/bash")
            .arg(&gate).current_dir(ws).output();
        match result {
            Ok(o) => {
                let mut buf = o.stdout.clone(); buf.extend_from_slice(&o.stderr);
                let _ = std::fs::write(&out, &buf);
                o.status.code().unwrap_or(1)
            }
            Err(_) => { let _ = std::fs::write(&out, "verify.sh spawn failed"); 1 }
        }
    } else {
        let _ = std::fs::write(&out, "no verify.sh yet");
        1
    }
}

/// One iteration: plan, select, dispatch in parallel, integrate. Returns merged count.
pub async fn iterate(cfg: &Config, ws: &Path, n: u32, reporter: &Arc<dyn Reporter>) -> Result<u32> {
    let sdir = ws.join(".agentloop/state");
    let ldir = ws.join(format!(".agentloop/logs/iter-{n}"));
    std::fs::create_dir_all(&ldir)?;
    std::fs::create_dir_all(ws.join(".agentloop/results"))?;
    let bk = sdir.join("backlog.json");
    let itimeout = Duration::from_secs(cfg.item_timeout_sec());
    let maxpar = cfg.max_parallel() as usize;
    let maxatt = cfg.max_attempts();

    // Planner (tracked, awaited).
    let prole = cfg.resolve_role("planner").unwrap_or_default();
    let ptool = cfg.role_field(&prole, "tool").unwrap_or_default();
    let pmodel = cfg.role_field(&prole, "model").unwrap_or_default();
    reporter.dispatch("planner", "planning", &ptool, &pmodel);
    let ok = planner::planner_run(cfg, ws, &ldir.join("planner.log"), itimeout).await?;
    if !ok { eprintln!("planner failed/invalid"); anyhow::bail!("planner invalid"); }
    reporter.status("planner", "done", &ptool, &pmodel);

    let ready = state::ready_items(&bk, maxpar)?;
    if ready.is_empty() { return Ok(0); }

    // Dispatch each ready item in its own worktree, concurrently.
    let mut handles = Vec::new();
    let mut dispatched: Vec<String> = Vec::new();
    for id in ready {
        let v = state::read(&bk)?;
        let item = match state::item(&v, &id) { Some(i) => i.clone(), None => continue };
        let att = item.get("attempts").and_then(|a| a.as_u64()).unwrap_or(0) as u32;
        if att >= maxatt {
            state::set_status(&bk, &id, "failed", &format!("exceeded max_attempts ({maxatt})"))?;
            continue;
        }
        let wt = ws.join(format!(".agentloop/worktrees/{id}"));
        let _ = std::fs::remove_dir_all(&wt);
        worktree::remove(ws, &wt, &format!("item/{id}"));
        if worktree::create(ws, &format!("item/{id}"), &wt).is_err() {
            state::set_status(&bk, &id, "failed", "worktree create failed")?;
            continue;
        }
        state::set_status(&bk, &id, "in_progress", "")?;
        state::increment_attempts(&bk, &id)?;

        let role = item["role"].as_str().unwrap_or("build").to_string();
        let rrole = cfg.resolve_role(&role).unwrap_or_default();
        let tool = cfg.role_field(&rrole, "tool").unwrap_or_default();
        let model = cfg.role_field(&rrole, "model").unwrap_or_default();
        let label = item["title"].as_str().unwrap_or("").to_string();
        reporter.dispatch(&id, &label, &tool, &model);

        let cfg2 = cfg.clone();
        let ws2 = ws.to_path_buf();
        let log = ldir.join(format!("item-{id}.log"));
        let item2: Value = item.clone();
        let id2 = id.clone();
        handles.push(tokio::spawn(async move {
            let _ = worker::worker_dispatch(&cfg2, &ws2, &item2, &wt, &log, itimeout).await;
            id2
        }));
        dispatched.push(id);
    }

    // Await all workers.
    for h in handles { let _ = h.await; }

    // Integrate sequentially based on each worker's result file.
    let mut merged = 0u32;
    for id in &dispatched {
        let rfile = ws.join(format!(".agentloop/results/{id}.json"));
        let result_done = std::fs::read_to_string(&rfile).ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| v["status"] == "done").unwrap_or(false);
        let branch = format!("item/{id}");

        if result_done {
            if !worktree::has_commits_ahead(ws, &branch) {
                state::set_status(&bk, id, "ready", "worker reported done but made no commits")?;
                reporter.status(id, "bounced", "", "");
            } else if worktree::merge(ws, &branch)? {
                state::set_status(&bk, id, "done", "")?;
                reporter.status(id, "merged", "", "");
                merged += 1;
            } else {
                state::set_status(&bk, id, "ready", "merge conflict; replan")?;
                reporter.status(id, "bounced", "", "");
            }
        } else {
            state::set_status(&bk, id, "ready", "worker did not report done")?;
            reporter.status(id, "failed", "", "");
        }
        worktree::remove(ws, &ws.join(format!(".agentloop/worktrees/{id}")), &branch);
        let _ = std::fs::remove_file(&rfile);
    }
    Ok(merged)
}

/// Drive iterations until DONE (0), cap/stall (1), or hard error (Err).
pub async fn run(cfg: &Config, ws: &Path, reporter: &dyn Reporter) -> Result<i32> {
    // Reporter is shared into spawned tasks via Arc; wrap a cheap clone-free handle.
    let reporter: Arc<dyn Reporter> = unsafe_arc(reporter);
    let sdir = ws.join(".agentloop/state");
    let bk = sdir.join("backlog.json");
    let maxit = cfg.max_iterations();
    let budget = Duration::from_secs(cfg.total_budget_sec());
    let start = Instant::now();
    let (mut n, mut stalls) = (0u32, 0u32);
    let mut prev_gate = String::from("init");

    while n < maxit {
        n += 1;
        if start.elapsed() >= budget { eprintln!("STOP: time budget exceeded"); return Ok(1); }

        let merged = iterate(cfg, ws, n, &reporter).await?;

        let grc = gate(ws);
        let gate_state = if grc == 0 { "pass" } else { "fail" };
        let open = state::open_count(&bk)?;
        reporter.iteration(n, merged, gate_state, open);

        if gate_state == "pass" && open == 0 { eprintln!("DONE"); return Ok(0); }

        if merged == 0 && gate_state == prev_gate {
            stalls += 1;
            if stalls >= 2 { eprintln!("STOP: no progress for 2 stalls (3 consecutive iterations)"); return Ok(1); }
        } else {
            stalls = 0;
        }
        prev_gate = gate_state.to_string();
    }
    eprintln!("STOP: max_iterations reached");
    Ok(1)
}

/// Wrap a borrowed Reporter in an Arc without taking ownership. The Arc never
/// outlives `run`, so this is sound; a Phase-2 refactor will pass Arc directly.
fn unsafe_arc(r: &dyn Reporter) -> Arc<dyn Reporter> {
    // Implemented safely in Phase 2 by threading an Arc<dyn Reporter> through run().
    // For Phase 1, construct a fresh EventLineReporter-equivalent is avoided; instead
    // callers pass EventLineReporter and we build the Arc here.
    Arc::from(Box::<dyn Reporter>::from(BoxedClone::clone_box(r)))
}
```

> **Executor note (important):** the `unsafe_arc`/`BoxedClone` shim above is a smell. Do this cleanly instead: change `run`'s signature to take `reporter: Arc<dyn Reporter>` and have `iterate` take `&Arc<dyn Reporter>`. Update the call sites (`main`/tests pass `Arc::new(EventLineReporter)`). Delete the shim. The test in Step 1 then becomes `orchestrator::run(&cfg, &ws, Arc::new(EventLineReporter)).await`. Adjust the Task 9 wiring accordingly. (Kept explicit here so the seam is obvious; implement the `Arc` signature from the start.)

- [ ] **Step 4: Apply the clean Arc signature**

Change `pub async fn run(cfg: &Config, ws: &Path, reporter: Arc<dyn Reporter>)` and `iterate(..., reporter: &Arc<dyn Reporter>)`, remove `unsafe_arc`, and update the Step-1 test call to `orchestrator::run(&cfg, &ws, Arc::new(EventLineReporter)).await`.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --test loop_test`
Expected: PASS — loop reaches DONE, `made.txt` merged to main, open==0.

- [ ] **Step 6: Commit**

```bash
git add src/orchestrator.rs tests/loop_test.rs tests/common/mod.rs
git commit -m "feat(orchestrator): async plan/dispatch/integrate/gate loop"
```

---

### Task 9: cli.rs + main wiring — bootstrap, dry-run, end-to-end

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs` (already calls `cli::run`)
- Test: `tests/cli_bootstrap_test.rs`

Ports `agentloop.sh`: arg parse, workspace bootstrap (git init, gitignore, seed state, copy templates), `--max-iterations` override, `--dry-run`, signal handling.

- [ ] **Step 1: Write the failing test** in `tests/cli_bootstrap_test.rs`

```rust
use agentloop::cli;
use std::path::Path;

#[test]
fn bootstrap_creates_state_and_git() {
    let ws = std::env::temp_dir().join(format!("alboot-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    cli::bootstrap_workspace(&ws, "build a todo app", None).unwrap();

    assert!(ws.join(".git").exists(), "git repo initialized");
    assert!(ws.join(".agentloop/state/goal.md").exists());
    assert_eq!(std::fs::read_to_string(ws.join(".agentloop/state/goal.md")).unwrap().trim(), "build a todo app");
    assert!(ws.join(".agentloop/state/backlog.json").exists());
    assert!(ws.join(".agentloop/state/master.md").exists());
    assert!(ws.join(".agentloop/config.yaml").exists());
    let gi = std::fs::read_to_string(ws.join(".gitignore")).unwrap();
    assert!(gi.contains(".agentloop/"));
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test cli_bootstrap_test`
Expected: FAIL — `cli::bootstrap_workspace` undefined.

- [ ] **Step 3: Implement `src/cli.rs`**

```rust
use anyhow::{bail, Context, Result};
use clap::Parser;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use crate::config::Config;
use crate::events::EventLineReporter;
use crate::orchestrator;

const TEMPLATE_CONFIG: &str = include_str!("../templates/config.yaml");
const TEMPLATE_MASTER: &str = include_str!("../templates/master.md");

#[derive(Parser, Debug)]
#[command(name = "agentloop", about = "Autonomous app builder")]
struct Args {
    /// The goal prompt (quote it)
    goal: String,
    /// Target dir (default: current dir)
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// config.yaml path (default: <workspace>/.agentloop/config.yaml)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Wipe existing .agentloop state and start over
    #[arg(long)]
    fresh: bool,
    /// Override caps.max_iterations
    #[arg(long)]
    max_iterations: Option<u32>,
    /// Plan only; do not dispatch workers
    #[arg(long)]
    dry_run: bool,
}

fn git(repo: &Path, args: &[&str]) -> bool {
    Command::new("git").arg("-C").arg(repo).args(args).status().map(|s| s.success()).unwrap_or(false)
}

/// Create .agentloop scaffolding, init git, seed state + config. Idempotent.
pub fn bootstrap_workspace(ws: &Path, goal: &str, config: Option<&Path>) -> Result<PathBuf> {
    std::fs::create_dir_all(ws)?;
    let ws = ws.canonicalize().unwrap_or_else(|_| ws.to_path_buf());
    let meta = ws.join(".agentloop");
    for sub in ["state", "results", "logs", "worktrees"] {
        std::fs::create_dir_all(meta.join(sub))?;
    }

    if !ws.join(".git").exists() { git(&ws, &["init", "-q"]); }
    if !git(&ws, &["config", "user.email"]) { git(&ws, &["config", "user.email", "agentloop@local"]); }
    if !git(&ws, &["config", "user.name"]) { git(&ws, &["config", "user.name", "agentloop"]); }

    let gi = ws.join(".gitignore");
    let cur = std::fs::read_to_string(&gi).unwrap_or_default();
    if !cur.lines().any(|l| l == ".agentloop/") {
        std::fs::write(&gi, format!("{cur}.agentloop/\n"))?;
    }
    // Ensure at least one commit exists so `worktree add HEAD` works.
    if !git(&ws, &["rev-parse", "HEAD"]) {
        git(&ws, &["add", "-A"]);
        git(&ws, &["commit", "-qm", "agentloop: initial commit"]);
    }

    let cfg_path = match config {
        Some(p) => p.to_path_buf(),
        None => meta.join("config.yaml"),
    };
    if !cfg_path.exists() { std::fs::write(&cfg_path, TEMPLATE_CONFIG)?; }
    let master = meta.join("state/master.md");
    if !master.exists() { std::fs::write(&master, TEMPLATE_MASTER)?; }
    let goalf = meta.join("state/goal.md");
    if !goalf.exists() { std::fs::write(&goalf, format!("{goal}\n"))?; }
    let bk = meta.join("state/backlog.json");
    if !bk.exists() { std::fs::write(&bk, "{\"items\":[]}\n")?; }

    Ok(cfg_path)
}

pub async fn run() -> Result<()> {
    let args = Args::parse();
    let ws = args.workspace.clone().unwrap_or(std::env::current_dir()?);
    if args.fresh { let _ = std::fs::remove_dir_all(ws.join(".agentloop")); }

    let cfg_path = bootstrap_workspace(&ws, &args.goal, args.config.as_deref())?;
    let ws = ws.canonicalize().unwrap_or(ws);
    let mut cfg = Config::load(&cfg_path)?;
    if let Some(m) = args.max_iterations { cfg.caps.max_iterations = Some(m); }

    if args.dry_run {
        let log = ws.join(".agentloop/logs/dryrun-planner.log");
        let ok = crate::planner::planner_run(&cfg, &ws, &log, std::time::Duration::from_secs(cfg.item_timeout_sec())).await?;
        if !ok { bail!("dry-run: planner produced invalid backlog"); }
        let bk = std::fs::read_to_string(ws.join(".agentloop/state/backlog.json"))?;
        println!("dry-run: planned backlog ->\n{bk}");
        return Ok(());
    }

    // Graceful shutdown on Ctrl-C handled by Phase-1 process-group kills per agent;
    // a top-level ctrl_c handler is added in Phase 2 with the TUI.
    let rc = orchestrator::run(&cfg, &ws, Arc::new(EventLineReporter)).await?;
    eprintln!("=== agentloop finished (rc={rc}). See {}/.agentloop/state/master.md ===", ws.display());
    std::process::exit(rc);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test cli_bootstrap_test`
Expected: PASS.

- [ ] **Step 5: Full suite + manual dry-run smoke (fake agent)**

Run: `cargo test`
Expected: all integration tests PASS.

Manual check against the fake agent (no tokens):
```bash
cargo build
# point the fake bin at the echo stub and run --dry-run is real-agent; skip unless creds present.
```

- [ ] **Step 6: Update README + remove dead bash (optional in Phase 1)**

Update `README.md`: replace the bash usage/layout with the Rust binary (`cargo build --release`, `./target/release/agentloop "<goal>" --workspace ./app`). Keep the "How it works" and config sections — they're unchanged. Leave the old `agentloop.sh`/`lib/` in place until Phase 3 cutover (or delete now if you prefer a clean break; the design treats the bash app as superseded).

- [ ] **Step 7: Commit**

```bash
git add src/cli.rs README.md
git commit -m "feat(cli): arg parse + workspace bootstrap + dry-run wiring"
```

---

## Self-Review

**Spec coverage (Phase 1 scope):**
- Module mapping (config/state/spawn/worktree/planner/worker/orchestrator + events) → Tasks 2–8. ✓
- `serde_yaml` config, python helper removed → Task 2 (no python in the binary; the test stub uses python only to avoid a `jq` test dep). ✓
- Atomic state mutations, ready/deps/parallel logic → Task 3. ✓
- Identical argv + process-group timeout + FAKE_AGENT hook → Task 4. ✓
- git worktree create/merge/remove → Task 5. ✓
- Reporter seam for Phase 2 → Task 6, threaded as `Arc<dyn Reporter>` → Task 8. ✓
- Verbatim prompts + validate/re-prompt → Task 7. ✓
- Loop control flow + termination (DONE/budget/stall/max_iter) → Task 8. ✓
- Bootstrap parity (git init, gitignore, seed state, templates), `--max-iterations`, `--dry-run` → Task 9. ✓
- Behavior-parity tests ported from `tests/` → Tasks 2,3,4,5,7,8. ✓

**Deferred to later phases (correctly out of Phase 1 scope):** TTY dashboard/TUI, question inbox (`needs_input`/blocked/answers), add-task/`requests.jsonl`, standby lifecycle, top-level Ctrl-C TUI handler.

**Type consistency:** `Reporter` methods (`dispatch`/`status`/`iteration`) are used identically in Tasks 6 and 8. `state::` function names (`read`, `backlog_valid`, `ready_items`, `open_count`, `set_status`, `increment_attempts`, `item`) are consistent across Tasks 3, 8. `spawn::{build_argv, run_with_timeout, agent_run}` consistent across Tasks 4, 7. `worktree::{create, merge, remove, has_commits_ahead}` consistent across Tasks 5, 8. `orchestrator::run` settled on `Arc<dyn Reporter>` in Task 8 Step 4 and used that way in Task 9.

**Placeholder scan:** the only intentionally-flagged smell is `unsafe_arc` in Task 8 Step 3, immediately corrected in Step 4 with the clean `Arc<dyn Reporter>` signature — no placeholders remain in the committed result.
