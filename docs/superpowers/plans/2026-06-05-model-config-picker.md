# Model Config Picker + Stale gpt-5 Default Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `ctrl-o` panel in the TUI to pick and persist tool/model/effort per role (saved to `~/.agentloop/config.json`, applied live to the running loop), plus drop the stale pinned `gpt-5` from the default config so codex uses its own default model.

**Architecture:** The TUI view-model (`src/tui.rs`) gains a `ModelConfig` view opened by `ctrl-o` from any screen; cell edits emit a new `Command::SetRole`. The app layer (`src/app.rs`) persists each `SetRole` to the config file immediately (read-modify-write of the JSON, preserving unknown keys) and forwards it to the orchestrator, which applies it to a local mutable `Config` copy at its existing command-drain points. An empty `model`/`effort` means "unset" — `build_argv` already omits the flag, so the tool's own default applies (this is also the gpt-5 fix: codex model slugs churn, so the default config stops pinning one).

**Tech Stack:** Rust, ratatui/crossterm (TUI), serde_json (config), tokio mpsc (commands). Tests: cargo test (unit + integration in `tests/`), `ratatui::backend::TestBackend` for render tests.

**Context for the engineer (read first):**
- Roles (`manager`, `architect`, `builder`, `customer`, `resolver`) are routed to a tool (`claude` or `codex`) + optional model + optional effort via `Config.routing` (`src/config.rs`). `Config::role_field` returns `None` for absent OR empty fields, and `spawn::build_argv` (`src/spawn.rs:75`) only pushes `--model`/`-m` when the field is `Some` — so "empty string = tool default" already works end-to-end for spawning.
- The TUI is a pure view-model: `AppState::on_key` maps keys to optional `Command`s; side effects (file writes, channel sends) live in `src/app.rs`. Keep it that way.
- The orchestrator (`run_interactive` in `src/orchestrator.rs`) receives `Command`s at exactly three points: the pre-goal wait loop, the working-phase `try_recv` drain, and the standby `recv`. The `Command` enum match is exhaustive everywhere, so adding a variant makes the compiler point at every site to update.
- Run all commands from the repo root: `/Users/ngthluu/choscor/one-shot-agent-loop`.

**File structure (what changes where):**
- `src/config.rs` — default config loses `"model": "gpt-5"`; new `apply_role` (in-memory) + `update_role_file` (persisted, preserves unknown JSON keys).
- `src/events.rs` — new `Command::SetRole` variant; new `fmt_tool_model` display helper; `EventLineReporter` uses it.
- `src/tui.rs` — new `View::ModelConfig` + `RoleEntry` rows + key handling + `render_model_config` + footer hints; job rows/detail use `fmt_tool_model`.
- `src/orchestrator.rs` — `run_interactive` works on a local `mut Config` and applies `SetRole` at its three command sites.
- `src/app.rs` — `run_tui` gains a `cfg_path` param, seeds the panel rows from the config, persists `SetRole` before forwarding.
- `src/cli.rs` — passes `cfg_path` to `run_tui`.
- Tests: `tests/config_test.rs`, `tests/spawn_test.rs`, `tests/loop_set_role_test.rs` (new), `tests/tui_viewmodel_test.rs`, `tests/tui_render_test.rs`, `tests/tui_helpers_test.rs`.

---

### Task 1: Drop the stale pinned `gpt-5` from the default config

The default config routes `builder` to `codex` with `"model": "gpt-5"`, a slug the codex backend no longer accepts ("gpt-5 not found"). Stop pinning a codex model entirely: codex then uses the default from the user's own `~/.codex/config.toml`, which never goes stale.

**Files:**
- Modify: `src/config.rs:17`
- Test: `tests/config_test.rs`, `tests/spawn_test.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/config_test.rs`:

```rust
#[test]
fn default_builder_has_no_pinned_model() {
    let path = temp_path("defaults/config.json");
    Config::ensure_default_config(&path).unwrap();
    let cfg = Config::load(&path).unwrap();

    assert_eq!(cfg.role_field("builder", "tool").as_deref(), Some("codex"));
    // codex model slugs churn (gpt-5 no longer exists); never pin one in the
    // default config — the tool's own default applies.
    assert_eq!(cfg.role_field("builder", "model"), None);
}
```

Append to `tests/spawn_test.rs`:

```rust
#[test]
fn codex_argv_without_model_omits_model_flag() {
    let c: Config = serde_json::from_str(
        r#"{ "routing": { "builder": { "tool": "codex", "effort": "high" } },
             "defaults": { "role": "builder" } }"#,
    )
    .unwrap();
    let a = spawn::build_argv(&c, "builder", "GO").unwrap();
    assert!(
        !a.iter().any(|s| s == "-m"),
        "no -m flag when no model is pinned: {a:?}"
    );
    assert!(a.contains(&"--yolo".to_string()));
    assert!(
        a.iter().any(|s| s == "model_reasoning_effort=high"),
        "effort still passed without a model"
    );
}
```

- [ ] **Step 2: Run tests to verify the new config test fails**

Run: `cargo test --test config_test default_builder_has_no_pinned_model`
Expected: FAIL — `assertion failed` (role_field returns `Some("gpt-5")`).

Run: `cargo test --test spawn_test codex_argv_without_model_omits_model_flag`
Expected: PASS already (build_argv handles a missing model) — it locks the behavior in.

- [ ] **Step 3: Edit the default config**

In `src/config.rs`, change line 17:

```rust
    "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" },
```

to:

```rust
    "builder": { "tool": "codex", "effort": "high" },
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test config_test --test spawn_test`
Expected: PASS (all tests in both files).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs tests/config_test.rs tests/spawn_test.rs
git commit -m "fix(config): drop stale pinned gpt-5 — unpinned codex uses its own default model"
```

---

### Task 2: Config helpers — `apply_role` (in-memory) and `update_role_file` (persisted)

Two pure helpers the picker builds on. `update_role_file` must read-modify-write the JSON as a `serde_json::Value` (NOT via the `Config` struct) so caps, `defaults`, and unknown future keys survive the rewrite.

**Files:**
- Modify: `src/config.rs`
- Test: `tests/config_test.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/config_test.rs` (note: the file already has `use agentloop::config::Config;`, `temp_path`, and `write_cfg`):

```rust
const ROUTED_JSON: &str = r#"
{
  "caps": { "max_iterations": 7 },
  "routing": {
    "manager": { "tool": "claude", "model": "opus", "effort": "high" },
    "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" }
  },
  "defaults": { "role": "builder" },
  "future_key": { "keep": true }
}
"#;

#[test]
fn update_role_file_rewrites_one_role_and_preserves_the_rest() {
    let path = write_cfg("config.json", ROUTED_JSON);

    agentloop::config::update_role_file(&path, "builder", "codex", "gpt-5.5", "medium").unwrap();

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["routing"]["builder"]["tool"], "codex");
    assert_eq!(v["routing"]["builder"]["model"], "gpt-5.5");
    assert_eq!(v["routing"]["builder"]["effort"], "medium");
    assert_eq!(v["routing"]["manager"]["model"], "opus", "other roles untouched");
    assert_eq!(v["caps"]["max_iterations"], 7, "caps preserved");
    assert_eq!(v["future_key"]["keep"], true, "unknown keys preserved");
}

#[test]
fn update_role_file_omits_empty_fields_so_tool_defaults_apply() {
    let path = write_cfg("config.json", ROUTED_JSON);

    agentloop::config::update_role_file(&path, "builder", "codex", "", "").unwrap();

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["routing"]["builder"]["tool"], "codex");
    assert!(v["routing"]["builder"].get("model").is_none(), "empty model omitted");
    assert!(v["routing"]["builder"].get("effort").is_none(), "empty effort omitted");
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.role_field("builder", "model"), None);
}

#[test]
fn update_role_file_starts_from_defaults_when_file_is_missing() {
    let path = temp_path("missing/config.json");

    agentloop::config::update_role_file(&path, "builder", "claude", "opus", "high").unwrap();

    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.role_field("builder", "tool").as_deref(), Some("claude"));
    assert_eq!(cfg.role_field("builder", "model").as_deref(), Some("opus"));
    assert_eq!(
        cfg.role_field("manager", "tool").as_deref(),
        Some("claude"),
        "the other default roles are seeded too"
    );
}

#[test]
fn update_role_file_refuses_to_clobber_invalid_json() {
    let path = write_cfg("config.json", "{ this is not json");

    let err = agentloop::config::update_role_file(&path, "builder", "codex", "", "")
        .unwrap_err()
        .to_string();
    assert!(err.contains("parse config json"), "unexpected error: {err}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{ this is not json",
        "a hand-edited broken file is never overwritten"
    );
}

#[test]
fn apply_role_updates_in_memory_routing_and_clears_empty_fields() {
    let mut cfg: Config = serde_json::from_str(
        r#"{ "routing": { "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" } },
             "defaults": { "role": "builder" } }"#,
    )
    .unwrap();

    agentloop::config::apply_role(&mut cfg, "builder", "claude", "opus", "");
    assert_eq!(cfg.role_field("builder", "tool").as_deref(), Some("claude"));
    assert_eq!(cfg.role_field("builder", "model").as_deref(), Some("opus"));
    assert_eq!(cfg.role_field("builder", "effort"), None, "empty clears the field");

    // Unknown role: the entry is created.
    agentloop::config::apply_role(&mut cfg, "reviewer", "claude", "sonnet", "medium");
    assert_eq!(cfg.role_field("reviewer", "tool").as_deref(), Some("claude"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test config_test`
Expected: FAIL to compile — `cannot find function update_role_file in module agentloop::config` (and `apply_role`).

- [ ] **Step 3: Implement the helpers**

Append to `src/config.rs` (after the `impl Config` block, before `fn non_empty_env_path`):

```rust
/// Set one role's routing on an in-memory Config (TUI picker / Command::SetRole).
/// Empty strings clear the field so the tool's own default applies.
pub fn apply_role(cfg: &mut Config, role: &str, tool: &str, model: &str, effort: &str) {
    let norm = |s: &str| {
        let s = s.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    };
    let entry = cfg.routing.entry(role.to_string()).or_default();
    entry.tool = norm(tool);
    entry.model = norm(model);
    entry.effort = norm(effort);
}

/// Read-modify-write `routing.<role>` in the config file, preserving every other
/// key (caps, defaults, unknown future fields). A missing file starts from the
/// default config; an unparseable one is an error (never clobber a hand-edited
/// file). Empty fields are omitted, so the tool's own default applies.
pub fn update_role_file(
    path: &Path,
    role: &str,
    tool: &str,
    model: &str,
    effort: &str,
) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => DEFAULT_CONFIG_JSON.to_string(),
    };
    let mut root: serde_json::Value = serde_json::from_str(&text).context("parse config json")?;
    let obj = root
        .as_object_mut()
        .context("config root is not a JSON object")?;
    let routing = obj
        .entry("routing")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("config routing is not a JSON object")?;

    let mut entry = serde_json::Map::new();
    for (key, value) in [("tool", tool), ("model", model), ("effort", effort)] {
        let value = value.trim();
        if !value.is_empty() {
            entry.insert(key.to_string(), serde_json::Value::String(value.to_string()));
        }
    }
    routing.insert(role.to_string(), serde_json::Value::Object(entry));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create config dir {}", parent.display()))?;
    }
    let pretty = serde_json::to_string_pretty(&root).context("serialize config")?;
    std::fs::write(path, format!("{pretty}\n"))
        .with_context(|| format!("write config {}", path.display()))?;
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test config_test`
Expected: PASS (all tests).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs tests/config_test.rs
git commit -m "feat(config): apply_role + update_role_file routing helpers"
```

---

### Task 3: `Command::SetRole` + orchestrator handling

Add the command variant and make `run_interactive` apply it to a local mutable config copy at all three command-drain points. A `SetRole` must never start or re-engage the run by itself.

**Files:**
- Modify: `src/events.rs:87-93` (Command enum)
- Modify: `src/orchestrator.rs:994-1108` (`run_interactive`)
- Test: `tests/loop_set_role_test.rs` (new)

- [ ] **Step 1: Write the failing test**

Create `tests/loop_set_role_test.rs`:

```rust
use agentloop::config::Config;
use agentloop::events::{Command, EventLineReporter, Reporter};
use agentloop::orchestrator;
use std::sync::Arc;

#[tokio::test]
async fn set_role_before_start_is_consumed_without_starting_work() {
    let ws = std::env::temp_dir().join(format!(
        "setrole-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&ws).unwrap();
    let cfg: Config = serde_json::from_str(
        r#"{ "routing": { "builder": { "tool": "codex", "effort": "high" } },
             "defaults": { "role": "builder" } }"#,
    )
    .unwrap();
    let (ctx, mut crx) = tokio::sync::mpsc::unbounded_channel::<Command>();
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    // SetRole then Quit: the pre-goal wait loop must apply the routing edit and
    // keep waiting (not treat it as a run start), then exit cleanly on Quit.
    ctx.send(Command::SetRole {
        role: "builder".into(),
        tool: "claude".into(),
        model: "opus".into(),
        effort: String::new(),
    })
    .unwrap();
    ctx.send(Command::Quit).unwrap();

    let rc = orchestrator::run_interactive(&cfg, &ws, rep, &mut crx)
        .await
        .unwrap();
    assert_eq!(rc, 0);
    assert!(
        !ws.join(".agentloop/logs/iter-1").exists(),
        "no iteration ran"
    );
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test loop_set_role_test`
Expected: FAIL to compile — `no variant named SetRole found for enum Command`.

- [ ] **Step 3: Add the Command variant**

In `src/events.rs`, change the `Command` enum to:

```rust
/// UI -> orchestrator.
#[derive(Debug, Clone)]
pub enum Command {
    StartRun { goal: String },
    AddTask { request: String },
    /// Update one role's routing (empty field = unset → the tool's own
    /// default). Persisted to the config file by the TUI layer; applied to the
    /// running loop's in-memory config at the orchestrator's next command drain.
    SetRole {
        role: String,
        tool: String,
        model: String,
        effort: String,
    },
    Quit,
}
```

- [ ] **Step 4: Handle it in `run_interactive` (compiler-driven: `cargo build` now errors on every non-exhaustive match)**

In `src/orchestrator.rs`, `run_interactive`:

(a) Right after the signature's opening brace (before `let bk = ...`), add a local mutable copy. Routing edits apply to this run without a restart; caps are read once below — `SetRole` only touches routing:

```rust
    // Routing edits (Command::SetRole) apply mid-run: work on a local mutable
    // copy of the config. Caps are read once — SetRole only touches routing.
    let mut cfg = cfg.clone();
```

(b) The single `iterate(cfg, ws, n, &reporter)` call in this function (in the working-phase loop) becomes `iterate(&cfg, ws, n, &reporter)` (the local is now an owned `Config`). Note: only inside `run_interactive` — `run()` (headless) is unchanged.

(c) Pre-goal wait loop — add an arm (the edit must apply even before the run starts):

```rust
            Some(Command::SetRole { role, tool, model, effort }) => {
                crate::config::apply_role(&mut cfg, &role, &tool, &model, &effort);
            }
```

(d) Working-phase drain (`while let Ok(cmd) = crx.try_recv()`) — add the same arm:

```rust
                    Command::SetRole { role, tool, model, effort } => {
                        crate::config::apply_role(&mut cfg, &role, &tool, &model, &effort);
                    }
```

(e) Standby phase — a routing edit must NOT re-engage the loop (it would burn an iteration just for changing models). Wrap the single `match crx.recv()` in a loop where `AddTask`/`StartRun` `break` to re-engage and `SetRole` continues waiting:

```rust
        // --- STANDBY phase ---
        reporter.standby(&standby_reason);
        loop {
            match crx.recv().await {
                None | Some(Command::Quit) => return Ok(0),
                Some(Command::AddTask { request }) => {
                    if let Err(e) = crate::requests::append(ws, &request) {
                        reporter.status(
                            "addtask",
                            "failed",
                            "",
                            "",
                            &format!("could not queue request: {e:#}"),
                        );
                    }
                    break;
                }
                Some(Command::StartRun { .. }) => break,
                // Routing edits don't re-engage the loop; keep waiting for work.
                Some(Command::SetRole { role, tool, model, effort }) => {
                    crate::config::apply_role(&mut cfg, &role, &tool, &model, &effort);
                }
            }
        }
```

- [ ] **Step 5: Build and run the full test suite (the new variant must compile everywhere)**

Run: `cargo build && cargo test --test loop_set_role_test`
Expected: clean build, test PASS.

Run: `cargo test`
Expected: PASS (no other match site missed — the compiler caught them in Step 4; this confirms no behavior regressed).

- [ ] **Step 6: Commit**

```bash
git add src/events.rs src/orchestrator.rs tests/loop_set_role_test.rs
git commit -m "feat(loop): Command::SetRole applies routing edits to the running loop"
```

---

### Task 4: TUI view-model — ctrl-o model-config panel (state machine)

`ctrl-o` toggles a `ModelConfig` view from any screen. Arrow keys move a cell cursor over role rows × (tool, model, effort) columns. Enter on the tool column cycles claude↔codex and commits; Enter on model/effort opens an inline text edit (Enter commits, Esc cancels). Every commit emits `Command::SetRole` with the full row snapshot. Esc (when not editing) closes the panel back to the previous view.

**Files:**
- Modify: `src/tui.rs`
- Test: `tests/tui_viewmodel_test.rs`

- [ ] **Step 1: Write the failing tests**

In `tests/tui_viewmodel_test.rs`, change line 2 from `use agentloop::tui::AppState;` to:

```rust
use agentloop::tui::{AppState, RoleEntry};
```

Then append:

```rust
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// An AppState seeded with two routing rows (sorted, as app.rs provides them).
fn routed(goal: &str) -> AppState {
    let mut s = AppState::new(goal.into());
    s.set_routing(vec![
        RoleEntry {
            role: "architect".into(),
            tool: "claude".into(),
            model: "opus".into(),
            effort: "high".into(),
        },
        RoleEntry {
            role: "builder".into(),
            tool: "codex".into(),
            model: String::new(), // unpinned: tool default
            effort: "high".into(),
        },
    ]);
    s
}

#[test]
fn ctrl_o_opens_model_config_and_esc_returns_to_the_previous_view() {
    // From goal entry…
    let mut s = routed("");
    assert!(s.in_goal_entry());
    assert!(s.on_key(ctrl('o')).is_none());
    assert!(s.in_model_config());
    assert!(s.on_key(key(KeyCode::Esc)).is_none());
    assert!(s.in_goal_entry(), "esc returns to where ctrl-o was pressed");

    // …and from the list view.
    let mut s = routed("g");
    s.on_key(key(KeyCode::Enter)); // commit goal -> List
    s.on_key(ctrl('o'));
    assert!(s.in_model_config());
    s.on_key(ctrl('o')); // ctrl-o also closes
    assert!(!s.in_model_config() && !s.in_goal_entry());
}

#[test]
fn arrows_move_the_cell_cursor_within_bounds() {
    let mut s = routed("");
    s.on_key(ctrl('o'));
    assert_eq!(s.model_selection(), (0, 0));
    s.on_key(key(KeyCode::Up)); // already at the top
    s.on_key(key(KeyCode::Left)); // already leftmost
    assert_eq!(s.model_selection(), (0, 0));
    s.on_key(key(KeyCode::Down));
    s.on_key(key(KeyCode::Right));
    s.on_key(key(KeyCode::Right));
    s.on_key(key(KeyCode::Right)); // clamped at effort
    assert_eq!(s.model_selection(), (1, 2));
    s.on_key(key(KeyCode::Down)); // clamped at the last row
    assert_eq!(s.model_selection(), (1, 2));
}

#[test]
fn enter_on_tool_cycles_claude_codex_and_emits_set_role() {
    let mut s = routed("");
    s.on_key(ctrl('o')); // row 0 = architect, col 0 = tool
    let cmd = s.on_key(key(KeyCode::Enter));
    assert!(
        matches!(
            cmd,
            Some(Command::SetRole { ref role, ref tool, ref model, ref effort })
                if role == "architect" && tool == "codex" && model == "opus" && effort == "high"
        ),
        "got {cmd:?}"
    );
    assert_eq!(s.model_rows()[0].tool, "codex");
    // Cycles back.
    let cmd = s.on_key(key(KeyCode::Enter));
    assert!(matches!(cmd, Some(Command::SetRole { ref tool, .. }) if tool == "claude"));
}

#[test]
fn editing_model_commits_on_enter_and_emits_set_role() {
    let mut s = routed("");
    s.on_key(ctrl('o'));
    s.on_key(key(KeyCode::Down)); // builder row
    s.on_key(key(KeyCode::Right)); // model column
    assert!(s.on_key(key(KeyCode::Enter)).is_none(), "enter starts the edit");
    assert_eq!(s.model_edit_buffer(), Some(""), "unpinned model edits from empty");
    for c in "gpt-5.5".chars() {
        s.on_key(key(KeyCode::Char(c)));
    }
    let cmd = s.on_key(key(KeyCode::Enter));
    assert!(
        matches!(
            cmd,
            Some(Command::SetRole { ref role, ref model, .. })
                if role == "builder" && model == "gpt-5.5"
        ),
        "got {cmd:?}"
    );
    assert_eq!(s.model_rows()[1].model, "gpt-5.5");
    assert_eq!(s.model_edit_buffer(), None, "edit closed after commit");
}

#[test]
fn esc_cancels_an_edit_without_committing() {
    let mut s = routed("");
    s.on_key(ctrl('o'));
    s.on_key(key(KeyCode::Right)); // architect / model
    s.on_key(key(KeyCode::Enter)); // edit "opus"
    s.on_key(key(KeyCode::Char('X')));
    assert!(s.on_key(key(KeyCode::Esc)).is_none());
    assert_eq!(s.model_rows()[0].model, "opus", "value unchanged");
    assert!(s.in_model_config(), "esc in an edit closes the edit, not the panel");
}

#[test]
fn clearing_a_model_commits_empty_meaning_tool_default() {
    let mut s = routed("");
    s.on_key(ctrl('o'));
    s.on_key(key(KeyCode::Right)); // architect / model
    s.on_key(key(KeyCode::Enter)); // edit "opus"
    for _ in 0..4 {
        s.on_key(key(KeyCode::Backspace));
    }
    let cmd = s.on_key(key(KeyCode::Enter));
    assert!(
        matches!(cmd, Some(Command::SetRole { ref model, .. }) if model.is_empty()),
        "empty model = unset = tool default, got {cmd:?}"
    );
    assert_eq!(s.model_rows()[0].model, "");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test tui_viewmodel_test`
Expected: FAIL to compile — `no RoleEntry in tui`, no `set_routing`/`in_model_config`/`model_selection`/`model_rows`/`model_edit_buffer` methods.

- [ ] **Step 3: Implement the view-model**

In `src/tui.rs`:

(a) After the `Job` impl (line ~21), add the row type:

```rust
/// One role's routing as shown/edited in the ctrl-o model-config panel.
/// Empty model/effort = unset: the tool's own default applies.
#[derive(Clone, Debug, PartialEq)]
pub struct RoleEntry {
    pub role: String,
    pub tool: String,
    pub model: String,
    pub effort: String,
}

/// The full row snapshot as a SetRole command (empty fields = tool default).
fn set_role_cmd(row: &RoleEntry) -> Command {
    Command::SetRole {
        role: row.role.clone(),
        tool: row.tool.clone(),
        model: row.model.clone(),
        effort: row.effort.clone(),
    }
}
```

(b) Add the view variant:

```rust
#[derive(PartialEq, Clone, Copy)]
enum View {
    GoalEntry,
    List,
    JobDetail,
    ModelConfig,
}
```

(c) Add fields to `AppState` (after `started`):

```rust
    // ctrl-o model-config panel state.
    roles: Vec<RoleEntry>,
    prev_view: View,
    cfg_row: usize,
    cfg_col: usize, // 0 = tool, 1 = model, 2 = effort
    cfg_edit: Option<String>,
```

and initialize them in `AppState::new` (after `started: std::time::Instant::now(),`):

```rust
            roles: vec![],
            prev_view: View::GoalEntry,
            cfg_row: 0,
            cfg_col: 0,
            cfg_edit: None,
```

(d) Replace `pub fn on_key` with a version that intercepts ctrl-o first (chars with CONTROL must never reach the per-view text inputs):

```rust
    /// Map a key to an optional Command. Returns None when the key only changes UI state.
    pub fn on_key(&mut self, k: KeyEvent) -> Option<Command> {
        // ctrl-o toggles the model-routing panel from any view.
        if k.code == KeyCode::Char('o')
            && k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
        {
            if self.view == View::ModelConfig {
                self.cfg_edit = None;
                self.view = self.prev_view;
            } else {
                self.prev_view = self.view;
                self.view = View::ModelConfig;
            }
            return None;
        }
        match self.view {
            View::GoalEntry => self.on_key_goal_entry(k),
            View::JobDetail => self.on_key_job_detail(k),
            View::List => self.on_key_list(k),
            View::ModelConfig => self.on_key_model_config(k),
        }
    }
```

(e) Add the panel key handler (next to the other `on_key_*` methods):

```rust
    fn on_key_model_config(&mut self, k: KeyEvent) -> Option<Command> {
        // Editing a cell: keys go to the edit buffer until Enter commits / Esc cancels.
        if self.cfg_edit.is_some() {
            match k.code {
                KeyCode::Enter => {
                    let value = self.cfg_edit.take().unwrap_or_default().trim().to_string();
                    let col = self.cfg_col;
                    let row = self.roles.get_mut(self.cfg_row)?;
                    if col == 1 {
                        row.model = value;
                    } else {
                        row.effort = value;
                    }
                    return Some(set_role_cmd(row));
                }
                KeyCode::Esc => self.cfg_edit = None,
                KeyCode::Backspace => {
                    if let Some(buf) = self.cfg_edit.as_mut() {
                        buf.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(buf) = self.cfg_edit.as_mut() {
                        buf.push(c);
                    }
                }
                _ => {}
            }
            return None;
        }
        match k.code {
            KeyCode::Esc => {
                self.view = self.prev_view;
                None
            }
            KeyCode::Up => {
                self.cfg_row = self.cfg_row.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                if self.cfg_row + 1 < self.roles.len() {
                    self.cfg_row += 1;
                }
                None
            }
            KeyCode::Left => {
                self.cfg_col = self.cfg_col.saturating_sub(1);
                None
            }
            KeyCode::Right => {
                if self.cfg_col < 2 {
                    self.cfg_col += 1;
                }
                None
            }
            KeyCode::Enter => {
                let col = self.cfg_col;
                let row = self.roles.get_mut(self.cfg_row)?;
                if col == 0 {
                    // Only two known tools; Enter cycles instead of free text.
                    row.tool = if row.tool == "claude" {
                        "codex".into()
                    } else {
                        "claude".into()
                    };
                    Some(set_role_cmd(row))
                } else {
                    self.cfg_edit = Some(if col == 1 {
                        row.model.clone()
                    } else {
                        row.effort.clone()
                    });
                    None
                }
            }
            _ => None,
        }
    }
```

(f) Add the public accessors (next to `in_goal_entry`):

```rust
    /// Seed the ctrl-o panel rows (sorted by role; from the loaded config).
    pub fn set_routing(&mut self, roles: Vec<RoleEntry>) {
        self.roles = roles;
        self.cfg_row = 0;
        self.cfg_col = 0;
    }

    pub fn in_model_config(&self) -> bool {
        self.view == View::ModelConfig
    }

    pub fn model_rows(&self) -> &[RoleEntry] {
        &self.roles
    }

    /// (row, col) of the panel's cell cursor; col 0 = tool, 1 = model, 2 = effort.
    pub fn model_selection(&self) -> (usize, usize) {
        (self.cfg_row, self.cfg_col)
    }

    /// The in-progress cell edit, if any.
    pub fn model_edit_buffer(&self) -> Option<&str> {
        self.cfg_edit.as_deref()
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test tui_viewmodel_test`
Expected: PASS (all tests, old and new).

- [ ] **Step 5: Commit**

```bash
git add src/tui.rs tests/tui_viewmodel_test.rs
git commit -m "feat(tui): ctrl-o model-config view-model — per-role tool/model/effort editing"
```

---

### Task 5: TUI render — panel + footer hints

Render the panel full-screen when active, and advertise the shortcut: a hint line on the goal-entry screen (the unused 5th layout chunk) and `[ctrl-o] models` in the list-view footer.

**Files:**
- Modify: `src/tui.rs` (`render`, `render_goal_entry`, footer hint strings; new `render_model_config`)
- Test: `tests/tui_render_test.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/tui_render_test.rs`:

```rust
fn routed(goal: &str) -> AppState {
    use agentloop::tui::RoleEntry;
    let mut s = AppState::new(goal.into());
    s.set_routing(vec![
        RoleEntry {
            role: "architect".into(),
            tool: "claude".into(),
            model: "opus".into(),
            effort: "high".into(),
        },
        RoleEntry {
            role: "builder".into(),
            tool: "codex".into(),
            model: String::new(),
            effort: "high".into(),
        },
    ]);
    s
}

#[test]
fn model_config_panel_renders_roles_values_and_defaults() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut s = routed("");
    s.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));

    let backend = TestBackend::new(100, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();

    assert!(find(&term, "Model routing").is_some(), "panel title rendered");
    assert!(find(&term, "architect").is_some());
    assert!(find(&term, "opus").is_some());
    assert!(
        find(&term, "(default)").is_some(),
        "unpinned model shows (default)"
    );
    assert!(find(&term, "[esc] close").is_some(), "close hint shown");
}

#[test]
fn footer_hints_advertise_ctrl_o() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    // Goal-entry screen.
    let s = routed("");
    let backend = TestBackend::new(100, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "ctrl-o").is_some(),
        "goal entry advertises the model picker"
    );

    // List view footer.
    let mut s = routed("g");
    s.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let backend = TestBackend::new(120, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(
        find(&term, "[ctrl-o] models").is_some(),
        "list footer advertises the model picker"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test tui_render_test`
Expected: FAIL — `find(&term, "Model routing")` is None (panel not rendered; ctrl-o flips the view but `render` doesn't know it), and "ctrl-o" hints absent.

- [ ] **Step 3: Implement the rendering**

In `src/tui.rs`:

(a) At the top of `pub fn render` (before the goal-entry early return):

```rust
    if s.in_model_config() {
        render_model_config(f, s, area);
        return;
    }
```

(b) Add the panel renderer (after `render_goal_entry`):

```rust
fn render_model_config(f: &mut ratatui::Frame, s: &AppState, area: ratatui::layout::Rect) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Paragraph};

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let (sel_row, sel_col) = s.model_selection();
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        format!(" {:<12} {:<10} {:<24} {:<10}", "role", "tool", "model", "effort"),
        Style::default().fg(Color::DarkGray),
    ))];
    for (i, row) in s.model_rows().iter().enumerate() {
        let cell = |col: usize, value: &str| -> Span<'static> {
            // The selected cell is highlighted; an in-progress edit shows the
            // buffer with a cursor mark instead of the stored value.
            let editing = i == sel_row && col == sel_col && s.model_edit_buffer().is_some();
            let text = if editing {
                format!("{}▏", s.model_edit_buffer().unwrap_or(""))
            } else if value.is_empty() {
                "(default)".to_string()
            } else {
                value.to_string()
            };
            let width = match col {
                0 => 10,
                1 => 24,
                _ => 10,
            };
            let padded = format!("{text:<width$}");
            if i == sel_row && col == sel_col {
                Span::styled(
                    padded,
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw(padded)
            }
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:<12} ", row.role),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            cell(0, &row.tool),
            Span::raw(" "),
            cell(1, &row.model),
            Span::raw(" "),
            cell(2, &row.effort),
        ]));
    }
    let panel = Paragraph::new(lines).block(
        Block::default()
            .title(" Model routing — saved to config.json ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(panel, chunks[0]);

    let hint =
        " [↑↓←→] move  [enter] edit / cycle tool  [esc] close  ·  empty model/effort = tool default";
    f.render_widget(
        Paragraph::new(Line::from(hint)).style(Style::default().fg(Color::DarkGray)),
        chunks[1],
    );
}
```

Note the header row pads to the same widths as the cells (role 12 inside `" {:<12} "`, tool 10, model 24, effort 10) so columns align.

(c) In `render_goal_entry`, the layout's last chunk (`chunks[4]`, `Length(1)`) is currently unused — render the shortcut hint there (the Continue-button line is already ~76 chars wide; appending there would overflow 80-col terminals):

```rust
    let hint = Paragraph::new(Line::from(" [ctrl-o] models — pick tool/model/effort per role"))
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(hint, chunks[4]);
```

(d) In `render` (list-view footer), add `[ctrl-o] models` to both hint strings:

```rust
    let hint = if s.standby {
        " ✓ standby · [enter] submit  [shift+enter] newline  [↑↓] jobs  [ctrl-o] models  [esc] clear  [q] quit"
    } else {
        " [enter] submit  [shift+enter] newline  [↑↓] jobs  [ctrl-o] models  [esc] clear  [q] quit"
    };
```

(These run to ~90 chars; on narrow terminals the tail truncates harmlessly — same as the existing standby variant. The render test uses a 120-col TestBackend.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test tui_render_test --test tui_viewmodel_test`
Expected: PASS (all tests).

- [ ] **Step 5: Commit**

```bash
git add src/tui.rs tests/tui_render_test.rs
git commit -m "feat(tui): render the ctrl-o model-routing panel and advertise the shortcut"
```

---

### Task 6: Display polish — `fmt_tool_model` for unpinned models

With no pinned model, dispatches report `model=""` and the UI shows `[codex/]` / `codex/`. Show just the tool name instead.

**Files:**
- Modify: `src/events.rs` (helper + `EventLineReporter`)
- Modify: `src/tui.rs` (job row + job-detail header)
- Test: `tests/tui_helpers_test.rs`, `tests/tui_render_test.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/tui_helpers_test.rs`:

```rust
#[test]
fn fmt_tool_model_omits_the_slash_when_no_model_is_pinned() {
    use agentloop::events::fmt_tool_model;
    assert_eq!(fmt_tool_model("codex", ""), "codex");
    assert_eq!(fmt_tool_model("codex", "gpt-5.5"), "codex/gpt-5.5");
    assert_eq!(fmt_tool_model("claude", "opus"), "claude/opus");
}
```

Append to `tests/tui_render_test.rs`:

```rust
#[test]
fn job_row_with_unpinned_model_shows_tool_only() {
    let mut s = started("goal");
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: String::new(), // unpinned: tool default
        log_path: None,
    });
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(find(&term, "[codex]").is_some(), "tool shown without a slash");
    assert!(find(&term, "[codex/]").is_none(), "no dangling slash");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test tui_helpers_test`
Expected: FAIL to compile — `no fmt_tool_model in events`.

- [ ] **Step 3: Implement**

(a) In `src/events.rs`, add near the top (after the `Reporter` trait):

```rust
/// "tool/model" for display; just "tool" when no model is pinned (the tool's
/// own default applies — codex model slugs churn, so unpinned is the norm).
pub fn fmt_tool_model(tool: &str, model: &str) -> String {
    if model.is_empty() {
        tool.to_string()
    } else {
        format!("{tool}/{model}")
    }
}
```

(b) In `EventLineReporter::dispatch`, replace the body with:

```rust
        eprintln!(
            "{}  dispatch {:<10} {}  {}",
            hms(),
            id,
            fmt_tool_model(tool, model),
            label
        );
```

and in `EventLineReporter::status`:

```rust
        if note.is_empty() {
            eprintln!(
                "{}  {:<9} {:<10} {}",
                hms(),
                status,
                id,
                fmt_tool_model(tool, model)
            );
        } else {
            eprintln!(
                "{}  {:<9} {:<10} {}  {}",
                hms(),
                status,
                id,
                fmt_tool_model(tool, model),
                note
            );
        }
```

(c) In `src/tui.rs` `render` (jobs list), change the row line:

```rust
                let line = format!(
                    " {} {} [{}]  {}",
                    glyph,
                    j.label,
                    crate::events::fmt_tool_model(&j.tool, &j.model),
                    dur
                );
```

(d) In `render_job_detail`, change the header line:

```rust
                vec![Line::from(format!(
                    " status: {} {}   tool: {}   {}",
                    status_glyph(&j.status),
                    j.status,
                    crate::events::fmt_tool_model(&j.tool, &j.model),
                    dur
                ))],
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test tui_helpers_test --test tui_render_test`
Expected: PASS (all tests; the existing `jobs_pane_renders_without_inbox` still passes — `fmt_tool_model("codex", "gpt-5")` = `codex/gpt-5`).

- [ ] **Step 5: Commit**

```bash
git add src/events.rs src/tui.rs tests/tui_helpers_test.rs tests/tui_render_test.rs
git commit -m "feat(ui): show bare tool name when no model is pinned"
```

---

### Task 7: Wire it together — cfg_path into the TUI, persist SetRole, seed panel rows

The single integration point: `run_tui` learns the config path, seeds the panel from the loaded config, and persists each `SetRole` to disk *before* forwarding it (the orchestrator only drains commands between rounds; the file write must not wait on that).

**Files:**
- Modify: `src/app.rs:133-213` (`run_tui`)
- Modify: `src/cli.rs:367` (call site)

- [ ] **Step 1: Change the `run_tui` signature and seed the rows**

In `src/app.rs`, change the signature:

```rust
pub async fn run_tui(cfg: Config, cfg_path: PathBuf, ws: PathBuf, goal: String) -> Result<i32> {
```

and right after `let mut state = AppState::new(goal);` add:

```rust
    // Seed the ctrl-o model-routing panel from the loaded config (BTreeMap:
    // already sorted by role).
    state.set_routing(
        cfg.routing
            .iter()
            .map(|(name, r)| tui::RoleEntry {
                role: name.clone(),
                tool: r.tool.clone().unwrap_or_default(),
                model: r.model.clone().unwrap_or_default(),
                effort: r.effort.clone().unwrap_or_default(),
            })
            .collect(),
    );
```

(`cfg` is still owned here — the orchestrator task got its own clone, `cfg_o`, above.)

- [ ] **Step 2: Persist SetRole before forwarding**

In the key-handling branch of the event loop, replace:

```rust
                if let Some(cmd) = state.on_key(k) {
                    let quit = matches!(cmd, Command::Quit);
                    let _ = ctx.send(cmd);
                    if quit {
                        break Ok(());
                    }
                }
```

with:

```rust
                if let Some(cmd) = state.on_key(k) {
                    // Persist routing edits immediately: the orchestrator only
                    // drains commands between rounds, and the pick must survive
                    // this session even if the loop never reaches a drain point.
                    if let Command::SetRole {
                        role,
                        tool,
                        model,
                        effort,
                    } = &cmd
                    {
                        if let Err(e) =
                            crate::config::update_role_file(&cfg_path, role, tool, model, effort)
                        {
                            eprintln!("failed to save model routing: {e:#}");
                        }
                    }
                    let quit = matches!(cmd, Command::Quit);
                    let _ = ctx.send(cmd);
                    if quit {
                        break Ok(());
                    }
                }
```

(`eprintln!` lands in `.agentloop/logs/run.log` via the existing `StderrRedirect`.)

- [ ] **Step 3: Update the call site**

In `src/cli.rs` line 367, change:

```rust
        let rc = crate::app::run_tui(cfg, ws.clone(), goal_text).await?;
```

to:

```rust
        let rc = crate::app::run_tui(cfg, cfg_path, ws.clone(), goal_text).await?;
```

(`cfg_path` is the already-resolved `PathBuf` from earlier in `run()`; it is not used after this point.)

- [ ] **Step 4: Build and run the whole suite**

Run: `cargo build && cargo test`
Expected: clean build, all tests PASS.

- [ ] **Step 5: Manual smoke test (TTY behavior can't be covered by TestBackend)**

In a scratch dir:

```bash
cd "$(mktemp -d)" && AGENTLOOP_CONFIG="$PWD/config.json" /Users/ngthluu/choscor/one-shot-agent-loop/target/debug/agentloop
```

On the entry screen: confirm the `[ctrl-o] models` hint renders; press `ctrl-o`; arrow to builder/model; Enter; type `gpt-5.5`; Enter; Esc; then Ctrl-C to quit **without** starting a run. Verify the pick persisted:

```bash
cat config.json   # expect routing.builder.model == "gpt-5.5", other roles intact
```

Expected: the file contains the full default config with only `routing.builder` rewritten.

- [ ] **Step 6: Commit**

```bash
git add src/app.rs src/cli.rs
git commit -m "feat: wire ctrl-o model picker — persist to config.json and apply to the running loop"
```

---

### Task 8: Documentation

**Files:**
- Modify: `README.md` (Options/TUI section ~line 75-90, Routing bullet ~line 104)

- [ ] **Step 1: Document the picker and the unpinned default**

In `README.md`, find the **Routing** bullet under "How it works":

```markdown
- **Routing:** global `~/.agentloop/config.json` maps roles to tool/model/effort.
  The available roles are `manager`, `architect`, `builder`, `customer`, and
  `resolver`. Tool permission switches are fixed by agentloop: `claude` always gets
  `--dangerously-skip-permissions`, and `codex` always gets `--yolo`.
```

and extend it to:

```markdown
- **Routing:** global `~/.agentloop/config.json` maps roles to tool/model/effort.
  The available roles are `manager`, `architect`, `builder`, `customer`, and
  `resolver`. Tool permission switches are fixed by agentloop: `claude` always gets
  `--dangerously-skip-permissions`, and `codex` always gets `--yolo`.
  An omitted `model`/`effort` leaves the choice to the tool itself — the default
  config pins no codex model, because codex model slugs churn (`gpt-5` no longer
  exists). Press `ctrl-o` in the TUI to pick and persist tool/model/effort per
  role: edits are written back to the config file immediately and apply to the
  running loop from its next dispatch.
```

- [ ] **Step 2: Verify, run the suite one last time, and commit**

Run: `cargo test && cargo clippy --all-targets`
Expected: tests PASS, no new clippy warnings.

```bash
git add README.md
git commit -m "docs: ctrl-o model picker and unpinned codex default"
```

---

## Out of scope (deliberately)

- **No model auto-discovery:** neither `claude` nor `codex` exposes a "list models" command worth shelling out to; the model cell is free text, and empty = tool default is the safe escape hatch.
- **No per-workspace config:** routing stays global (`~/.agentloop/config.json` / `$AGENTLOOP_CONFIG`), matching existing behavior; the picker writes wherever `--config`/`$AGENTLOOP_CONFIG` resolved.
- **No caps editing in the panel:** roles only. `--max-iterations` already covers the common cap override.
- **Headless runs:** `SetRole` is TUI-only; headless runs read the (already-persisted) config file at startup, which the picker keeps current.
