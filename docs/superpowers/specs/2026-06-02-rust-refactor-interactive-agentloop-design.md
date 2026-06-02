# Design: Rust refactor of agentloop + interactive features

**Date:** 2026-06-02
**Status:** Approved design, pending implementation plan

## Summary

Port the existing bash `agentloop` orchestrator to a single Rust binary with the
same architecture and on-disk contract, then add three interactive features:

1. **Question inbox** — agents can ask the user a question (async, file-based) and
   pause that item until answered.
2. **TUI navigation** — a `ratatui` terminal UI showing live progress and a
   navigable inbox to answer pending questions.
3. **Add-task + standby** — add new tasks at any time (routed through the planner),
   and on "done" stay alive in a standby state instead of exiting.

The build follows **Approach A (target-architecture, phased)**: an async,
event-driven core is built from day one so the interactive features plug in
without reworking the loop.

## Goals

- Single `agentloop` Rust binary replacing the bash scripts and the python YAML helper.
- Preserve the `.agentloop/` on-disk contract, the YAML routing config format, and the
  planner/worker prompt text, so existing workspaces remain readable and behavior matches.
- Behavior parity validated against the existing offline (fake-agent) test scenarios.
- Live, interactive operation: watch progress, answer agent questions, add tasks,
  and keep the session alive after completion.

## Non-goals

- Changing the planner/worker prompt semantics or the routing model.
- Replacing `git` with a library (we shell out to `git`, as today).
- A standalone daemon/server, web UI, or multi-user operation.
- `agentloop add-task "..."` out-of-band subcommand for a running/stopped workspace
  (possible later nicety; out of scope for this design).

## Approach

**Approach A — target-architecture, phased.** Build the async core first, ship in
three phases:

- **Phase 1:** behavior-parity headless port (same on-disk state, same prompts),
  validated against ported test scenarios.
- **Phase 2:** `ratatui` progress panel + question inbox.
- **Phase 3:** live add-task (through planner) + standby-on-done lifecycle.

Rejected: a literal 1:1 synchronous port first (Approach B) — its sequential loop
would be torn down and rebuilt event-driven for the TUI, producing throwaway work.

## Module layout

The crate mirrors the bash libs one-to-one where possible:

| bash | Rust module | role |
|---|---|---|
| `agentloop.sh` | `main.rs` + `cli.rs` | arg parse (clap), bootstrap workspace, run |
| `lib/config.sh` + `helpers/yaml2json.py` | `config.rs` | load `config.yaml` via `serde_yaml`; role resolution. Python helper deleted. |
| `lib/state.sh` | `state.rs` | `backlog.json` model + atomic mutations (`serde_json`, temp+rename) |
| `lib/spawn.sh` | `spawn.rs` | build claude/codex argv; run with timeout + process-group kill |
| `lib/worktree.sh` | `worktree.rs` | shell out to `git` (worktree add/merge/remove) |
| `lib/planner.sh` | `planner.rs` | planner prompt + invoke + validate |
| `lib/worker.sh` | `worker.rs` | worker prompt + dispatch |
| `lib/progress.sh` | `tui.rs` | replaced by ratatui panel (progress + inbox) |
| `lib/loop.sh` | `orchestrator.rs` | plan→dispatch→integrate→gate loop, async |
| *(new)* | `inbox.rs` | question/answer data flow |
| *(new)* | `events.rs` | `Event`/`Command` enums exchanged over channels |

**Dependencies:** `tokio` (async + subprocess), `clap`, `serde` / `serde_json` /
`serde_yaml`, `ratatui` + `crossterm`, `command-group` or `nix` (process-group
spawn/kill to preserve the kill-tree behavior), `anyhow`.

## Concurrency model

Three concurrent actors over `tokio`, coordinated by an event loop in `main`:

```
        Command (mpsc)                    Event (mpsc)
  TUI  ───────────────►  Orchestrator  ───────────────►  TUI
 (input thread)            (async task)    (registers,      (render loop)
                                │           status, logs,
                                │           questions...)
                          spawns│
                                ▼
                    Agent subprocesses (claude/codex)
                    each in its own git worktree
```

- **Orchestrator** (one async task): owns the canonical run state and is the single
  writer of `.agentloop/` files. Runs the loop; uses `tokio::select!` over (a) agent
  subprocess completions, (b) a periodic tick (budget/stall checks + log-tail refresh),
  and (c) incoming `Command`s. Emits `Event`s.
- **Agents:** spawned by `spawn.rs` as child processes in worktrees, capped by a
  per-item timeout; each awaited by a tokio task. Parallelism bounded by `max_parallel`.
- **TUI:** a render loop (~10–15 fps) reading the latest snapshot, plus a `crossterm`
  input reader. Key presses become `Command`s. The TUI never mutates run state; it only
  sends commands and renders snapshots (read-only mirror updated from `Event`s).

**Message types** (`events.rs`):

- `Event` (orchestrator → TUI): `JobRegistered`, `JobStatus`, `LogTail`,
  `IterationResult { merged, gate, open }`, `QuestionRaised { item_id, text }`,
  `EnteredStandby`, `Shutdown`.
- `Command` (TUI → orchestrator): `AnswerQuestion { item_id, text }`,
  `AddTask { request }`, `Quit`.

**State ownership.** The actor model (single writer + read-only mirror over channels)
avoids `Arc<Mutex>` races and keeps the render loop responsive while agents run.

**Headless mode.** When stdout isn't a TTY (or `--headless`), the same orchestrator
runs but the TUI is replaced by a plain event-line printer, mirroring today's non-TTY
`progress.sh` behavior and what the test suite drives.

**Cancellation.** A `CancellationToken` / broadcast shutdown propagates `Quit`/Ctrl-C to
all agent tasks, which kill their process groups — preserving the bash `kill_tree`
semantics so no orphaned `claude`/`codex` keep burning credits.

## Phase 1 — behavior-parity port

A headless Rust binary reproducing today's behavior, validated against the existing
test scenarios, with no new features.

- **Bootstrap** (`main.rs`/`cli.rs`): parse `--workspace/--config/--fresh/--max-iterations/--dry-run`
  + goal; create `.agentloop/{state,results,logs,worktrees}`; `git init` if needed, set
  local user, append `.agentloop/` to `.gitignore`, ensure one commit exists; copy default
  `config.yaml`/`master.md`; seed `goal.md` and empty `backlog.json`. Ctrl-C/SIGTERM →
  graceful kill-tree.
- **Config** (`config.rs`): `serde_yaml` into a typed struct (`caps`, `routing: Map<role,
  {tool, model, effort, flags}>`, `defaults.role`). Typed `resolve_role`, `role_field`,
  `cap` accessors. `--max-iterations` override applied in-memory.
- **State** (`state.rs`): `Backlog { items: Vec<Item> }`, `Item { id, title, desc, role,
  deps, status, attempts, acceptance, notes }`. Ports `backlog_valid`, `ready_items`
  (deps-all-done, capped at `max_parallel`), `open_count`, `set_status`,
  `increment_attempts` — all atomic temp-file + rename.
- **Spawn** (`spawn.rs`): builds the identical argv (`claude -p <prompt>` /
  `codex exec <prompt>` with model/effort/flags mapping). Runs in a new process group;
  on timeout, SIGTERM the group, brief grace, SIGKILL — same 124-on-timeout semantics. The
  `FAKE_AGENT` interception hook is preserved so the offline test suite works unchanged.
- **Worktree** (`worktree.rs`): `git worktree add -b item/<id> … HEAD`, merge `--no-edit`
  (abort on conflict → non-zero), remove + branch -D.
- **Planner/Worker** (`planner.rs`, `worker.rs`): prompt text copied verbatim;
  `planner_run` validates `backlog.json` and re-prompts once on invalid; `worker_dispatch`
  resolves role and runs the agent in the worktree.
- **Orchestrator** (`orchestrator.rs`): the loop from `loop.sh` — per iteration run
  planner (tracked job), select ready items, dispatch in parallel worktrees (respecting
  `max_attempts`), await completions, integrate sequentially (commit-check → merge →
  status; conflicts/no-commits bounce to `ready`), run `verify.sh` gate, then termination
  logic (DONE on gate-pass + open==0; stop on max_iterations, budget, or 2 stalls). Phase 1
  emits plain event-lines; the async `select!` structure is in place so Phases 2–3 plug in
  without rework.

**Verification:** port the offline scenarios from `tests/` (fake-agent runs of
config/state/spawn/worktree/loop/prompts) to Rust integration tests, asserting the same
outcomes (merged counts, statuses, stall/cap stops). This is the parity gate for Phase 1.

## Phase 2 — question inbox

**Agent raises a question.** The worker/planner prompt gains a clause: *if blocked needing
a decision only the user can make, write `.agentloop/questions/<item_id>.json` =
`{"question": "...", "context": "..."}` and write the result file with
`"status":"needs_input"` instead of done/failed, then stop.* One pending question per item
per round; the agent can ask again next round.

**Orchestrator detects it.** During integration, a result with `status:"needs_input"`:

- The item's worktree/branch is cleaned up (no commits to merge); backlog status set to
  `blocked` with the question stored in `notes`.
- A `QuestionRaised { item_id, text }` event goes to the TUI.
- Termination logic is adjusted: a run whose only open work is `blocked` items awaiting
  the user enters an *awaiting-input* state rather than tripping the no-progress stall.

**User navigates & answers (TUI inbox).** Pending questions form a list the user arrow-keys
through; each shows item title, question, and context. The user types an answer inline and
submits → TUI sends `AnswerQuestion { item_id, text }`.

**Answer routes back.** The orchestrator writes `.agentloop/answers/<item_id>.json`, appends
the Q&A to the item, and flips it `blocked` → `ready`. On the next dispatch, `worker_prompt`
includes a **prior Q&A block** (question + answer) so the re-spawned one-shot agent has the
missing context. The question file is consumed (archived under `logs/`).

```
agent → questions/<id>.json + result:needs_input
   → orchestrator: item=blocked, notes=question, emit QuestionRaised
      → TUI inbox (navigate, type answer)
         → Command::AnswerQuestion
            → answers/<id>.json, item=ready, Q&A appended to prompt
               → re-dispatch with prior-Q&A block
```

Other agents keep running and merging while questions sit unanswered — answering one item
never blocks unrelated in-flight work.

## Phase 3 — add-task + standby lifecycle

**Adding a task (routes through the planner).** In the TUI the user presses `a`, types a
natural-language request, submits → `Command::AddTask { request }`. The orchestrator appends
it to `.agentloop/state/requests.jsonl` (append-only log; each `{ts, text,
status:"pending"}`). The **planner prompt gains a "PENDING USER REQUESTS" section** listing
unconsumed requests; the planner folds them into the backlog (new/split items, roles, deps,
updated `verify.sh`) on its next run, then marks them consumed. The planner remains the
single owner of the backlog — the user feeds intent, not hand-authored items.

**Standby on done.** Reaching DONE (gate-pass + open==0) or a cap/stall no longer exits the
process. The orchestrator transitions to a **`Standby` state** and emits `EnteredStandby`. In
standby it idles, awaiting a `Command`:

- `AddTask` → record the request, **re-engage**: reset the stall counter, resume iterating
  (planner picks up the new request next iteration).
- `AnswerQuestion` → as Phase 2 (answering a leftover blocked item also re-engages).
- `Quit` → clean shutdown.

**Re-engagement / caps policy.** Standby is free (no budget consumed). The budget clock
pauses on entering standby; a new `AddTask` starts a fresh budget window and resets the
stall counter. The iteration counter continues to increment across engagements, but
`max_iterations` is evaluated per-engagement so a resumed run isn't immediately capped.

**Headless mode** does not idle — without a TTY there's no interactive adder, so headless
preserves today's exit-on-done behavior. Standby is a TUI-mode feature.

**On-disk additions:** `state/requests.jsonl` (user requests), `questions/` + `answers/`
(Phase 2). Everything else unchanged.

## TUI layout

A single full-screen `ratatui` app: top status bar, a main split (progress left, inbox
right), and a context-sensitive footer of keybindings.

```
┌ agentloop ─────────────────────────────────── iter 4 · elapsed 12m03s/6h ┐
│ goal: Build a Python CLI todo app with a passing pytest suite            │
│ gate: ✗ fail   ·   open: 3   ·   running: 2   ·   done: 5   ·  ❓ 1       │
├──────────────────────────── jobs ────────────┬──────── inbox (1) ────────┤
│ ● planner    claude/opus    planning   0m41s │ ▸ db-schema               │
│   └ writing backlog.json…                    │   "Postgres or SQLite     │
│ ● api-routes codex/gpt-5.5  in_progress 3m12s │    for storage?"          │
│   └ added GET /todos handler…                │                           │
│ ◍ db-schema  claude/opus    finishing  2m05s │   tests-e2e               │
│ ✓ scaffold   codex/gpt-5.5  merged     1m18s │   "Confirm the CLI name   │
│ ✗ migrate    claude/sonnet  failed     4m00s │    is `todo`?"            │
│ ↺ cli-parse  claude/haiku   bounced    0m52s │                           │
├──────────────────────────────────────────────┴───────────────────────────┤
│ [↑↓] navigate inbox   [enter] answer   [a] add task   [l] logs   [q] quit │
└───────────────────────────────────────────────────────────────────────────┘
```

Job glyphs (`●` running, `◍` finishing, `✓` merged, `✗` failed, `↺` bounced, `·` queued)
and running-first ordering carry over from `progress.sh`. The `❓` counter and inbox panel
are new.

**Answering** (`enter` on a selected question) drops an input area over the footer:

```
│ answering db-schema — "Postgres or SQLite for storage?"                   │
│ > SQLite, single-file, no external service____________________            │
│ [enter] submit   [esc] cancel                                             │
```

**Adding a task** (`a`) uses the same input affordance:

```
│ add task (sent to planner):                                               │
│ > also support a `--due` date flag and show overdue items in red_________ │
│ [enter] submit   [esc] cancel                                             │
```

**Standby** (after DONE) flips the status bar and idles until the user acts:

```
┌ agentloop ──────────────────────────────── ✓ DONE · standby · gate pass ─┐
│ All items complete and the gate passes. Waiting for input.               │
│ [a] add task   [q] quit                                                   │
```

Exact widths/colors are settled in implementation.

## Error handling

- **Agent timeout** → 124 semantics preserved; item treated as not-done, bounced per
  existing attempt logic.
- **Invalid `backlog.json` from planner** → re-prompt once (as today), then surface a hard
  error if still invalid.
- **Merge conflict / no-commit "done"** → bounce item to `ready` with a note; never
  auto-resolve (matches current behavior).
- **Malformed `questions/<id>.json`** → log and treat as a normal non-done result (item
  bounced), so a bad question file can't wedge the loop.
- **TUI/orchestrator channel disconnect** → orchestrator continues headless-style and
  shuts down agents cleanly; TUI exit restores the terminal (raw mode/alt-screen guard).
- **Caps** (`max_iterations`, budget, stalls) → unchanged stop conditions, except blocked-on-
  user does not count as a stall.

## Testing

- **Unit:** config parse/role-resolve, state mutations + `ready_items` deps logic, argv
  construction, question/answer file round-trip.
- **Integration (fake agent):** port every offline scenario from `tests/` and assert
  identical outcomes (merged counts, statuses, stall/cap stops). Parity gate for Phase 1.
- **New scenarios:** a fake agent that emits `needs_input` → item goes `blocked`, an injected
  answer re-dispatches with the Q&A block and completes; an injected `AddTask` request in
  standby re-engages the planner and produces new items.
- **Live smoke:** keep an opt-in end-to-end build test (real CLIs), as today.

## Phasing / milestones

1. **Phase 1** — headless parity port; ported test suite green.
2. **Phase 2** — ratatui progress panel + question inbox; new needs_input scenarios green.
3. **Phase 3** — add-task through planner + standby lifecycle; re-engagement scenarios green.
