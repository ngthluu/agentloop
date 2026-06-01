# Live Progress Tracking — Design

## Problem

During a run, agentloop prints output only at iteration boundaries — a single
`iter N: merged=… gate=… open=…` line in `lib/loop.sh:96`. The expensive work
between those lines is invisible:

- The **planner** runs synchronously (`planner_run`) for up to `item_timeout_sec`.
- Up to `max_parallel` **workers** run as backgrounded subshells, then the parent
  blocks on `wait` (`lib/loop.sh:53`).

Agents write only to log files, so the user stares at a frozen screen for the
bulk of every iteration. The display is effectively static. We want a live view
of what's happening: which task started, with what tool/model, how long it's been
running, and what it's doing right now.

## Goals

- A live, redrawing dashboard on an interactive terminal showing every in-flight
  job (planner + workers) with a live elapsed timer and a live tail of its log.
- Graceful degradation to append-only event lines when output is not a TTY
  (pipes, CI, the offline test suite).
- Keep `lib/loop.sh` as pure control flow; isolate all display logic in a new
  `lib/progress.sh`.
- Change nothing about the existing `iter N`/`DONE`/`STOP` summary output that
  callers and tests rely on — progress is purely additive.

## Non-Goals

- No browser/web UI. This is a terminal tool.
- No structured parsing of agent CLI output. The live tail is the last non-empty
  log line, stripped and truncated — best-effort, not a parsed activity model.
- No new config knobs (refresh interval is a hardcoded constant). YAGNI.

## Decisions (from brainstorming)

1. **Display style:** redrawing dashboard on a TTY; event log lines otherwise.
   Auto-detected once via `[ -t 2 ]`.
2. **Row detail:** id, title, tool/model, status, elapsed timer, **plus** a live
   tail of the agent's log line underneath.
3. **Non-TTY fallback:** append-only event lines (dispatch / done / merge / gate).

## Architecture

### Rendering driver: inline render loop replaces `wait`

The parent process owns all job state and renders single-threaded. Instead of
blocking on `wait`, it runs a poll-and-redraw loop:

- every ~1s, redraw the current frame (read state files + tail each log);
- check `kill -0` on each tracked pid;
- when all tracked pids are dead, do a final render and return.

This is used for **both** phases:

- **Planner phase:** background `planner_run` in a subshell, register one job
  ("planner"), render until it exits, then reap and read its exit code.
- **Worker phase:** workers are already backgrounded (`lib/loop.sh:50`); register
  each job's state before `&`, then replace the bare `wait` with the render loop.

Rejected alternative — a **separate background renderer process** — would put two
processes on the terminal, add a second lifecycle to tear down on both completion
and Ctrl-C, and interact badly with the existing recursive `kill_tree` trap. The
inline loop is the smallest, safest change.

### Frame redraw mechanism

Track the number of lines printed in the previous frame. Each new frame moves the
cursor up that many lines (`\033[<n>A`) and clears to end of screen (`\033[J`)
before reprinting. No full-screen `clear`, so no flicker. The cursor is **not**
hidden — there is nothing to restore, which keeps the existing Ctrl-C trap clean.

### State directory

`.agentloop/state/progress/` holds one tiny file per job, written by the **parent**
(it already resolves tool/model via `config_resolve_role` / `config_role_field`):

```
{ "id", "label", "tool", "model", "log", "start_epoch", "status" }
```

- `status`: `running` → set to `merged` / `failed` / `bounced` by the integration
  pass, or `done` for the planner.
- The renderer is pure read: it scans this dir each frame, computes elapsed from
  `start_epoch`, and tails `log`.
- The dir is wiped at the start of each iteration so rows reflect the current
  iteration only.

### Data flow per iteration

```
loop_iterate
  ├─ progress_reset(statedir)
  ├─ planner: progress_register "planner" …; ( planner_run ) & ; progress_wait
  ├─ for each ready id:
  │     write worktree, set in_progress, increment attempts
  │     progress_register id title tool model log
  │     ( worker_dispatch … ) &
  ├─ progress_wait            # replaces `wait` — redraws until all pids dead
  ├─ integration pass: on each result, progress_set_status id merged|failed|bounced
  ├─ progress_render_final    # one last frame showing ✓/✗ outcomes
  └─ existing: echo "iter N: merged=… gate=… open=…"   (unchanged, below frame)
```

## Components

### `lib/progress.sh` (new)

All display logic. Sourced by `lib/loop.sh`.

- `progress_init` — set `PROGRESS_TTY` from `[ -t 2 ]` once.
- `progress_reset <statedir>` — clear the progress state dir for a fresh iteration.
- `progress_register <statedir> <id> <label> <tool> <model> <log>` — write the job
  state file (status=running, start_epoch=now). On non-TTY, also emit a
  `dispatch` event line.
- `progress_set_status <statedir> <id> <status>` — update a job's status. On
  non-TTY, emit a `done`/`merge` event line.
- `progress_wait <statedir> -- <pid…>` — the poll/redraw loop. On TTY, redraw a
  frame every ~1s until all pids are dead. On non-TTY, just `wait` for the pids
  (event lines were already emitted by register/set_status).
- `progress_render <statedir>` — draw one frame: header (iter, budget elapsed,
  gate, open count) + one row per job (running-first), each with its tailed log
  line underneath.
- `progress_render_final <statedir>` — one terminal frame after integration.
- Helpers (pure, unit-testable): `progress_fmt_elapsed <secs>`,
  `progress_strip_ansi`, `progress_truncate <width>`, `progress_tail_log <file>`
  (last non-empty stripped line, or a `starting…` placeholder when empty).

### `lib/loop.sh` (changed)

- Source `lib/progress.sh`; call `progress_init` at the top of `loop_run`.
- `loop_iterate`: `progress_reset`; background and track the planner; write each
  worker's state file before `&`; replace `wait` with `progress_wait`; call
  `progress_set_status` inside the integration loop; `progress_render_final` after.
- Keep the `iter N`/`DONE`/`STOP` lines exactly as they are, printed below the
  final frame.

## Error handling & edge cases

- **Non-TTY** (`PROGRESS_TTY=0`): no cursor control; `progress_wait` degrades to a
  plain `wait`; visibility comes from append-only event lines. The offline test
  suite exercises this path.
- **Ctrl-C / TERM:** existing `kill_tree` trap is unaffected — cursor was never
  hidden, no renderer process to reap.
- **Empty agent log:** `claude -p` may emit little until it finishes; the row tail
  shows a `starting…` placeholder rather than a blank. *Known limitation:* tail
  richness depends on what each CLI streams (`codex exec` is chatty, `claude -p`
  can be quiet until the end).
- **Narrow terminal:** rows truncate to terminal width; the previous-line-count
  tracking handles frames that shrink or grow between redraws.
- **Planner re-prompt:** `planner_run` may invoke the agent twice internally; it is
  tracked as a single "planner" job over the whole call.

## Testing (TDD, offline / fake-agent)

- Unit-test pure helpers: `progress_fmt_elapsed`, `progress_strip_ansi`,
  `progress_truncate`, `progress_tail_log` (incl. empty-log placeholder).
- `progress_render` against a fixture progress state dir + fake log files →
  assert header and per-row content.
- Non-TTY path: assert `progress_register` / `progress_set_status` emit the
  expected `dispatch` / `done` event lines, and that `progress_wait` behaves like
  `wait`.
- Regression: assert the existing `iter N` / `DONE` / `STOP` stderr output is
  unchanged under the fake agent.

## Files

```
lib/progress.sh   (new)   all progress display logic + state-file IO + helpers
lib/loop.sh       (edit)  init progress; track planner; replace wait; set status
tests/            (new)   progress unit + non-TTY event-line + regression tests
```
