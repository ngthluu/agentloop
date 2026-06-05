# agentloop

Autonomous app builder. Give it one goal; a manager maintains a business backlog,
architects write per-task technical designs, builders implement independent subitems
in parallel git worktrees, `verify.sh` checks software behavior, and a silly
customer approves each business task against its acceptance criteria before the loop
calls it done.

When run in a terminal it shows a live TUI: a goal-entry screen lets you confirm or
edit the goal before anything runs, then a jobs panel and a persistent input bar for
adding tasks. Questions agents raise are answered automatically ("you decide"), so
the loop never waits on you. When the goal is done it stays alive in standby so you
can keep adding tasks. Piped/non-TTY runs fall back to plain event-line output and
exit on completion.

## Install

Prebuilt binaries (macOS arm64/x86_64, Linux x86_64):

```bash
curl -fsSL https://raw.githubusercontent.com/ngthluu/agentloop/main/scripts/install.sh | bash
```

This installs the `agentloop` binary to `~/.local/bin` (override with
`AGENTLOOP_INSTALL_DIR=/usr/local/bin`). Ensure the install dir is on your `PATH`.
The `claude` and/or `codex` CLIs must also be on `PATH` at runtime.

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
empty on a fresh workspace. Nothing runs until you
type/edit the goal and press `enter` (the "[ Continue ]" action). `Ctrl-C` at the
entry screen quits without running. A goal passed as the CLI argument pre-fills the
entry screen rather than starting immediately.

Headless/piped (non-TTY) runs are unchanged: they use the goal arg or persisted goal
and run directly without the entry screen.

Options:

- `--config <path>` — `config.json` path. By default agentloop uses
  `$AGENTLOOP_CONFIG` when set, otherwise `~/.agentloop/config.json`.
- `--fresh` — wipe existing `.agentloop` state and start over
- `--max-iterations N` — override `caps.max_iterations` from config
- `--dry-run` — run the manager once and print the business backlog; do not dispatch builders

## How it works

- **State:** `.agentloop/state/backlog.json` is the manager-owned business backlog,
  with `.agentloop/state/master.md` as the human-readable status board. Per-task
  technical state lives under `.agentloop/state/tasks/<task-id>/`, including the
  architect's `design.md`, builder subitems, and customer approval state.
- **Routing:** global `~/.agentloop/config.json` maps roles to tool/model/effort.
  The available roles are `manager`, `architect`, `builder`, `customer`, and
  `resolver`. Tool permission switches are fixed by agentloop: `claude` always gets
  `--dangerously-skip-permissions`, and `codex` always gets `--yolo`.
- **Gate and customer:** `.agentloop/verify.sh` still gates software behavior, so
  agentloop can target any kind of software. After the gate passes for a business
  task, the customer approves or rejects that task by its acceptance criteria.
- **Caps:** `max_iterations`, `max_parallel`, `item_timeout_sec`, `total_budget_sec`,
  `max_attempts`. The loop also stops on a no-progress stall.
- **Parallelism:** independent ready items run concurrently, each in its own git worktree;
  successful builders are merged back sequentially.
- **Merge conflicts:** when a builder's branch conflicts on merge, agentloop spawns a
  dedicated **resolver** agent (config role `resolver`) in the workspace to resolve the
  conflict and complete the merge, instead of bouncing the item. The resolver is unbounded
  (no attempt cap, no timeout) but is killed when you quit, so it never orphans. If it
  cannot resolve, the merge is aborted and the item is returned for manager repair.
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
- **Add tasks any time:** an add-task request is appended to `.agentloop/state/requests.jsonl`;
  the manager folds it into the business backlog on its next round (you feed intent;
  the manager stays the sole owner of the backlog).
- **Standby:** on completion (or a cap/stall) the interactive run idles in standby instead
  of exiting; adding a task re-engages it with a fresh budget window.
- **Re-run = more context:** re-running with new goal text (without `--fresh`) appends it
  to `goal.md` and queues it as a pending request, so the manager folds it into the
  business backlog as new tasks and the loop re-engages instead of reporting an instant
  "Done, nothing changed."

## Interactive mode (TUI)

Running in a terminal opens a full-screen panel. A persistent, text-wrapping input bar
sits at the bottom of the screen at all times (similar to Claude Code's input). Keys:

- Printable keys always type into the persistent bottom input bar; it wraps long text
  automatically. `shift+enter` (or `alt+enter`) inserts a newline.
- `enter` — submits the input as a new task for the manager. When the input is empty,
  `enter` opens the selected job's detail view (live log tail + a real-time working
  timer).
- `↑`/`↓` — navigate the jobs list, or scroll the log in the job-detail view
- `esc` — clear the input bar, or leave the job-detail view
- `q` — quit (only when the input bar is empty); `Ctrl-C` always quits

The status bar shows the goal, current iteration, gate state, open-item count, and a
live `⏱` total-run-time readout; `✓ DONE · standby` appears when the run is idle and
waiting. (Headless runs print the total elapsed time on exit.) Long goals are ellipsized
so the counters and timer always stay visible.

The main panel is the Jobs list (full width).

## Troubleshooting

Nothing the loop produces is deleted:

- `.agentloop/state/events.jsonl` — append-only history of every dispatch and
  status transition (bounced/failed/merged/approved/rejected/redesign) with its
  reason. `agentloop --report --workspace <dir>` prints all bounced and failed
  cases plus what is currently failed in the backlog and builder plans.
- `.agentloop/logs/iter-N/` — per-iteration agent logs, plus each builder's
  archived result JSON (timestamp-prefixed).
- `.agentloop/logs/gate.log` — every verify.sh run (timestamp, rc, full output);
  `state/last_gate.txt` keeps just the latest.
- `.agentloop/state/tasks/<id>/archive/` — superseded builder plans, designs,
  customer reviews, and redesign counters.
- `.agentloop/logs/answered-<id>-<ts>.json` — consumed agent questions.

These files and archive dirs grow without bound by design (they are the audit
trail); prune them manually — or start over with `--fresh` — if a long-lived
workspace gets too big.

Before running, agentloop verifies that every CLI tool the config routes roles
to (claude/codex) is installed, and exits with install instructions otherwise.

## Layout

```
src/
  main.rs          binary entry point
  cli.rs           arg parsing (clap), workspace bootstrap, dry-run wiring
  config.rs        Config / Caps / Role deserialization + helpers
  state.rs         backlog.json validate / query / mutate (atomic writes)
  spawn.rs         timeout + claude/codex command building (+ fake-agent hook)
  worktree.rs      worktree create / merge / cleanup
  manager.rs       business backlog prompt + invoke + validate
  architect.rs     per-business-task design + builder plan prompt
  worker.rs        builder and resolver prompts + dispatch
  customer.rs      acceptance-criteria approval prompt + validation
  task_state.rs    task-local design/builders/customer state helpers
  events.rs        Reporter trait, Event/Command enums, stderr + channel reporters
  history.rs       append-only state/events.jsonl, artifact archiving, --report
  preflight.rs     startup check that configured agent CLIs are installed
  inbox.rs         question/answer file IO + prior-Q&A prompt block (auto-answered)
  limits.rs        usage/rate-limit detection + auto-continue wait math
  requests.rs      pending user-request log (requests.jsonl) + manager prompt block
  orchestrator.rs  iteration loop, dispatch, integration, termination, standby machine
  tui.rs           ratatui view-model (events -> state, keys -> commands) + render
  app.rs           wires orchestrator + TUI over channels; TTY vs headless dispatch
  bin/fake_agent.rs  offline stub used by tests
templates/
  master.md        embedded default master status board
tests/             offline integration suite (fake_agent, scripted stub, no tokens)
```

## Releasing (CD)

Releases are cut by pushing to the `production` branch:

1. Bump `version` in `Cargo.toml` (e.g. `0.1.0` -> `0.1.1`) and merge to `main`.
2. Fast-forward/merge `main` into `production` and push:

   ```bash
   git push origin main:production
   ```

3. The `release` workflow reads `version` from `Cargo.toml`, creates and pushes
   the tag `v{version}`, builds `agentloop` for each supported target, and
   publishes a GitHub Release with the `agentloop-<target>.tar.gz` assets.

If the tag `v{version}` already exists, the workflow no-ops — bump the version to
cut a new release. `install.sh` always fetches the **latest** release.

## Tests

```bash
cargo test          # offline; uses the in-crate fake_agent + scripted stub, no tokens spent
```
