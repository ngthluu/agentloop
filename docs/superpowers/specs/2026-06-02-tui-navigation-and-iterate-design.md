# TUI navigation, working-time, stable frame, and additive re-run

Date: 2026-06-02
Status: Approved (design); ready for implementation plan

## Summary

Four improvements to the `agentloop` interactive TUI and re-run behavior:

1. **Stable in-place frame** — stop orchestrator `eprintln!` diagnostics from
   scrolling over the ratatui alt-screen.
2. **Navigate jobs ↔ inbox + job detail** — focus switching between the Jobs and
   Inbox panes, and a per-job detail view that tails the job's live log.
3. **Real-time working time** — a live elapsed timer per job, frozen at completion.
4. **Re-run = more context, never a reset** — re-running with new goal text adds it
   as work on top of the existing effort instead of being ignored (which currently
   shows an instant "Done, nothing changed").

These are independent enough to implement and review in sequence, but share the
`tui.rs` view-model and `events.rs` types, so they ship together.

## Context

Relevant existing files:

- `src/app.rs` — wires orchestrator + TUI over channels; sets up/tears down the
  ratatui terminal (alt-screen, raw mode). Event loop redraws every ~80 ms.
- `src/tui.rs` — `AppState` view-model (events → state, keys → commands) + `render`.
  `Job` currently holds `id, label, tool, model, status`. Navigation (`↑/↓`) only
  moves within the inbox; jobs are not focusable and have no detail view.
- `src/events.rs` — `Reporter` trait, `Event`/`Command` enums, `EventLineReporter`
  (stderr lines, headless) and `ChannelReporter` (forwards to the TUI).
- `src/orchestrator.rs` — iteration loop; computes per-item log path
  `.agentloop/logs/iter-{n}/item-{id}.log` and the planner log `planner.log`. Emits
  many `eprintln!` diagnostics (DONE, STOP…, planner failed, worker errors).
- `src/cli.rs` — arg parsing + `bootstrap_workspace`. `goal.md` is only written
  `if !goalf.exists()`, so a re-run keeps the old goal.
- `src/planner.rs` / `src/requests.rs` — planner prompt folds pending requests from
  `requests.jsonl` into the backlog, then marks them consumed.

## Feature 1 — Stable in-place frame

**Cause.** Every `eprintln!` in `orchestrator.rs` / `planner.rs` / `worktree.rs` /
`spawn.rs` writes to the process's stderr (fd 2). While the ratatui alt-screen owns
the terminal, those writes scroll the screen and force full repaints — observed as
the panel piling up in scrollback rather than updating in place.

**Change (`app.rs`).** For the lifetime of the TUI, redirect the process's stderr fd
to `.agentloop/logs/run.log`:

- After `setup_terminal()` and before the event loop: open/create `run.log`,
  `nix::unistd::dup(stderr)` to save the original stderr, then
  `nix::unistd::dup2(logfile_fd, stderr_fd)`.
- In/after `restore_terminal()`: `dup2(saved_fd, stderr_fd)` to restore, so the
  post-exit summary still reaches the real terminal.
- Unix-only, behind `#[cfg(unix)]`, using the existing `nix` dependency (already
  pulled in for signal/process handling). No-op on non-Unix.

Orchestrator diagnostics then land in `run.log` (still inspectable), the TUI renders
one stable frame, and no `eprintln!` call sites change. The headless path
(`EventLineReporter`) keeps stderr untouched.

## Feature 2 — Navigate jobs ↔ inbox + job detail

### State (`tui.rs`)

```rust
enum Focus { Jobs, Inbox }
enum View  { List, JobDetail }
```

`AppState` gains:

- `focus: Focus` — default `Inbox` (preserves current key behavior).
- `view: View` — default `List`.
- `selected_job: usize` — selection within the Jobs pane (separate from `selected`,
  which remains the Inbox selection).
- `log_scroll: u16` — detail-view scroll offset; `0` follows the live tail.

`Job` gains `log_path: Option<PathBuf>`.

### Events (`events.rs`)

- `Event::JobDispatched` and `Reporter::dispatch` gain a `log_path` argument.
- Orchestrator passes the worker log path `ldir.join("item-{id}.log")` and the
  planner's `planner.log`.
- `EventLineReporter` and `ChannelReporter` are updated to the new signature;
  `EventLineReporter` ignores the path.
- `AppState::apply` stores `log_path` on the matching `Job`.

### Keys

Normal mode (`View::List`):

- `Tab` — toggle `focus` between Jobs and Inbox.
- `↑/↓` — move selection within the focused pane (`selected_job` or `selected`).
- `Enter` — if `focus == Jobs`, set `view = JobDetail`; if `focus == Inbox`, answer
  the selected question (unchanged).
- `a` add-task, `q` quit — unchanged.

`View::JobDetail`:

- `↑/↓` — scroll the log (`log_scroll`).
- `Esc` — return to `View::List`.
- Tab / answer / add-task are inert in this view.

### Render

- The focused pane draws a bright/bold border; the unfocused pane stays dim.
- When `view == JobDetail`, the two-pane body is replaced by the detail layout:
  - **Header:** title, status glyph, role, `tool/model`, elapsed time.
  - **Log region:** bordered area tailing the selected job's `log_path`.
- `tail_file(path, max_lines ≈ 400, max_bytes ≈ 32 KB)` reads the last lines each
  tick; `log_scroll` offsets upward from the bottom (`0` = follow live tail). A
  missing/empty file renders `(no output yet)`.

Approved detail layout (header + live log tail):

```
┌ Job: it-3 — build auth middleware ─────┐
status: ● running   role: build   3m12s
tool: claude / sonnet
├────────────────────────────────────────┤
  ...writing src/auth/mw.rs
  running cargo check
  warning: unused import
  > applying fix█
└ [esc] back  [↑↓] scroll ────────────────┘
```

## Feature 3 — Real-time working time

`Job` gains `started: Option<Instant>` and `frozen: Option<Duration>`.

- On `JobDispatched` or status → `running`: set `started = Some(Instant::now())` and
  clear `frozen`. (A bounced item that re-dispatches restarts its timer.)
- On a terminal status (`merged` / `done` / `failed` / `bounced`): set
  `frozen = started.map(|s| s.elapsed())`.
- `render` computes the displayed duration as
  `frozen.or_else(|| started.map(|s| s.elapsed()))` and formats via `fmt_elapsed`:
  - `< 60s`  → `{s}s`
  - `< 1h`   → `{m}m{s:02}s`
  - `>= 1h`  → `{h}h{m:02}m`
- Displayed right-aligned per row in the Jobs list and in the detail header. It is
  live because `render` runs each ~80 ms tick. `Instant` is in-memory only (`Job`
  is not serialized), so there is no serde impact.

## Feature 4 — Re-run = more context, never a reset

Any new goal text on a re-run is treated as **additional context layered onto the
same effort** — never a different goal that resets or pivots.

**`cli.rs` `run()`** (after bootstrap, when `--fresh` is not set):

- Read the existing `goal.md`. If it already existed *and* its text does not already
  contain the new CLI goal arg (trimmed compare), treat the arg as added context:
  1. `requests::append(ws, &goal)` — queues it so the planner folds it into the
     backlog as new task(s) on its next round.
  2. Append it to `goal.md` under an accumulating section, e.g.
     `\n## Added <timestamp>\n{goal}\n`, so the overarching goal grows rather than
     being replaced.
- If `goal.md` did not previously exist (first run): `bootstrap_workspace` writes it
  as today.
- Identical re-run text already present in `goal.md` → no-op (just resumes; standby
  or Done as appropriate).

`bootstrap_workspace` keeps writing `goal.md` only on first creation; the additive
logic lives in `run()` so it can distinguish a pre-existing workspace from a new one.
Because a pending request now exists, the planner produces open items and the loop
re-engages, eliminating the instant "Done, nothing changed."

This applies to both the interactive (`run_interactive`) and headless (`run`) paths,
since both run the planner first and the planner consumes pending requests.

## Testing

- **Re-run additive** (`tests/`, offline, no tokens): bootstrap a workspace, run the
  planner once, then re-run with extra goal text. Assert `requests.jsonl` gained a
  pending entry and `goal.md` contains both the original and the added text. Assert a
  re-run with identical text adds nothing.
- **Time formatting**: unit test `fmt_elapsed` across the three ranges
  (`s` / `m+s` / `h+m`).
- **Job detail / focus** (manual, `tui_demo.sh` checklist): `Tab` to the Jobs pane,
  `Enter` opens detail with a live log tail and a ticking timer, `Esc` returns;
  verify no scrollback pile-up and a clean terminal restore on quit.
- Gate: `cargo test` and `cargo build --release` green; `tui_demo.sh` passes by eye.

## Out of scope (YAGNI)

- Mouse support.
- Log search / filtering in the detail view.
- Persisting timers across process restarts.
- Non-Unix stderr redirect (the frame fix is Unix-only; other platforms are no-op).
