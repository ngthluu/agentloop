# agentloop

Autonomous app builder. Give it one goal; it plans a backlog, spawns `claude`/`codex`
workers in parallel git worktrees, integrates their work, runs a planner-authored
`verify.sh` gate, and loops until the app works or a safety cap trips.

When run in a terminal it shows a live TUI: a progress panel, an inbox for answering
questions that agents raise, and an add-task prompt. When the goal is done it stays
alive in standby so you can keep adding tasks. Piped/non-TTY runs fall back to plain
event-line output and exit on completion.

## Requirements

Rust (edition 2021), git, and the `claude` and/or `codex` CLIs on PATH.

## Build

```bash
cargo build --release
```

## Usage

```bash
./target/release/agentloop "<goal>" --workspace ./app   # first run, with a goal
./target/release/agentloop --workspace ./app             # resume an existing workspace
./target/release/agentloop --workspace ./new-dir         # fresh dir: starts in standby; press [a] to add a task
```

The goal argument is optional. When omitted, agentloop reads the goal from
`<workspace>/.agentloop/state/goal.md` and resumes; if there is no prior goal it starts
in standby waiting for you to add a task.

Options:

- `--config <path>` — config.yaml path (default: `<workspace>/.agentloop/config.yaml`)
- `--fresh` — wipe existing `.agentloop` state and start over
- `--max-iterations N` — override `caps.max_iterations` from config
- `--dry-run` — run the planner once and print the planned backlog; do not dispatch workers

## How it works

- **State:** `.agentloop/state/master.md` (human-readable status board) + `backlog.json`
  (machine state). The planner rewrites both each iteration.
- **Routing:** edit `.agentloop/config.yaml` to map each role to a tool/model/effort/flags.
  The planner tags every backlog item with a role; the loop spawns it accordingly.
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

Running in a terminal opens a full-screen panel. Keys:

- `tab` — switch focus between the Jobs and Inbox panes (the focused pane is highlighted)
- `↑`/`↓` — navigate the focused pane (jobs or the question inbox)
- `enter` — on the Jobs pane: open the selected job's detail (live log tail + a real-time
  working timer); on the Inbox: answer the selected question (type, `enter` submit, `esc` cancel)
- `esc` — leave the job-detail view
- `a` — add a task (type a natural-language request, `enter` to submit)
- `q` — quit

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
