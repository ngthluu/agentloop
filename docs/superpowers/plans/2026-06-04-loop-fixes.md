# Loop Fixes: Customer Status Display, Usage-Limit Auto-Continue, Inbox Removal, Backlog Liveness

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix four defects in agentloop: (1) customer-review jobs show `?` and keep ticking after approval; (2) agent runs die permanently on provider usage limits instead of waiting and continuing; (3) the question Inbox parks work on a human — replace it with an automatic "you decide" responder and remove the Inbox UI; (4) the loop stalls with open backlog items it can never dispatch (reproduced in `/Users/ngthluu/choscor/test-chat-app`).

**Architecture:** All changes are inside the single `agentloop` crate. The TUI fix is a status-vocabulary fix in `tui.rs`. Usage-limit handling is a new pure module `limits.rs` (detect + wait math) plus a retry loop in `spawn::agent_run`. Inbox removal rewires the orchestrator's `needs_input` path to auto-answer and deletes the question/answer plumbing from `events.rs`/`tui.rs`. Backlog liveness adds a deterministic `repair_backlog` pass after each manager round plus hardened manager-prompt rules.

**Tech Stack:** Rust (edition 2021), tokio, ratatui/crossterm, serde_json. Offline test suite via `FAKE_AGENT` stub scripts (`tests/common/mod.rs`) — `cargo test` spends no tokens.

**Verification reference (issue 4):** `/Users/ngthluu/choscor/test-chat-app/.agentloop/state/` exhibits all three zombie modes this plan repairs:
- `task-4`, `task-5`, `task-7`: `status:"in_progress"` with no `tasks/<id>/builders.json` → never re-architected (only `ready` items reach the architect).
- `task-9-b2r`, `task-9-b3r`, `task-9-b5`, `task-9-b6` (business backlog): deps reference `task-9-b1`/`task-9-b4` which are builder ids, not backlog items → `ready_items` can never satisfy them.
- `task-9`: valid plan, but remaining `ready` builders (`b5`,`b6`) dep on `failed` builders (`b2`,`b3`) → `ready_builders` is empty forever while `all_builders_done` is false.

**Background invariants you must not break:**
- `tests/roles_prompt_test.rs::manager_prompt_is_business_only` asserts the manager prompt does **not** contain the substrings `design.md`, `builders.json`, `role`, `architect`, `builder`, `builders`. All new manager-prompt text must avoid those words.
- `state::set_status(path, id, status, note)` only overwrites `notes` when `note` is non-empty.
- The orchestrator-emitted job statuses are exactly: `running` (dispatch), `done`, `failed`, `merged`, `bounced`, `approved`, `rejected`.

---

### Task 1: TUI — treat customer `approved`/`rejected` as terminal statuses

The customer job is reported with `reporter.status(&cid, "approved", ...)` / `"rejected"` (src/orchestrator.rs:558,562). `tui.rs` freezes the working timer only for `merged|done|failed|bounced` (src/tui.rs:116-117) and `status_glyph` has no arm for the customer statuses, so the job renders `?` and its timer keeps ticking.

**Files:**
- Modify: `src/tui.rs:114-123` (apply), `src/tui.rs:417-427` (status_glyph)
- Test: `tests/tui_viewmodel_test.rs`, `tests/tui_render_test.rs`

- [ ] **Step 1: Write the failing viewmodel test**

Append to `tests/tui_viewmodel_test.rs`:

```rust
#[test]
fn customer_review_statuses_freeze_timer() {
    for status in ["approved", "rejected"] {
        let mut s = AppState::new("g".into());
        s.apply(Event::JobDispatched {
            id: "task-1-customer".into(),
            label: "customer review".into(),
            tool: "claude".into(),
            model: "sonnet".into(),
            log_path: None,
        });
        s.apply(Event::JobStatus {
            id: "task-1-customer".into(),
            status: status.into(),
        });
        let j = s.jobs.iter().find(|j| j.id == "task-1-customer").unwrap();
        assert!(j.frozen.is_some(), "{status} must freeze the working timer");
        assert_eq!(j.status, status);
    }
}
```

- [ ] **Step 2: Write the failing render test**

Append to `tests/tui_render_test.rs`:

```rust
#[test]
fn customer_review_statuses_render_glyphs_not_question_mark() {
    let mut s = started("goal");
    s.apply(Event::JobDispatched {
        id: "task-1-customer".into(),
        label: "customer review".into(),
        tool: "claude".into(),
        model: "sonnet".into(),
        log_path: None,
    });
    s.apply(Event::JobStatus {
        id: "task-1-customer".into(),
        status: "approved".into(),
    });
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(find(&term, "✓ customer review").is_some(), "approved renders ✓");
    assert!(find(&term, "? customer review").is_none(), "no ? for approved");

    s.apply(Event::JobStatus {
        id: "task-1-customer".into(),
        status: "rejected".into(),
    });
    term.draw(|f| tui::render(f, &s)).unwrap();
    assert!(find(&term, "✗ customer review").is_some(), "rejected renders ✗");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --test tui_viewmodel_test customer_review -- --nocapture && cargo test --test tui_render_test customer_review`
Expected: FAIL — `frozen` is `None` for "approved"; render shows `? customer review`.

- [ ] **Step 4: Implement**

In `src/tui.rs`, replace the `Event::JobStatus` arm body:

```rust
            Event::JobStatus { id, status } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    if is_terminal_status(&status) && j.frozen.is_none() {
                        j.frozen = j.started.map(|s| s.elapsed());
                    }
                    j.status = status;
                }
            }
```

Add the helper next to `status_glyph` (bottom of the file, before `status_glyph`):

```rust
/// Job statuses that end the working timer (the job will not run further).
/// Must cover every terminal status the orchestrator reports: merged/done/failed/
/// bounced for build jobs, approved/rejected for customer reviews.
fn is_terminal_status(status: &str) -> bool {
    matches!(
        status,
        "merged" | "done" | "failed" | "bounced" | "approved" | "rejected"
    )
}
```

Replace `status_glyph`:

```rust
fn status_glyph(status: &str) -> &'static str {
    match status {
        "running" => "●",
        "merged" => "✓",
        "done" => "✓",
        "approved" => "✓",
        "failed" => "✗",
        "rejected" => "✗",
        "bounced" => "↺",
        "queued" => "·",
        _ => "?",
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --test tui_viewmodel_test && cargo test --test tui_render_test`
Expected: PASS (all tests in both files).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/tui.rs tests/tui_viewmodel_test.rs tests/tui_render_test.rs
git commit -m "fix(tui): freeze timer and render glyphs for customer approved/rejected"
```

---

### Task 2: `limits.rs` — detect provider usage limits and compute waits

Pure functions only; no I/O besides a log-tail reader. The claude CLI reports usage limits as e.g. `Claude AI usage limit reached|1750118400` (epoch suffix); codex says `You've hit your usage limit`; the API error type is `rate_limit_error`. Patterns are deliberately narrow so a failing builder whose log merely *discusses* rate limiting is not misdetected.

**Files:**
- Create: `src/limits.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Create `src/limits.rs` with failing tests included**

```rust
use std::path::Path;
use std::time::Duration;

/// A provider usage/rate limit detected in agent output.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageLimit {
    /// Unix epoch (seconds) when the limit resets, when the output included one.
    pub reset_epoch: Option<i64>,
}

/// Scan agent output for a usage/rate-limit message. Patterns are kept narrow
/// (provider phrasings, not the words "rate limit" alone) so ordinary failures
/// in agents that happen to discuss rate limiting are not misdetected.
/// Known shapes:
///   claude: "Claude AI usage limit reached|1750118400"
///   claude: "You've reached your usage limit"
///   codex:  "You've hit your usage limit" / "Rate limit reached"
///   API:    "rate_limit_error"
pub fn detect_usage_limit(text: &str) -> Option<UsageLimit> {
    let lower = text.to_lowercase();
    const PATTERNS: [&str; 5] = [
        "usage limit reached",
        "reached your usage limit",
        "hit your usage limit",
        "rate limit reached",
        "rate_limit_error",
    ];
    if !PATTERNS.iter().any(|p| lower.contains(p)) {
        return None;
    }
    // claude appends the reset epoch as "...usage limit reached|<epoch>".
    const EPOCH_MARK: &str = "usage limit reached|";
    let reset_epoch = lower.find(EPOCH_MARK).and_then(|i| {
        let digits: String = lower[i + EPOCH_MARK.len()..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse::<i64>().ok().filter(|e| *e > 1_000_000_000)
    });
    Some(UsageLimit { reset_epoch })
}

/// How long to wait before auto-continuing: until the reset epoch plus a slack
/// margin when known, else a fallback window. Capped at 6h. The slack and
/// fallback are env-overridable (AGENTLOOP_LIMIT_SLACK_SECS,
/// AGENTLOOP_LIMIT_FALLBACK_SECS) so tests can shrink them.
pub fn wait_duration(limit: &UsageLimit, now_epoch: i64) -> Duration {
    let slack = env_secs("AGENTLOOP_LIMIT_SLACK_SECS", 60);
    let fallback = env_secs("AGENTLOOP_LIMIT_FALLBACK_SECS", 900);
    let secs = match limit.reset_epoch {
        Some(reset) => (reset - now_epoch).max(0) as u64 + slack,
        None => fallback,
    };
    Duration::from_secs(secs.min(6 * 3600))
}

fn env_secs(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Last `max_bytes` of `path` as lossy UTF-8; "" when missing/unreadable.
pub fn log_tail(path: &Path, max_bytes: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_claude_limit_with_reset_epoch() {
        let l = detect_usage_limit("blah\nClaude AI usage limit reached|1750118400\n").unwrap();
        assert_eq!(l.reset_epoch, Some(1750118400));
    }

    #[test]
    fn detects_limits_without_epoch() {
        for text in [
            "You've reached your usage limit",
            "you've hit your usage limit, try again later",
            "Rate limit reached for requests",
            r#"{"type":"error","error":{"type":"rate_limit_error"}}"#,
        ] {
            let l = detect_usage_limit(text).unwrap_or_else(|| panic!("missed: {text}"));
            assert_eq!(l.reset_epoch, None);
        }
    }

    #[test]
    fn ordinary_failures_are_not_limits() {
        for text in [
            "compile error: expected `;`",
            "tests failed: 3 passed; 1 failed",
            "we should rate limit the login endpoint",
            "",
        ] {
            assert!(detect_usage_limit(text).is_none(), "false positive: {text}");
        }
    }

    #[test]
    fn wait_until_reset_plus_slack_capped_at_six_hours() {
        let l = UsageLimit {
            reset_epoch: Some(1_700_000_300),
        };
        assert_eq!(
            wait_duration(&l, 1_700_000_000),
            Duration::from_secs(300 + 60)
        );
        // Past reset -> just the slack.
        assert_eq!(wait_duration(&l, 1_800_000_000), Duration::from_secs(60));
        // No epoch -> fallback window.
        assert_eq!(
            wait_duration(&UsageLimit { reset_epoch: None }, 0),
            Duration::from_secs(900)
        );
        // Far-future reset is capped.
        let far = UsageLimit {
            reset_epoch: Some(2_000_000_000),
        };
        assert_eq!(wait_duration(&far, 1_700_000_000), Duration::from_secs(6 * 3600));
    }

    #[test]
    fn log_tail_reads_last_bytes_and_tolerates_missing_files() {
        let dir = std::env::temp_dir().join(format!(
            "limits-tail-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.log");
        assert_eq!(log_tail(&p, 16), "");
        std::fs::write(&p, "0123456789ABCDEF-tail").unwrap();
        assert_eq!(log_tail(&p, 5), "-tail");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Register the module**

In `src/lib.rs`, insert `pub mod limits;` between `pub mod inbox;` and `pub mod manager;` (the list is alphabetical):

```rust
pub mod inbox;
pub mod limits;
pub mod manager;
```

- [ ] **Step 3: Run the unit tests**

Run: `cargo test --lib limits`
Expected: PASS (5 tests). Note: these tests rely on `AGENTLOOP_LIMIT_*` env vars being unset in the lib test binary — do not add env mutation to lib tests.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add src/limits.rs src/lib.rs
git commit -m "feat(limits): detect provider usage limits and compute auto-continue waits"
```

---

### Task 3: `spawn::agent_run` — wait out usage limits and auto-continue

On a non-zero exit, read the log tail; if it shows a usage limit, log a ⏳ note, sleep until the reset (interruptible by quit), and re-run the same agent command. Also switch the job log to append mode so retries (and the existing manager/customer re-prompts, which reuse one log path) accumulate instead of truncating history.

**Files:**
- Modify: `src/spawn.rs`
- Test: `tests/spawn_test.rs`

- [ ] **Step 1: Write the failing integration test**

Append to `tests/spawn_test.rs` (it already has `cfg()` with a claude-tool `manager` role, and `ENV_LOCK`):

```rust
#[tokio::test]
async fn agent_run_waits_out_usage_limit_and_auto_continues() {
    let _guard = ENV_LOCK.lock().await;
    let dir = std::env::temp_dir().join(format!(
        "limitws-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mark = dir.join("first-done");
    // First call: print a usage-limit message (reset already in the past) and fail.
    // Second call: succeed.
    let stub = format!(
        "#!/bin/bash\nif [ ! -f \"{mark}\" ]; then\n  touch \"{mark}\"\n  echo \"Claude AI usage limit reached|1700000000\"\n  exit 1\nfi\necho \"FAKE_OK\"\nexit 0\n",
        mark = mark.display()
    );
    let stub_path = dir.join("stub.sh");
    std::fs::write(&stub_path, stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", &stub_path);
    std::env::set_var("AGENTLOOP_LIMIT_SLACK_SECS", "0");

    let log = dir.join("agent.log");
    let code = spawn::agent_run(
        &cfg(),
        "manager",
        "HELLO",
        &dir,
        &log,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("AGENTLOOP_LIMIT_SLACK_SECS");

    assert_eq!(code, 0, "second attempt after the limit wait succeeds");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains("usage limit reached"),
        "first attempt's limit message is kept in the log: {text}"
    );
    assert!(
        text.contains("auto-continuing"),
        "the wait note is logged: {text}"
    );
    assert!(text.contains("FAKE_OK"), "second attempt's output is appended");
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test spawn_test agent_run_waits_out_usage_limit -- --nocapture`
Expected: FAIL — `code` is 1 (no retry happens yet).

- [ ] **Step 3: Implement in `src/spawn.rs`**

3a. Extend the imports at the top:

```rust
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
```

3b. Below `ACTIVE_PGIDS`, add the shutdown flag and cap:

```rust
/// Set when the process is quitting (signal handler / TUI exit). Usage-limit waits
/// poll this so a quit interrupts an hours-long sleep instead of hanging shutdown.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// True once a shutdown (quit / signal) has been requested.
pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Cap on how many usage-limit waits a single agent run will sit through.
const MAX_LIMIT_WAITS: u32 = 48;
```

3c. In `kill_all_agents`, set the flag first (new first line of the function body):

```rust
pub fn kill_all_agents() {
    SHUTDOWN.store(true, Ordering::SeqCst);
    use nix::sys::signal::{killpg, Signal};
    ...
```

3d. In `run_with_timeout`, open the log in append mode (replace the `File::create` line):

```rust
    // Append so usage-limit retries and re-prompts that reuse one log path keep
    // the earlier attempt's output (the TUI tails this file live).
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("create log {}", log.display()))?;
```

3e. Add two helpers above `agent_run`:

```rust
/// Append one line to the job log (best-effort; the log is the TUI's live view).
fn append_log_line(log: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        let _ = writeln!(f, "{line}");
    }
}

/// Sleep in short slices so a quit (kill_all_agents) interrupts the wait.
/// Returns false when shutdown was requested before the wait completed.
async fn sleep_interruptible(total: Duration) -> bool {
    let mut left = total;
    while !left.is_zero() {
        if shutdown_requested() {
            return false;
        }
        let step = left.min(Duration::from_secs(5));
        tokio::time::sleep(step).await;
        left = left.saturating_sub(step);
    }
    !shutdown_requested()
}
```

3f. Replace `agent_run` entirely:

```rust
/// Resolve a role and run the matching CLI (or fake) in cwd, capped by timeout.
/// When the agent dies on a provider usage/rate limit, wait until the limit resets
/// (parsed from the output when present, a fallback window otherwise) and re-run
/// automatically. The wait is interruptible by shutdown and capped per run.
pub async fn agent_run(
    cfg: &Config,
    role: &str,
    prompt: &str,
    cwd: &Path,
    log: &Path,
    t: Duration,
) -> Result<i32> {
    let mut argv = build_argv(cfg, role, prompt)?;
    // claude emits stream-json (see build_argv) which run_with_timeout formats live.
    let rrole = cfg.resolve_role(role).context("no resolvable role")?;
    let stream_claude = cfg.role_field(&rrole, "tool").as_deref() == Some("claude");
    if std::env::var("FAKE_AGENT").as_deref() == Ok("1") {
        let bin =
            std::env::var("FAKE_AGENT_BIN").context("FAKE_AGENT=1 but FAKE_AGENT_BIN unset")?;
        argv.insert(0, bin);
    }

    let mut waits = 0u32;
    loop {
        let code = run_with_timeout(&argv, cwd, log, t, stream_claude).await?;
        if code == 0 {
            return Ok(code);
        }
        let tail = crate::limits::log_tail(log, 16 * 1024);
        let Some(limit) = crate::limits::detect_usage_limit(&tail) else {
            return Ok(code);
        };
        waits += 1;
        if waits > MAX_LIMIT_WAITS || shutdown_requested() {
            return Ok(code);
        }
        let wait = crate::limits::wait_duration(&limit, chrono::Local::now().timestamp());
        let until = (chrono::Local::now() + chrono::Duration::seconds(wait.as_secs() as i64))
            .format("%H:%M:%S");
        let note = format!(
            "⏳ usage limit reached — waiting {}s (until ~{until}), then auto-continuing (wait {waits}/{MAX_LIMIT_WAITS})",
            wait.as_secs()
        );
        eprintln!("{note}");
        append_log_line(log, &note);
        if !sleep_interruptible(wait).await {
            return Ok(code); // shutting down: do not respawn agents
        }
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --test spawn_test`
Expected: PASS (all spawn tests, including the new one — the wait is 0s because the reset epoch is in the past and slack is overridden to 0).

- [ ] **Step 5: Run the full suite to catch append-mode fallout**

Run: `cargo test`
Expected: PASS. (Logs were previously truncated per `run_with_timeout` call; each orchestrator dispatch writes to a unique `iter-N/<id>.log`, so append only changes within-call retries.)

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/spawn.rs tests/spawn_test.rs
git commit -m "feat(spawn): wait out provider usage limits and auto-continue agent runs"
```

---

### Task 4: Orchestrator — auto-answer questions instead of parking items

A builder that reports `needs_input` currently becomes `blocked` and waits for a human (src/orchestrator.rs:441-467). Replace this with an immediate canned "you decide" answer: persist the Q&A (so the existing `inbox::prior_qa_block` feeds it into the re-dispatched builder prompt), consume the question, flip the builder back to `ready`. Also sweep any question files left by an interrupted older run at the start of each iteration. Re-dispatch still increments `attempts`, so an agent that asks forever still hits `max_attempts` → redesign (no infinite loop).

**Files:**
- Modify: `src/orchestrator.rs`
- Test: `tests/loop_needs_input_test.rs` (rewrite)

- [ ] **Step 1: Rewrite the test file to specify the new behavior**

Replace the entire contents of `tests/loop_needs_input_test.rs` with:

```rust
mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::{inbox, orchestrator, state, task_state};
use std::sync::Arc;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cfg() -> Config {
    serde_json::from_str(r#"{
  "caps": { "max_iterations": 6, "max_parallel": 1, "item_timeout_sec": 30, "total_budget_sec": 300, "max_attempts": 5 },
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

fn set_env(ws: &std::path::Path) {
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", ws);
}

fn clear_env() {
    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
}

#[tokio::test]
async fn question_is_auto_answered_and_item_completes() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_asking_stub();
    set_env(&ws);
    let bk = ws.join(".agentloop/state/backlog.json");
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    // Iteration 1: builder asks -> question is auto-answered, item flips back to
    // ready (not blocked), nothing waits on a human.
    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();
    assert_eq!(merged, 0);
    let builders = task_state::read_builders(&ws, "task-1").unwrap();
    assert_eq!(
        task_state::item(&builders, "task-1-b1").unwrap()["status"],
        "ready",
        "asking item is re-queued, not parked"
    );
    let a = inbox::read_answer(&ws, "task-1-b1").unwrap();
    assert_eq!(a.question, "make the file?");
    assert_eq!(a.answer, orchestrator::AUTO_ANSWER);
    assert!(
        !ws.join(".agentloop/questions/task-1-b1.json").exists(),
        "question file is consumed"
    );

    // Iteration 2: the stub sees the answer file and completes; customer approves.
    let merged2 = orchestrator::iterate(&cfg(), &ws, 2, &rep).await.unwrap();
    assert_eq!(merged2, 1);
    assert_eq!(
        std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(),
        "made"
    );
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "task-1").unwrap()["status"], "done");

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn stale_question_from_prior_run_is_swept() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_asking_stub();
    // Simulate an interrupted earlier run: task in progress, builder parked
    // blocked on a question that nobody answered.
    let st = ws.join(".agentloop/state");
    std::fs::write(
        st.join("backlog.json"),
        r#"{"items":[{"id":"task-1","title":"f","desc":"d","deps":[],"status":"in_progress","attempts":0,"acceptance":"file exists"}]}"#,
    )
    .unwrap();
    let tdir = st.join("tasks/task-1");
    std::fs::create_dir_all(&tdir).unwrap();
    std::fs::write(tdir.join("design.md"), "Make the file.").unwrap();
    std::fs::write(
        tdir.join("builders.json"),
        r#"{"items":[{"id":"task-1-b1","title":"make file","desc":"write made.txt","deps":[],"status":"blocked","attempts":1,"acceptance":"made.txt exists"}]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/questions")).unwrap();
    std::fs::write(
        ws.join(".agentloop/questions/task-1-b1.json"),
        r#"{"question":"make the file?","context":"need confirm"}"#,
    )
    .unwrap();
    set_env(&ws);
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    // One iteration: the sweep auto-answers, the builder re-dispatches (stub sees
    // the answer file), the work merges and the customer approves.
    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();
    assert_eq!(merged, 1, "swept question lets the parked builder finish");
    let a = inbox::read_answer(&ws, "task-1-b1").unwrap();
    assert_eq!(a.answer, orchestrator::AUTO_ANSWER);
    let bk = ws.join(".agentloop/state/backlog.json");
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "task-1").unwrap()["status"], "done");

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn headless_run_auto_continues_past_questions() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_asking_stub();
    set_env(&ws);
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    let rc = orchestrator::run(&cfg(), &ws, rep).await.unwrap();
    assert_eq!(rc, 0, "headless run no longer halts on builder questions");
    assert_eq!(
        std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(),
        "made"
    );

    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test loop_needs_input_test`
Expected: FAIL to compile (`orchestrator::AUTO_ANSWER` does not exist) — that is the expected red state.

- [ ] **Step 3: Implement in `src/orchestrator.rs`**

3a. Add the constant and helpers near the top (after the `NO_TIMEOUT` const):

```rust
/// Canned reply for any question an agent raises: the loop is autonomous, so the
/// "user" always delegates the decision back to the agent.
pub const AUTO_ANSWER: &str = "Decide the best option for me — you decide. Pick whatever best serves the goal and the acceptance criteria, record your decision in the result summary, and continue.";

/// Auto-answer a raised question with [`AUTO_ANSWER`] and re-queue the asking item.
pub fn auto_answer(ws: &Path, item_id: &str) -> Result<()> {
    apply_answer(ws, item_id, AUTO_ANSWER)
}

/// Auto-answer every outstanding question file (e.g. left by an interrupted older
/// run) so the asking items re-enter the dispatchable set this round.
fn auto_answer_pending(ws: &Path) {
    let Ok(entries) = std::fs::read_dir(ws.join(".agentloop/questions")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if let Err(e) = auto_answer(ws, id) {
            eprintln!("auto-answer failed for {id}: {e:#}");
        }
    }
}
```

(`apply_answer` already does exactly the right persistence: record answer, consume question, flip the builder — or business item — to `ready`. It is removed/inlined in Task 5.)

3b. In `iterate`, call the sweep right after the directories are created (before the manager dispatch):

```rust
    std::fs::create_dir_all(&ldir)?;
    std::fs::create_dir_all(ws.join(".agentloop/results"))?;
    auto_answer_pending(ws);
```

3c. Replace the whole `if status == "needs_input" { ... }` block in the integration loop with:

```rust
        if status == "needs_input" {
            // Autonomous mode: never park the item on a human. Persist the canned
            // "you decide" answer and re-dispatch with the prior Q&A appended to
            // the builder prompt. Re-dispatch consumes an attempt, so a builder
            // that asks forever still hits max_attempts -> redesign.
            if crate::inbox::has_question(ws, id) {
                if let Err(e) = auto_answer(ws, id) {
                    eprintln!("auto-answer failed for {id}: {e:#}");
                    task_state::set_builder_status(ws, task_id, id, "ready", "auto-answer failed")?;
                }
            } else {
                // Malformed/missing question file: treat as a normal non-done bounce.
                task_state::set_builder_status(
                    ws,
                    task_id,
                    id,
                    "ready",
                    "needs_input without a question file",
                )?;
            }
            reporter.status(id, "bounced", "", "");
            worktree::remove(ws, &ws.join(format!(".agentloop/worktrees/{id}")), &branch);
            let _ = std::fs::remove_file(&rfile);
            continue;
        }
```

Note this removes the `reporter.question(...)` call — the inbox is never populated again.

- [ ] **Step 4: Run the tests**

Run: `cargo test --test loop_needs_input_test`
Expected: PASS (3 tests). The headless test passes because `user_blocked_business_count` is now always 0 (questions are consumed before the check).

- [ ] **Step 5: Run the full suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/orchestrator.rs tests/loop_needs_input_test.rs
git commit -m "feat(orchestrator): auto-answer agent questions instead of parking on the user"
```

---

### Task 5: Remove the question/answer plumbing (events, orchestrator halts, state counters)

Nothing raises questions to the UI anymore; delete the dead paths so the code matches the behavior.

**Files:**
- Modify: `src/events.rs`, `src/orchestrator.rs`, `src/state.rs`
- Test: `tests/state_test.rs` (remove one test)

- [ ] **Step 1: Shrink `src/events.rs`**

Remove from the `Reporter` trait:

```rust
    /// An agent raised a question for the user. Default: no-op.
    fn question(&self, _item_id: &str, _label: &str, _text: &str, _context: &str) {}
```

Remove the `question` impls from `EventLineReporter` and `ChannelReporter`.

Remove the `QuestionRaised` variant from `Event`:

```rust
    QuestionRaised {
        item_id: String,
        label: String,
        text: String,
        context: String,
    },
```

Remove the `AnswerQuestion` variant from `Command`:

```rust
    AnswerQuestion { item_id: String, text: String },
```

- [ ] **Step 2: Shrink `src/orchestrator.rs`**

2a. Inline `apply_answer` into `auto_answer` and delete `apply_answer`. The final `auto_answer`:

```rust
/// Auto-answer a raised question with [`AUTO_ANSWER`]: persist the Q&A, consume the
/// question file, and flip the asking item ready so it is re-dispatched with the
/// prior Q&A appended to its prompt.
pub fn auto_answer(ws: &Path, item_id: &str) -> Result<()> {
    let bk = ws.join(".agentloop/state/backlog.json");
    if let Some(task_id) = builder_owner(ws, item_id)? {
        let question = match crate::inbox::read_question(ws, item_id) {
            Ok(q) => q.question,
            Err(_) => builder_item(ws, &task_id, item_id)?
                .and_then(|i| i["notes"].as_str().map(String::from))
                .unwrap_or_default(),
        };
        crate::inbox::record_answer(ws, item_id, &question, AUTO_ANSWER)?;
        let _ = crate::inbox::consume_question(ws, item_id);
        task_state::set_builder_status(
            ws,
            &task_id,
            item_id,
            "ready",
            "auto-answered; re-dispatching",
        )?;
        return Ok(());
    }

    let question = match crate::inbox::read_question(ws, item_id) {
        Ok(q) => q.question,
        Err(_) => {
            let v = state::read(&bk)?;
            state::item(&v, item_id)
                .and_then(|i| i["notes"].as_str().map(String::from))
                .unwrap_or_default()
        }
    };
    crate::inbox::record_answer(ws, item_id, &question, AUTO_ANSWER)?;
    let _ = crate::inbox::consume_question(ws, item_id);
    state::set_status(&bk, item_id, "ready", "auto-answered; re-dispatching")?;
    Ok(())
}
```

2b. Delete `task_blocked_on_builder_question` and `user_blocked_business_count` (both whole functions).

2c. In `run()`, delete the user-blocked halt:

```rust
        // Only a genuine user-question block (not a manager dependency-block, which
        // ready_items now dispatches autonomously) should halt a headless run.
        let user_blocked = user_blocked_business_count(&bk, ws)?;
        if open > 0 && open == user_blocked {
            eprintln!("STOP: blocked on user input (headless)");
            return Ok(1);
        }
```

2d. In `run_interactive()`:
- Update the doc comment: `/// Interactive driver with a standby state machine. DONE/cap/stall transitions to`
  `/// standby (idle, awaiting a command) instead of exiting. AddTask re-engages with a`
  `/// fresh budget window; Quit exits. Tasks can also be added mid-run.`
- In the pre-run wait loop, replace the stray-command arm:

```rust
            // Stray add-task before the run starts: ignore.
            Some(Command::AddTask { .. }) => {}
```

- In the working-phase drain loop, the match becomes:

```rust
                match cmd {
                    Command::Quit => return Ok(0),
                    Command::AddTask { request } => {
                        let _ = crate::requests::append(ws, &request);
                    }
                    Command::StartRun { .. } => {}
                }
```

- Delete `let user_blocked = user_blocked_business_count(&bk, ws)?;` and the whole `if open > 0 && open == user_blocked { ... continue 'working; }` block in the working phase.
- In the standby phase, the match becomes:

```rust
            match crx.recv().await {
                None | Some(Command::Quit) => return Ok(0),
                Some(Command::AddTask { request }) => {
                    let _ = crate::requests::append(ws, &request);
                }
                Some(Command::StartRun { .. }) => {}
            }
```

- [ ] **Step 3: Shrink `src/state.rs`**

Delete `blocked_count` and `user_blocked_count` (both whole functions; they were only used by the removed halt and one test).

- [ ] **Step 4: Update `tests/state_test.rs`**

Delete the test `user_blocked_counts_only_question_blocks` (and any now-unused helpers it alone used).

- [ ] **Step 5: Build and fix remaining references**

Run: `cargo build 2>&1 | head -50`
Expected: errors only in `src/tui.rs` (it still matches on `Event::QuestionRaised` and emits `Command::AnswerQuestion`) — those are fixed in Task 6, so **do this task and Task 6 in one commit if executing strictly task-by-task is impossible**. If you want a green build at this boundary, proceed directly to Task 6 before committing.

- [ ] **Step 6: Continue to Task 6 (same commit)**

---

### Task 6: TUI — remove the Inbox pane (jobs-only main view)

**Files:**
- Modify: `src/tui.rs`
- Test: `tests/tui_viewmodel_test.rs`, `tests/tui_render_test.rs`

- [ ] **Step 1: Update the tests first**

In `tests/tui_viewmodel_test.rs`:

1a. Replace `applies_events_to_view_model` with:

```rust
#[test]
fn applies_events_to_view_model() {
    let mut s = AppState::new("build a todo app".into());
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: None,
    });
    assert_eq!(s.jobs.len(), 1);

    s.apply(Event::JobStatus {
        id: "it-1".into(),
        status: "merged".into(),
    });
    assert_eq!(
        s.jobs.iter().find(|j| j.id == "it-1").unwrap().status,
        "merged"
    );
}
```

1b. Delete `typing_then_enter_answers_selected_question` and `target_label_tracks_focus_and_inbox`.

1c. Rename `typing_then_enter_adds_task_when_no_question` to `typing_then_enter_adds_task` (body unchanged).

1d. Replace `tab_switches_focus_and_empty_enter_opens_job_detail` with:

```rust
#[test]
fn empty_enter_opens_job_detail_and_esc_returns() {
    use std::path::PathBuf;
    let mut s = start("g");
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: Some(PathBuf::from("/tmp/x.log")),
    });
    // Empty input + Enter opens the detail view for the selected job.
    assert!(s
        .on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .is_none());
    assert!(s.in_job_detail());
    // Esc returns to the list.
    assert!(s
        .on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .is_none());
    assert!(!s.in_job_detail());
}
```

1e. In `q_quits_from_job_detail_when_input_empty`, delete the line `s.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)); // focus Jobs`.

In `tests/tui_render_test.rs`:

1f. Replace `jobs_render_above_inbox_full_width` with:

```rust
#[test]
fn jobs_pane_renders_without_inbox() {
    let mut s = started("goal");
    s.apply(Event::JobDispatched {
        id: "it-1".into(),
        label: "scaffold".into(),
        tool: "codex".into(),
        model: "gpt-5".into(),
        log_path: None,
    });

    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| tui::render(f, &s)).unwrap();

    assert!(find(&term, "Jobs").is_some(), "Jobs pane rendered");
    assert!(find(&term, "Inbox").is_none(), "Inbox pane removed");
}
```

1g. Delete `inbox_pane_scrolls_to_keep_selection_visible`.

1h. In `jobs_pane_scrolls_to_keep_selection_visible`, delete the line `s.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));` and update its comment to `// Move the selection to the last job.`

- [ ] **Step 2: Implement in `src/tui.rs`**

2a. Delete the `Pending` struct and the `Focus` enum.

2b. `AppState` fields and `new` become:

```rust
pub struct AppState {
    pub goal: String,
    pub jobs: Vec<Job>,
    pub iter: u32,
    pub gate: String,
    pub open: i64,
    pub standby: bool,
    input: String,
    view: View,
    goal_focus_continue: bool,
    selected_job: usize,
    log_scroll: u16,
    started: std::time::Instant,
}

impl AppState {
    pub fn new(goal: String) -> Self {
        Self {
            goal: goal.clone(),
            jobs: vec![],
            iter: 0,
            gate: "init".into(),
            open: 0,
            standby: false,
            input: goal,
            view: View::GoalEntry,
            goal_focus_continue: false,
            selected_job: 0,
            log_scroll: 0,
            started: std::time::Instant::now(),
        }
    }
```

(`inbox`, `selected`, and `focus` are gone.)

2c. In `apply`, delete the `Event::QuestionRaised { .. } => { ... }` arm.

2d. Replace `on_key_list`:

```rust
    fn on_key_list(&mut self, k: KeyEvent) -> Option<Command> {
        if Self::is_newline(&k) {
            self.input.push('\n');
            return None;
        }
        match k.code {
            KeyCode::Up => {
                if self.selected_job > 0 {
                    self.selected_job -= 1;
                }
                None
            }
            KeyCode::Down => {
                if self.selected_job + 1 < self.jobs.len() {
                    self.selected_job += 1;
                }
                None
            }
            KeyCode::Enter => {
                // Non-empty input submits a task; empty input opens the selected job.
                if self.input.trim().is_empty() {
                    if self.selected_job < self.jobs.len() {
                        self.view = View::JobDetail;
                        self.log_scroll = 0;
                    }
                    None
                } else {
                    self.submit()
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                None
            }
            KeyCode::Esc => {
                self.input.clear();
                None
            }
            KeyCode::Char('q') if self.input.trim().is_empty() => Some(Command::Quit),
            KeyCode::Char(c) => {
                self.input.push(c);
                None
            }
            _ => None,
        }
    }
```

2e. Replace `submit`:

```rust
    /// Submit the current input as a new task for the manager. Clears the input.
    fn submit(&mut self) -> Option<Command> {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.input.clear();
        Some(Command::AddTask { request: text })
    }
```

2f. Delete `focus_is_jobs` and `input_target_label` (public methods).

2g. In `render`, replace the status-bar text (drop the `❓` segment):

```rust
    let status_text = if s.standby {
        format!(
            " ✓ DONE · standby  │  {}  │  iter {}  │  gate: {}  │  open: {}  │  ⏱ {}",
            s.goal, s.iter, s.gate, s.open, total
        )
    } else {
        format!(
            " {}  │  iter {}  │  gate: {}  │  open: {}  │  ⏱ {}",
            s.goal, s.iter, s.gate, s.open, total
        )
    };
```

2h. Replace the whole main-area block (jobs + inbox split) with a jobs-only pane:

```rust
    // --- Main area: the jobs list, or the job-detail view ---
    if s.in_job_detail() {
        render_job_detail(f, s, chunks[1]);
    } else {
        let job_items: Vec<ListItem> = s
            .jobs
            .iter()
            .map(|j| {
                let glyph = status_glyph(&j.status);
                let dur = j.elapsed().map(fmt_elapsed).unwrap_or_default();
                let line = format!(" {} {} [{}/{}]  {}", glyph, j.label, j.tool, j.model, dur);
                ListItem::new(Line::from(line))
            })
            .collect();
        let jobs_list = List::new(job_items)
            .block(
                Block::default()
                    .title(" Jobs ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );
        let mut jobs_state = ListState::default();
        if !s.jobs.is_empty() {
            jobs_state.select(Some(s.selected_job.min(s.jobs.len() - 1)));
        }
        f.render_stateful_widget(jobs_list, chunks[1], &mut jobs_state);
    }
```

2i. The input-bar title becomes a constant string:

```rust
    let input = Paragraph::new(s.input_buffer())
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" Add task ")
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().fg(Color::White));
```

2j. Replace the hint lines (drop `[tab] pane`):

```rust
    let hint = if s.standby {
        " ✓ standby · [enter] submit  [shift+enter] newline  [↑↓] jobs  [esc] clear  [q] quit"
    } else {
        " [enter] submit  [shift+enter] newline  [↑↓] jobs  [esc] clear  [q] quit"
    };
```

2k. In `on_key_goal_entry`, keep the `KeyCode::Tab` arm (it toggles the Continue button) — do not delete that one.

- [ ] **Step 3: Build and test**

Run: `cargo build && cargo test`
Expected: PASS — everything compiles, all tests green (Tasks 5+6 together restore the build).

- [ ] **Step 4: Commit (covers Tasks 5 and 6)**

```bash
cargo fmt
git add src/events.rs src/orchestrator.rs src/state.rs src/tui.rs tests/state_test.rs tests/tui_viewmodel_test.rs tests/tui_render_test.rs
git commit -m "feat(tui)!: remove the question Inbox; the loop auto-answers questions"
```

---

### Task 7: Builder prompt — decide autonomously

The question mechanism still exists as a safety net, but builders should be told upfront that decisions are theirs (asking costs a full round-trip and an attempt).

**Files:**
- Modify: `src/worker.rs:52-56`
- Test: `tests/roles_prompt_test.rs`

- [ ] **Step 1: Write the failing test**

In `tests/roles_prompt_test.rs`, find the existing builder-prompt test (asserts `p.contains("You are a BUILDER")`, around line 96) and add to it:

```rust
    assert!(
        p.contains("Open decisions are yours"),
        "builders are told to decide autonomously"
    );
    assert!(
        p.contains("An automatic reply will tell you to decide for yourself"),
        "builders know questions are auto-answered"
    );
```

Run: `cargo test --test roles_prompt_test`
Expected: FAIL on the new assertions.

- [ ] **Step 2: Implement**

In `src/worker.rs`'s `builder_prompt`, replace the final rule block:

```
- If you are blocked needing a decision that only the user can make, DO NOT guess.
  Write {ws}/.agentloop/questions/{id}.json:
  {{"question":"<your question>","context":"<brief context>"}}
  and write the result file with {{"status":"needs_input","summary":"<what you need>"}} instead,
  then stop. The user will answer and you will be re-dispatched with their answer.{prior}
```

with:

```
- Open decisions are yours: when you hit a product or technical choice, pick the option
  that best serves the business task and its acceptance criteria, note the decision in
  your result summary, and keep going. Nobody reviews questions live.
- Only as a last resort, if you truly cannot proceed, write {ws}/.agentloop/questions/{id}.json:
  {{"question":"<your question>","context":"<brief context>"}}
  and write the result file with {{"status":"needs_input","summary":"<what you need>"}} instead,
  then stop. An automatic reply will tell you to decide for yourself and you will be
  re-dispatched with that Q&A — so prefer deciding now.{prior}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --test roles_prompt_test`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add src/worker.rs tests/roles_prompt_test.rs
git commit -m "feat(worker): instruct builders to decide autonomously"
```

---

### Task 8: `state.rs` — strip unknown deps; report failed-dep stuck items

Two pure backlog helpers: `strip_unknown_deps` deterministically removes deps on ids that do not exist in the backlog (they can never be satisfied — e.g. the leaked `task-9-b1` dep in test-chat-app); `failed_dep_report` describes open items stuck behind `failed` items for the manager prompt (Task 11).

**Files:**
- Modify: `src/state.rs`
- Test: `tests/state_test.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/state_test.rs`:

```rust
#[test]
fn strip_unknown_deps_removes_only_unsatisfiable_deps() {
    let dir = std::env::temp_dir().join(format!(
        "strip-deps-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let bk = dir.join("backlog.json");
    std::fs::write(
        &bk,
        r#"{"items":[
            {"id":"task-1","title":"a","desc":"d","deps":[],"status":"done","attempts":0,"acceptance":"x"},
            {"id":"task-2","title":"b","desc":"d","deps":["task-1","task-ghost"],"status":"ready","attempts":0,"acceptance":"x"},
            {"id":"task-3","title":"c","desc":"d","deps":["task-2"],"status":"ready","attempts":0,"acceptance":"x"}
        ]}"#,
    )
    .unwrap();

    let removed = state::strip_unknown_deps(&bk).unwrap();
    assert_eq!(removed, vec![("task-2".to_string(), "task-ghost".to_string())]);

    let v = state::read(&bk).unwrap();
    assert_eq!(
        state::item(&v, "task-2").unwrap()["deps"],
        serde_json::json!(["task-1"]),
        "only the unknown dep is removed"
    );
    assert!(state::item(&v, "task-2").unwrap()["notes"]
        .as_str()
        .unwrap()
        .contains("unknown"));
    assert_eq!(
        state::item(&v, "task-3").unwrap()["deps"],
        serde_json::json!(["task-2"]),
        "valid deps are untouched"
    );
    // Second pass is a no-op.
    assert!(state::strip_unknown_deps(&bk).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_dep_report_lists_open_items_behind_failed_items() {
    let dir = std::env::temp_dir().join(format!(
        "failed-deps-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let bk = dir.join("backlog.json");
    std::fs::write(
        &bk,
        r#"{"items":[
            {"id":"task-1","title":"a","desc":"d","deps":[],"status":"failed","attempts":3,"acceptance":"x"},
            {"id":"task-2","title":"b","desc":"d","deps":["task-1"],"status":"ready","attempts":0,"acceptance":"x"},
            {"id":"task-3","title":"c","desc":"d","deps":[],"status":"ready","attempts":0,"acceptance":"x"}
        ]}"#,
    )
    .unwrap();

    let report = state::failed_dep_report(&bk).unwrap();
    assert!(report.contains("task-2 depends on failed task-1"), "{report}");
    assert!(!report.contains("task-3"), "healthy items are not reported");
    let _ = std::fs::remove_dir_all(&dir);
}
```

Run: `cargo test --test state_test strip_unknown -- --nocapture; cargo test --test state_test failed_dep`
Expected: FAIL to compile (functions missing).

- [ ] **Step 2: Implement in `src/state.rs`** (append after `item`)

```rust
/// Remove deps that reference ids not present in the backlog at all — they can
/// never be satisfied (the `done` set can never include them), so the item would
/// sit open forever. Only open items (ready/in_progress/blocked) are repaired.
/// Returns the removed (item_id, dep_id) pairs; each repaired item gets a note.
pub fn strip_unknown_deps(path: &Path) -> Result<Vec<(String, String)>> {
    let mut v = read(path)?;
    let empty = vec![];
    let ids: HashSet<String> = v["items"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|i| i["id"].as_str().map(str::to_string))
        .collect();
    let mut removed: Vec<(String, String)> = Vec::new();
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if !matches!(
                it["status"].as_str(),
                Some("ready") | Some("in_progress") | Some("blocked")
            ) {
                continue;
            }
            let Some(id) = it["id"].as_str().map(str::to_string) else {
                continue;
            };
            let Some(deps) = it.get_mut("deps").and_then(|d| d.as_array_mut()) else {
                continue;
            };
            let mut gone: Vec<String> = Vec::new();
            deps.retain(|d| match d.as_str() {
                Some(dep) if !ids.contains(dep) => {
                    gone.push(dep.to_string());
                    false
                }
                _ => true,
            });
            if !gone.is_empty() {
                it["notes"] = json!(format!(
                    "removed deps on unknown ids: {}",
                    gone.join(", ")
                ));
                removed.extend(gone.into_iter().map(|g| (id.clone(), g)));
            }
        }
    }
    if !removed.is_empty() {
        write_atomic(path, &v)?;
    }
    Ok(removed)
}

/// Lines describing open items that depend on `failed` items (they can never run
/// until the manager reshapes them), or "" when there are none. Used to build the
/// manager-prompt repair section.
pub fn failed_dep_report(path: &Path) -> Result<String> {
    let v = read(path)?;
    let empty = vec![];
    let items = v["items"].as_array().unwrap_or(&empty);
    let failed: HashSet<&str> = items
        .iter()
        .filter(|i| i["status"] == "failed")
        .filter_map(|i| i["id"].as_str())
        .collect();
    let mut out = String::new();
    for it in items {
        if !matches!(
            it["status"].as_str(),
            Some("ready") | Some("in_progress") | Some("blocked")
        ) {
            continue;
        }
        let Some(id) = it["id"].as_str() else { continue };
        let bad: Vec<&str> = it
            .get("deps")
            .and_then(|d| d.as_array())
            .map(|deps| {
                deps.iter()
                    .filter_map(|d| d.as_str())
                    .filter(|d| failed.contains(d))
                    .collect()
            })
            .unwrap_or_default();
        if !bad.is_empty() {
            out.push_str(&format!("  - {id} depends on failed {}\n", bad.join(", ")));
        }
    }
    Ok(out)
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --test state_test`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add src/state.rs tests/state_test.rs
git commit -m "feat(state): strip unknown deps and report items stuck on failed deps"
```

---

### Task 9: `orchestrator::repair_backlog` — deterministic liveness repairs each round

Run right after the manager: (1) drop unknown deps; (2) flip `in_progress`-without-a-valid-plan back to `ready` (only `ready` items reach the architect, so these are otherwise zombies); (3) reopen for redesign any valid plan whose remaining builders can never dispatch (deps on `failed`/abandoned builders). Repairs run before the architect pass so repaired items make progress in the **same** iteration.

**Files:**
- Modify: `src/orchestrator.rs` (new fn + wiring in `iterate`; in-module unit tests)

- [ ] **Step 1: Write the failing unit tests**

Append inside the existing `mod tests` in `src/orchestrator.rs` (it already has `tmp_ws` and `setup` — `setup` seeds one `in_progress` task with a valid plan whose single builder is `done`):

```rust
    #[test]
    fn repair_backlog_leaves_healthy_tasks_alone() {
        let ws = tmp_ws("orch-healthy");
        setup(&ws);
        let bk = ws.join(".agentloop/state/backlog.json");

        repair_backlog(&bk, &ws, 3).unwrap();

        let v = state::read(&bk).unwrap();
        assert_eq!(state::item(&v, "task-1").unwrap()["status"], "in_progress");
        assert!(task_state::builders_path(&ws, "task-1").exists());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn repair_backlog_flips_in_progress_without_plan_to_ready() {
        let ws = tmp_ws("orch-noplan");
        setup(&ws);
        std::fs::remove_file(ws.join(".agentloop/state/tasks/task-1/builders.json")).unwrap();
        let bk = ws.join(".agentloop/state/backlog.json");

        repair_backlog(&bk, &ws, 3).unwrap();

        let v = state::read(&bk).unwrap();
        let task = state::item(&v, "task-1").unwrap();
        assert_eq!(task["status"], "ready");
        assert!(task["notes"]
            .as_str()
            .unwrap()
            .contains("re-architecting"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn repair_backlog_reopens_deadlocked_builder_plan() {
        let ws = tmp_ws("orch-deadlock");
        setup(&ws);
        // Remaining ready builder deps on a failed one: never dispatchable.
        std::fs::write(
            ws.join(".agentloop/state/tasks/task-1/builders.json"),
            r#"{"items":[
                {"id":"task-1-b1","title":"t","desc":"d","deps":[],"status":"failed","attempts":3,"acceptance":"a"},
                {"id":"task-1-b2","title":"t","desc":"d","deps":["task-1-b1"],"status":"ready","attempts":0,"acceptance":"a"}
            ]}"#,
        )
        .unwrap();
        let bk = ws.join(".agentloop/state/backlog.json");

        repair_backlog(&bk, &ws, 3).unwrap();

        let v = state::read(&bk).unwrap();
        assert_eq!(state::item(&v, "task-1").unwrap()["status"], "ready");
        assert!(
            !task_state::builders_path(&ws, "task-1").exists(),
            "deadlocked plan is invalidated for redesign"
        );
        assert_eq!(
            task_state::read_redesign(&ws, "task-1").0,
            1,
            "deadlock consumes a redesign"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }
```

Run: `cargo test --lib orchestrator::tests::repair`
Expected: FAIL to compile (`repair_backlog` missing).

- [ ] **Step 2: Implement `repair_backlog`** (place after `reopen_unapproved_done_tasks`)

```rust
/// Deterministic liveness repairs, run right after the manager each round: the
/// backlog must never hold open work the orchestrator can never dispatch, or the
/// loop stalls/standbys with open items and nothing to do.
fn repair_backlog(bk: &Path, ws: &Path, max_redesigns: u32) -> Result<()> {
    // 1) Deps on ids missing from the backlog can never be satisfied (e.g. a
    //    manager that leaked task-local sub-item ids) — drop them.
    for (id, dep) in state::strip_unknown_deps(bk)? {
        eprintln!("repair: dropped {id} dep on unknown id {dep}");
    }

    // 2) Only `ready` items reach the architect, so `in_progress` without a valid
    //    local plan would never get planned again (manager rewrite / stale resume)
    //    — flip it back to ready.
    let backlog = state::read(bk)?;
    let empty = vec![];
    for item in backlog["items"].as_array().unwrap_or(&empty) {
        let Some(id) = item["id"].as_str() else { continue };
        if item["status"] == "in_progress" && !task_state::builder_plan_valid(ws, id) {
            state::set_status(
                bk,
                id,
                "ready",
                "in_progress without a valid plan; re-architecting",
            )?;
        }
    }

    // 3) A valid plan whose remaining builders can never dispatch (deps on failed
    //    or abandoned builders) deadlocks its parent — reopen it for redesign.
    for task_id in active_business_ids(bk, ws)? {
        if !task_state::builder_plan_valid(ws, &task_id)
            || task_state::all_builders_done(ws, &task_id)?
            || !task_state::ready_builders(ws, &task_id, 1)?.is_empty()
        {
            continue;
        }
        reopen_parent_for_redesign(
            bk,
            ws,
            &task_id,
            "builder plan deadlocked: no dispatchable builders remain",
            max_redesigns,
        )?;
    }
    Ok(())
}
```

- [ ] **Step 3: Wire it into `iterate`**

Right after `let _ = reopen_unapproved_done_tasks(&bk, ws)?;` (the line following `reporter.status("manager", "done", ...)`), add:

```rust
    repair_backlog(&bk, ws, maxatt)?;
```

(Ordering matters: the Task-4 `auto_answer_pending` sweep already ran at the top of `iterate`, so no builder is still `blocked` on a question when the deadlock check runs.)

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib && cargo test`
Expected: PASS (unit tests + full suite; existing loop tests are unaffected because healthy states are no-ops).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/orchestrator.rs
git commit -m "feat(orchestrator): repair stuck backlogs after every manager round"
```

---

### Task 10: End-to-end repair tests (keep-backlog stub)

Integration proof that each zombie mode from test-chat-app now completes instead of stalling. Needs a stub whose MANAGER does **not** rewrite the backlog, so tests can pre-seed broken states.

**Files:**
- Modify: `tests/common/mod.rs`
- Create: `tests/loop_repair_test.rs`

- [ ] **Step 1: Add the stub helper**

Append to `tests/common/mod.rs`:

```rust
/// Stub whose MANAGER never rewrites backlog.json (it only refreshes verify.sh and
/// master.md), for tests that pre-seed broken backlog states the orchestrator must
/// repair. Architect/builder/customer behave like init_ws_with_stub's.
#[allow(dead_code)]
pub fn init_ws_with_keep_backlog_stub() -> PathBuf {
    let ws = init_ws_with_stub();
    let stub = r##"#!/bin/bash
tool="$1"; shift
ws_state="$WS/.agentloop/state"; res="$WS/.agentloop/results"
prompt="$*"
case "$prompt" in
  *MANAGER*)
    printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$WS/.agentloop/verify.sh"; chmod +x "$WS/.agentloop/verify.sh"
    echo "# updated" > "$ws_state/master.md"
    ;;
  *ARCHITECT*)
    mkdir -p "$ws_state/tasks/task-1"
    echo "Make the file." > "$ws_state/tasks/task-1/design.md"
    echo '{"items":[{"id":"task-1-b1","title":"make file","desc":"write made.txt","deps":[],"status":"ready","attempts":0,"acceptance":"made.txt exists"}]}' > "$ws_state/tasks/task-1/builders.json"
    ;;
  *BUILDER*)
    echo made > "$PWD/made.txt"; git add -A; git commit -qm "worker" 2>/dev/null
    echo '{"status":"done","summary":"made file","files_changed":["made.txt"]}' > "$res/task-1-b1.json"
    ;;
  *"SILLY CUSTOMER"*)
    mkdir -p "$ws_state/tasks/task-1"
    echo '{"status":"approved","summary":"accepted","acceptance_notes":"made.txt exists"}' > "$ws_state/tasks/task-1/customer.json"
    echo '{"status":"approved","summary":"accepted"}' > "$res/task-1-customer.json"
    ;;
esac
exit 0
"##;
    std::fs::write(ws.join("stub.sh"), stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(ws.join("stub.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    ws
}
```

- [ ] **Step 2: Create `tests/loop_repair_test.rs`**

```rust
mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::{orchestrator, state};
use std::sync::Arc;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cfg() -> Config {
    serde_json::from_str(r#"{
  "caps": { "max_iterations": 3, "max_parallel": 1, "item_timeout_sec": 30, "total_budget_sec": 300, "max_attempts": 3 },
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

fn set_env(ws: &std::path::Path) {
    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", ws);
}

fn clear_env() {
    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
}

fn write_backlog(ws: &std::path::Path, json: &str) {
    std::fs::write(ws.join(".agentloop/state/backlog.json"), json).unwrap();
}

/// Zombie mode (a) from test-chat-app: in_progress with no local plan was never
/// re-architected (only ready items reach the architect).
#[tokio::test]
async fn in_progress_task_without_plan_is_rearchitected_and_completes() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_keep_backlog_stub();
    write_backlog(
        &ws,
        r#"{"items":[{"id":"task-1","title":"f","desc":"d","deps":[],"status":"in_progress","attempts":0,"acceptance":"file exists"}]}"#,
    );
    set_env(&ws);
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();

    assert_eq!(merged, 1, "repaired task is architected and built this round");
    let bk = ws.join(".agentloop/state/backlog.json");
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "task-1").unwrap()["status"], "done");
    assert_eq!(
        std::fs::read_to_string(ws.join("made.txt")).unwrap().trim(),
        "made"
    );
    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

/// Zombie mode (b): ready item depending on an id that is not in the backlog
/// (e.g. a leaked sub-item id) could never dispatch.
#[tokio::test]
async fn ready_task_with_unknown_dep_is_unstuck_and_completes() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_keep_backlog_stub();
    write_backlog(
        &ws,
        r#"{"items":[{"id":"task-1","title":"f","desc":"d","deps":["task-ghost"],"status":"ready","attempts":0,"acceptance":"file exists"}]}"#,
    );
    set_env(&ws);
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();

    assert_eq!(merged, 1, "unknown dep stripped; item dispatches this round");
    let bk = ws.join(".agentloop/state/backlog.json");
    let v = state::read(&bk).unwrap();
    let task = state::item(&v, "task-1").unwrap();
    assert_eq!(task["status"], "done");
    assert_eq!(task["deps"], serde_json::json!([]));
    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}

/// Zombie mode (c): valid plan whose remaining ready builders dep on failed
/// builders deadlocked the parent forever.
#[tokio::test]
async fn deadlocked_builder_plan_is_redesigned_and_completes() {
    let _env = ENV_LOCK.lock().await;
    let ws = common::init_ws_with_keep_backlog_stub();
    write_backlog(
        &ws,
        r#"{"items":[{"id":"task-1","title":"f","desc":"d","deps":[],"status":"in_progress","attempts":0,"acceptance":"file exists"}]}"#,
    );
    let tdir = ws.join(".agentloop/state/tasks/task-1");
    std::fs::create_dir_all(&tdir).unwrap();
    std::fs::write(tdir.join("design.md"), "old design").unwrap();
    std::fs::write(
        tdir.join("builders.json"),
        r#"{"items":[
            {"id":"task-1-b1","title":"t","desc":"d","deps":[],"status":"failed","attempts":3,"acceptance":"a"},
            {"id":"task-1-b2","title":"t","desc":"d","deps":["task-1-b1"],"status":"ready","attempts":0,"acceptance":"a"}
        ]}"#,
    )
    .unwrap();
    set_env(&ws);
    let rep: Arc<dyn Reporter> = Arc::new(EventLineReporter);

    let merged = orchestrator::iterate(&cfg(), &ws, 1, &rep).await.unwrap();

    assert_eq!(merged, 1, "deadlocked plan is redesigned and rebuilt this round");
    let bk = ws.join(".agentloop/state/backlog.json");
    let v = state::read(&bk).unwrap();
    assert_eq!(state::item(&v, "task-1").unwrap()["status"], "done");
    assert!(
        !ws.join(".agentloop/state/tasks/task-1/redesign.json").exists(),
        "redesign counter resets on completion"
    );
    clear_env();
    let _ = std::fs::remove_dir_all(&ws);
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --test loop_repair_test`
Expected: PASS (3 tests). If `in_progress_task_without_plan...` fails with merged=0, check that `repair_backlog` runs **before** `state::ready_items` is computed for the architect pass in `iterate`.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add tests/common/mod.rs tests/loop_repair_test.rs
git commit -m "test(loop): end-to-end coverage for stuck-backlog repair"
```

---

### Task 11: Manager prompt — backlog ownership rules + stuck-item report

Prevention for issue 4: the manager must never write `in_progress`, never dep on ids outside the backlog, never copy task-local sub-items into the backlog, and must reshape items stuck behind `failed` items (reported to it each round).

**CONSTRAINT:** `manager_prompt_is_business_only` forbids the substrings `design.md`, `builders.json`, `role`, `architect`, `builder`, `builders` in this prompt. The wording below was chosen to comply — do not "improve" it with those words.

**Files:**
- Modify: `src/manager.rs`
- Test: `tests/roles_prompt_test.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/roles_prompt_test.rs` (match the file's existing imports/helpers; create a self-contained ws):

```rust
#[test]
fn manager_prompt_hardens_backlog_ownership() {
    let ws = std::env::temp_dir().join(format!(
        "mgr-rules-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(ws.join(".agentloop/state")).unwrap();
    let p = manager::manager_prompt(&ws, 3);
    assert!(p.contains("NEVER write \"in_progress\""));
    assert!(p.contains("must be the id of another item in this backlog.json"));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn manager_prompt_reports_items_stuck_on_failed_deps() {
    let ws = std::env::temp_dir().join(format!(
        "mgr-stuck-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let st = ws.join(".agentloop/state");
    std::fs::create_dir_all(&st).unwrap();
    std::fs::write(
        st.join("backlog.json"),
        r#"{"items":[
            {"id":"task-1","title":"a","desc":"d","deps":[],"status":"failed","attempts":3,"acceptance":"x"},
            {"id":"task-2","title":"b","desc":"d","deps":["task-1"],"status":"ready","attempts":0,"acceptance":"x"}
        ]}"#,
    )
    .unwrap();
    let p = manager::manager_prompt(&ws, 3);
    assert!(p.contains("STUCK ITEMS"));
    assert!(p.contains("task-2 depends on failed task-1"));
    let _ = std::fs::remove_dir_all(&ws);
}
```

Run: `cargo test --test roles_prompt_test manager_prompt`
Expected: the two new tests FAIL; `manager_prompt_is_business_only` still passes.

- [ ] **Step 2: Implement in `src/manager.rs`**

2a. In `manager_prompt`, after the `requests` line, compute the stuck block:

```rust
    let requests = crate::requests::prompt_block(ws).unwrap_or_default();
    let stuck = state::failed_dep_report(&st.join("backlog.json"))
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| {
            format!(
                "\n\nSTUCK ITEMS — these depend on failed items and can never run; reshape or drop them this round:\n{s}"
            )
        })
        .unwrap_or_default();
```

2b. Extend the numbered job list (items 1-5 exist; add 6-8 after item 5):

```
6. Statuses you may write are "ready", "blocked", and "failed". NEVER write "in_progress" — the orchestrator owns that transition.
7. Every id inside any "deps" array must be the id of another item in this backlog.json. Never invent ids and never copy task-local sub-items (ids like "task-3-b2", which live under .agentloop/state/tasks/) into this backlog; the orchestrator strips deps on unknown ids.
8. If an open item depends on a "failed" item it can never run: reshape or drop it this round.
```

2c. Append `{stuck}` to the format string's final line and bindings:

```rust
Do not print the JSON to stdout; write the files.{requests}{stuck}"#,
        ws = ws.display(),
        goal = goal,
        master = master,
        backlog = backlog,
        max_attempts = max_attempts,
        requests = requests,
        stuck = stuck
    )
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --test roles_prompt_test`
Expected: PASS — including `manager_prompt_is_business_only` (the new wording avoids all forbidden substrings).

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add src/manager.rs tests/roles_prompt_test.rs
git commit -m "feat(manager): harden backlog ownership rules and report stuck items"
```

---

### Task 12: README + final verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update README.md**

1a. Intro paragraph (lines 9-13): replace

```
When run in a terminal it shows a live TUI: a goal-entry screen lets you confirm or
edit the goal before anything runs, then a progress panel, an inbox for answering
questions that agents raise, and a persistent input bar for adding tasks.
```

with

```
When run in a terminal it shows a live TUI: a goal-entry screen lets you confirm or
edit the goal before anything runs, then a jobs panel and a persistent input bar for
adding tasks. Questions agents raise are answered automatically ("you decide"), so
the loop never waits on you.
```

1b. Replace the "Question inbox" bullet in "How it works" with:

```
- **Autonomous decisions:** a builder that hits a decision writes
  `.agentloop/questions/<id>.json` and reports `status:"needs_input"`. The loop answers
  it immediately with a canned "decide the best option for me — you decide" reply
  (stored in `.agentloop/answers/<id>.json`) and re-dispatches the item with the Q&A
  appended to its prompt. Nothing waits on the user.
- **Usage limits:** when claude/codex dies on a provider usage/rate limit, agentloop
  parses the reset time when the output includes one (else waits a fallback window),
  logs a ⏳ note to the job log, and re-runs the agent automatically. Quitting
  interrupts the wait.
- **Backlog repair:** after every manager round the orchestrator drops deps on ids
  that don't exist in the backlog, re-architects `in_progress` tasks that lost their
  plan, and redesigns plans whose remaining items can never dispatch — the loop never
  idles while open work exists.
```

1c. Rewrite the "Interactive mode (TUI)" key list (remove inbox/tab/answering):

```
- Printable keys always type into the persistent bottom input bar; it wraps long text
  automatically. `shift+enter` (or `alt+enter`) inserts a newline.
- `enter` — submits the input as a new task for the manager. When the input is empty,
  `enter` opens the selected job's detail view (live log tail + a real-time working
  timer).
- `↑`/`↓` — navigate the jobs list, or scroll the log in the job-detail view
- `esc` — clear the input bar, or leave the job-detail view
- `q` — quit (only when the input bar is empty); `Ctrl-C` always quits
```

1d. Update the status-bar sentence (remove "a pending-questions counter"):

```
The status bar shows the goal, current iteration, gate state, open-item count, and a
live `⏱` total-run-time readout; `✓ DONE · standby` appears when the run is idle and
waiting. (Headless runs print the total elapsed time on exit.)
```

1e. Replace "The main panel stacks the Jobs pane on top and the Inbox pane below (full width each)." with "The main panel is the Jobs list (full width)."

1f. In the Layout section, update two lines:
- `  events.rs        Reporter trait, Event/Command enums, stderr + channel reporters` (unchanged)
- `  inbox.rs         question/answer file IO + prior-Q&A prompt block (auto-answered)`
- Add after the `inbox.rs` line: `  limits.rs        usage/rate-limit detection + auto-continue wait math`

- [ ] **Step 2: Full verification**

Run:
```bash
cargo fmt --check && cargo build && cargo test
```
Expected: clean fmt, green build, all tests pass.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: autonomy (no inbox), usage-limit auto-continue, backlog repair"
```

- [ ] **Step 4 (manual, optional — spends tokens): verify on the stuck workspace**

Run agentloop against `/Users/ngthluu/choscor/test-chat-app` **without** `--fresh`. Expected within the first iteration (visible in `.agentloop/state/backlog.json` and the TUI):
- `task-9-b2r/b3r/b5/b6` lose their unknown deps (notes say `removed deps on unknown ids: ...`) and dispatch.
- `task-4`, `task-5`, `task-7` flip `in_progress` → `ready` and get re-architected.
- `task-9` is reopened for redesign (`builder plan deadlocked` note).
- Customer jobs show `✓`/`✗` with a frozen timer instead of `?` + ticking.

---

## Execution notes

- Tasks 5 and 6 form one compile unit — commit them together (the plan says so explicitly).
- Tests that mutate `FAKE_AGENT`/`WS`/`AGENTLOOP_*` env vars must hold their file's `ENV_LOCK` (separate test binaries are separate processes, so cross-file races don't occur).
- Never add `AGENTLOOP_LIMIT_*` env mutation to `--lib` unit tests (limits.rs tests assume defaults).
