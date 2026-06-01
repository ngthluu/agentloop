# agentloop

Autonomous app builder. Give it one goal; it plans a backlog, spawns `claude`/`codex`
workers in parallel git worktrees, integrates their work, runs a planner-authored
`verify.sh` gate, and loops until the app works or a safety cap trips.

## Requirements

bash, git, jq, python3 + PyYAML, and the `claude` and `codex` CLIs on PATH.

## Usage

```bash
./agentloop.sh "Build a Python CLI todo app with a passing pytest suite" --workspace ./todo
```

Options: `--config <path>`, `--fresh`, `--max-iterations N`, `--dry-run`.

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
agentloop.sh            entrypoint: arg parse, bootstrap, drive the loop
helpers/yaml2json.py    YAML -> JSON (PyYAML)
lib/config.sh           config parsing + role resolution
lib/state.sh            backlog.json validate/query/mutate (atomic writes)
lib/spawn.sh            timeout + claude/codex command building (+ fake-agent hook)
lib/worktree.sh         worktree create/merge/cleanup
lib/planner.sh          planner prompt + invoke + validate
lib/worker.sh           worker prompt + dispatch
lib/loop.sh             iteration loop, parallel dispatch, integration, termination
templates/              default config.yaml + master.md
tests/                  offline suite (fake agent) + opt-in live smoke test
```

## Tests

```bash
bash tests/run.sh          # offline, uses a fake agent (no tokens spent)
bash tests/smoke_live.sh   # opt-in: real CLIs, builds a tiny app end-to-end
```
