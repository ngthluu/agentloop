# agentloop

Autonomous app builder. Give it one goal; it plans a backlog, spawns `claude`/`codex`
workers in parallel git worktrees, integrates their work, runs a planner-authored
`verify.sh` gate, and loops until the app works or a safety cap trips.

When run in a terminal it shows a live TUI: a goal-entry screen lets you confirm or
edit the goal before anything runs, then a progress panel, an inbox for answering
questions that agents raise, and a persistent input bar for adding tasks. When the
goal is done it stays alive in standby so you can keep adding tasks. Piped/non-TTY
runs fall back to plain event-line output and exit on completion.

## Requirements

Rust (edition 2021), git, and the `claude` and/or `codex` CLIs on PATH.

## Build

```bash
cargo build --release
```

## Usage

```bash
./target/release/agentloop "<goal>" --workspace ./app   # pre-fills goal-entry screen with <goal>
./target/release/agentloop --workspace ./app             # resume: entry screen pre-filled from goal.md
./target/release/agentloop --workspace ./new-dir         # fresh dir: entry screen is empty
```

Every interactive (terminal) launch opens on a **goal-entry screen** first. The screen
is pre-filled with the existing goal from `<workspace>/.agentloop/state/goal.md`, or
empty on a fresh workspace. Nothing runs — no planner, no workers — until you
type/edit the goal and press `enter` (the "[ Continue ]" action). `Ctrl-C` at the
entry screen quits without running. A goal passed as the CLI argument pre-fills the
entry screen rather than starting immediately.

Headless/piped (non-TTY) runs are unchanged: they use the goal arg or persisted goal
and run directly without the entry screen.

Options:

- `--config <path>` — config.yaml path (default: `<workspace>/.agentloop/config.yaml`)
- `--fresh` — wipe existing `.agentloop` state and start over
- `--max-iterations N` — override `caps.max_iterations` from config
- `--dry-run` — run the planner once and print the planned backlog; do not dispatch workers

## How it works

- **State:** `.agentloop/state/master.md` (human-readable status board) + `backlog.json`
  (machine state). The planner rewrites both each iteration. The planner also writes and
  maintains `.agentloop/state/design.md` (the technical solution design); `build` workers
  implement against `design.md`.
- **Routing:** edit `.agentloop/config.yaml` to map each role to a tool/model/effort/flags.
  The available roles are `planner`, `build`, and `resolver`. The planner owns the
  technical design and emits a dependency-aware backlog of `build` items; the loop
  spawns each item according to its role.
- **Gate:** the planner writes `.agentloop/verify.sh`; the loop runs it as the acceptance
  check. This is what lets agentloop target any kind of software.
- **Caps:** `max_iterations`, `max_parallel`, `item_timeout_sec`, `total_budget_sec`,
  `max_attempts`. The loop also stops on a no-progress stall.
- **Parallelism:** independent ready items run concurrently, each in its own git worktree;
  successful workers are merged back sequentially.
- **Merge conflicts:** when a worker's branch conflicts on merge, agentloop spawns a
  dedicated **resolver** agent (config role `resolver`) in the workspace to resolve the
  conflict and complete the merge, instead of bouncing the item. The resolver is unbounded
  (no attempt cap, no timeout) but is killed when you quit, so it never orphans. If it
  cannot resolve, the merge is aborted and the item bounces back to the planner.
- **Question inbox:** a worker that needs a decision only you can make writes
  `.agentloop/questions/<id>.json` and reports `status:"needs_input"`. The item is parked
  as `blocked` and surfaced in the TUI inbox. Your answer is stored in
  `.agentloop/answers/<id>.json` and the item is re-dispatched with the prior Q&A appended
  to its prompt.
- **Add tasks any time:** an add-task request is appended to `.agentloop/state/requests.jsonl`;
  the planner folds it into the backlog on its next round (you feed intent — the planner
  stays the sole owner of the backlog).
- **Standby:** on completion (or a cap/stall) the interactive run idles in standby instead
  of exiting; adding a task or answering a question re-engages it with a fresh budget window.
- **Re-run = more context:** re-running with new goal text (without `--fresh`) appends it
  to `goal.md` and queues it as a pending request, so the planner folds it into the backlog
  as new tasks and the loop re-engages — instead of reporting an instant "Done, nothing changed."

## Interactive mode (TUI)

Running in a terminal opens a full-screen panel. A persistent, text-wrapping input bar
sits at the bottom of the screen at all times (similar to Claude Code's input). Keys:

- Printable keys always type into the persistent bottom input bar; it wraps long text
  automatically. `shift+enter` (or `alt+enter`) inserts a newline.
- `enter` — submits the input. When the Inbox pane is focused and a question is
  selected, the input text is used as the answer to that question; otherwise the text
  is added as a new task to the planner. A label above the input shows the current
  target: "Answering \<id\>" or "Add task". When the input is empty, `enter` on the
  Jobs pane opens the selected job's detail view (live log tail + a real-time working
  timer).
- `tab` — switch focus between the Jobs and Inbox panes (the focused pane is highlighted)
- `↑`/`↓` — navigate the focused pane (jobs or the question inbox), or scroll the log
  in the job-detail view
- `esc` — clear the input bar, or leave the job-detail view
- `q` — quit (only when the input bar is empty); `Ctrl-C` always quits

The status bar shows the goal, current iteration, gate state, open-item count, a
pending-questions counter, and a live `⏱` total-run-time readout; `✓ DONE · standby`
appears when the run is idle and waiting. (Headless runs print the total elapsed time on
exit.)

The main panel stacks the Jobs pane on top and the Inbox pane below (full width each).

## Layout

```
src/
  main.rs          binary entry point
  cli.rs           arg parsing (clap), workspace bootstrap, dry-run wiring
  config.rs        Config / Caps / Role deserialization + helpers
  state.rs         backlog.json validate / query / mutate (atomic writes)
  spawn.rs         timeout + claude/codex command building (+ fake-agent hook)
  worktree.rs      worktree create / merge / cleanup
  planner.rs       planner prompt + invoke + validate
  worker.rs        worker prompt + dispatch
  events.rs        Reporter trait, Event/Command enums, stderr + channel reporters
  inbox.rs         question/answer file IO + prior-Q&A prompt block
  requests.rs      pending user-request log (requests.jsonl) + planner prompt block
  orchestrator.rs  iteration loop, dispatch, integration, termination, standby machine
  tui.rs           ratatui view-model (events -> state, keys -> commands) + render
  app.rs           wires orchestrator + TUI over channels; TTY vs headless dispatch
  bin/fake_agent.rs  offline stub used by tests
templates/
  config.yaml      embedded default config (include_str!)
  master.md        embedded default master status board
tests/             offline integration suite (fake_agent, scripted stub, no tokens)
```

## Tests

```bash
cargo test          # offline; uses the in-crate fake_agent + scripted stub, no tokens spent
```
