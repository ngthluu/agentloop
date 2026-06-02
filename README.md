# agentloop

Autonomous app builder. Give it one goal; it plans a backlog, spawns `claude`/`codex`
workers in parallel git worktrees, integrates their work, runs a planner-authored
`verify.sh` gate, and loops until the app works or a safety cap trips.

## Requirements

Rust (edition 2021), git, and the `claude` and/or `codex` CLIs on PATH.

## Build

```bash
cargo build --release
```

## Usage

```bash
./target/release/agentloop "<goal>" --workspace ./app
```

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
  successful workers are merged back sequentially (conflicts bounce the item back to the
  planner — never auto-resolved).

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
  events.rs        Reporter trait + EventLineReporter (stderr event lines)
  orchestrator.rs  iteration loop, parallel dispatch, integration, termination
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
