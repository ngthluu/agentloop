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

**Platforms: macOS and Linux only.** agentloop manages agent processes with POSIX
process groups and signals; it does not build on Windows (a clear `compile_error!`
says so). Use WSL on Windows.

## Security model — read this first

agentloop is built to run **unattended**, which means it deliberately removes the
safety prompts you may be used to:

- Every agent is spawned with permission checks disabled (`claude
  --dangerously-skip-permissions`, `codex --yolo`). Agents can run any shell
  command, edit any file your user can, and access the network.
- `.agentloop/verify.sh` is executed via `bash` on every iteration. It is
  arbitrary code living inside the workspace — anything that can write to the
  workspace (including the agents themselves) controls what it does.
- Task descriptions, designs, and notes written by one agent are fed into the
  prompts of others. agentloop sanitizes the *identifiers* (branch/path safety)
  and bounds the *sizes*, but it cannot make prompt content trustworthy.

Therefore: **only point agentloop at goals and workspaces you trust, with
credentials you accept being exercised autonomously.** For anything else, run it
inside a container or VM with scoped credentials. A run can also spend real API
credits for hours; set `caps.total_budget_sec` / `caps.max_iterations`
accordingly and watch the first runs of a new goal.

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
- `--fresh` — wipe existing `.agentloop` state and start over. Prompts for
  confirmation (it deletes all run state, logs, and results); pass `--yes` to
  skip the prompt in scripts. The existing goal is preserved unless you pass a
  new one.
- `--yes` — skip confirmation prompts (required for `--fresh` when not on a TTY)
- `--max-iterations N` — override `caps.max_iterations` from config
- `--dry-run` — run the manager once and print the business backlog; do not dispatch builders
- `--report` — print the bounce/failure troubleshooting report for the workspace and exit

One run per workspace: agentloop holds an advisory lock on
`.agentloop/state/.lock`; a second concurrent run on the same workspace exits
with an error instead of corrupting shared state.

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
  agentloop can target any kind of software. The manager owns the script; its
  contract is build + the project's test suite, nothing more. A business task's
  acceptance run passes that task id as `$1` (so one task's flaky check can't
  fail — and force a redesign of — an unrelated task); the per-iteration DONE
  gate runs with no arguments. Scripts may ignore `$1` and always run everything.
  After the scoped gate passes for a business task, the customer approves or
  rejects that task by its acceptance criteria.
  The gate runs in its own process group with a wall-clock cap (default 30 min,
  override with `AGENTLOOP_GATE_TIMEOUT_SECS`) so a hung verify.sh can never
  hang the loop; a timeout reads as a gate failure (rc 124).
- **Caps:** `max_iterations`, `max_parallel`, `item_timeout_sec`, `total_budget_sec`,
  `max_attempts`, and `max_redesigns` (whole-task re-plans; deliberately separate
  from and higher than the per-builder attempt cap, so a handful of flaky gate
  runs can't fail a task outright). The loop also stops on a no-progress stall: two consecutive
  iterations that merge nothing **and** change no loop-relevant state (gate verdict,
  backlog/builder statuses and attempts). Iterations where the manager re-scopes a
  failed task or a builder consumes an attempt count as progress — those are
  cap-bounded, so they can't keep the loop alive forever.
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
  of exiting; adding a task re-engages it with a fresh budget window. The status bar
  shows *why* it parked (done / stall / max_iterations / budget, with open and failed
  counts).
- **Failed tasks hold the run open:** a task that exhausts its redesign cap is marked
  `failed`, but the run is only DONE when the gate passes and **no open or failed**
  items remain. Every failed item — including leaves nothing depends on — is listed
  in the manager prompt with its failure note, and the manager is required to
  reshape it into new tasks or drop it, instead of the loop silently ending over
  (or grinding forever against) abandoned work.
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
live `⏱` total-run-time readout. When the run parks, the banner says why:
`✓ DONE · standby` only when everything is done and the gate passes, otherwise
`⏸ standby: <reason>` (stall / max_iterations / budget, with open and failed counts).
The gate itself appears as a `gate` job row while `verify.sh` runs, so a long verify
never looks like a dead loop. (Headless runs print the total elapsed time on exit.)
Long goals are ellipsized so the counters and timer always stay visible.

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
