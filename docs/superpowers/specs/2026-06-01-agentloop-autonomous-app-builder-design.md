# agentloop — Autonomous App-Building Loop (Design)

**Date:** 2026-06-01
**Status:** Approved design, pre-implementation

## 1. Summary

`agentloop` is a single bash orchestrator that takes **one goal prompt** and drives a
**planner → parallel workers → integrate → verify** loop until a gate command confirms the
app works, or a safety cap trips. Output is a fully working application of any type.

All state lives on disk as plain files, so a run is **resumable** and **inspectable**. The
`claude` and `codex` CLIs are spawned as headless child processes. A **YAML config** decides
which tool/model/effort runs each work item.

The orchestrator (bash) makes **no product decisions**. It only: dispatches workers, integrates
their output, runs the gate command, and enforces caps. All product reasoning happens inside
spawned agents.

### Two sources of truth
- **`master.md`** — human-readable narrative + status board (the "master doc").
- **`backlog.json`** — machine state the loop reads/writes (items, deps, status, attempts).

The **planner agent** keeps both in sync.

## 2. Environment assumptions

Verified on the target machine (macOS, zsh):
- `claude` v2.1.159 — headless via `claude -p "<prompt>"`, supports `--model`, `--effort`
  (low|medium|high|xhigh|max), `--dangerously-skip-permissions`, `--add-dir`.
- `codex` v0.133.0 — headless via `codex exec "<prompt>"`, supports `-m/--model`,
  `-s/--sandbox`, `--dangerously-bypass-approvals-and-sandbox`.
- `jq` 1.7.1, `python3` 3.14, `node` v22, `git` 2.50 present.
- **No `yq`** → YAML parsed with an embedded python3 helper.
- **No `timeout`/`gtimeout`** → per-worker timeouts enforced via background-PID + poll + kill.

## 3. Workspace layout

```
<workspace>/                 # a git repo (enables worktrees for parallel work)
├── .agentloop/
│   ├── config.yaml          # routing table, caps, defaults
│   ├── state/
│   │   ├── goal.md          # the original prompt, frozen at first run
│   │   ├── master.md        # narrative + status board (human-readable)
│   │   └── backlog.json     # machine state (see schema §5)
│   ├── results/<id>.json    # each worker writes its structured result here
│   ├── verify.sh            # the gate command — planner creates/maintains it
│   └── logs/iter-NN/        # planner.log + item-<id>.log per iteration
└── (the generated app lives at the workspace root)
```

`.agentloop/` is git-ignored. `verify.sh` being **planner-authored** is what lets the loop
handle "any type of software": the orchestrator just runs it; the planner decides what
"verified" means for the chosen stack (build, tests, lint, etc.). On the first iteration the
planner scaffolds the project and writes an initial `verify.sh` (may start as a trivial
build/smoke check and grow stricter as the app matures).

## 4. The loop (one iteration)

1. **Plan/replan** — spawn the planner (role `planner`). Inputs: `goal.md`, `master.md`,
   `backlog.json`, last gate result, recent worker results. Outputs: rewritten `backlog.json`
   + updated `master.md`. The planner marks done items, splits/adds items, and sets each
   item's `role`, `acceptance`, and `deps`.
2. **Select** — orchestrator reads `backlog.json` and picks all items with `status=ready`
   whose `deps` are all `done`, up to `caps.max_parallel`.
3. **Dispatch (parallel)** — each selected item gets its own `git worktree`. Spawn the
   config-resolved tool/model/effort with a worker prompt (item title/desc + acceptance +
   pointer to the repo + instruction to write its result file). Workers run as background
   jobs; orchestrator `wait`s for all, enforcing `item_timeout_sec` per worker.
   Each worker MUST write `.agentloop/results/<id>.json` = `{status, summary, files_changed}`.
4. **Integrate** — merge each successful worktree back to the main branch **sequentially**.
   On merge conflict: abort that merge, set the item back to `ready` with a conflict note for
   the planner; do **not** auto-resolve.
5. **Gate** — run `.agentloop/verify.sh`; record exit code + tail of output into state.
6. **Terminate?**
   - **DONE** if gate passes AND `backlog.json` has no open items (`ready`/`in_progress`/`blocked`).
   - **STOP (report)** if `caps.max_iterations` reached or `caps.total_budget_sec` exceeded,
     or the no-progress detector trips (§7).
   - Otherwise increment iteration and loop.

## 5. `backlog.json` schema

```json
{
  "items": [
    {
      "id": "string (stable, e.g. it-001)",
      "title": "short label",
      "desc": "what to build/change",
      "role": "planner|architect|build|fix|trivial",
      "deps": ["it-000", "..."],
      "status": "ready|in_progress|done|failed|blocked",
      "attempts": 0,
      "acceptance": "how the worker knows it succeeded",
      "notes": "orchestrator/planner annotations (e.g. timeout, merge conflict)"
    }
  ]
}
```

Orchestrator validates this with `jq` after every plan step. On invalid JSON: re-prompt the
planner once; if still invalid, abort with a clear message (never loop on garbage).

## 6. Config (`config.yaml`)

Parsed with an embedded python3 helper. JSON values handled with `jq`.

```yaml
goal_file: .agentloop/state/goal.md

caps:                          # "Moderate" profile (default)
  max_iterations: 25
  max_parallel: 3
  item_timeout_sec: 1200       # 20 min per worker
  total_budget_sec: 21600      # 6 h whole run
  max_attempts: 3              # per item before planner must redesign/drop it

routing:                       # role -> how to spawn
  planner:   { tool: claude, model: opus,   effort: high,   flags: "--dangerously-skip-permissions" }
  architect: { tool: claude, model: opus,   effort: high,   flags: "--dangerously-skip-permissions" }
  build:     { tool: codex,  model: gpt-5,  effort: high,   flags: "--dangerously-bypass-approvals-and-sandbox" }
  fix:       { tool: claude, model: sonnet, effort: medium, flags: "--dangerously-skip-permissions" }
  trivial:   { tool: claude, model: haiku,  effort: low,    flags: "--dangerously-skip-permissions" }

defaults: { role: build }      # used if planner omits a role
```

Two thin spawn wrappers translate a role entry into a command line:
- `run_claude`: `claude -p "<prompt>" --model <model> --effort <effort> <flags>`
- `run_codex`:  `codex exec "<prompt>" -m <model> <flags>`

Both always set cwd to the item's worktree. Effort maps to codex via `-c model_reasoning_effort=<effort>` where applicable.

## 7. Error handling & safety

- **Full-auto but scoped:** every spawn runs with cwd pinned to the workspace/worktree;
  dangerous-skip flags live only in config so the blast radius is one folder.
- **Timeouts without `timeout(1)`:** spawn worker in background, record PID, poll against
  `item_timeout_sec`, then `kill` (escalate to `kill -9`) on overrun → item `failed(timeout)`.
- **Attempt cap:** each item tracks `attempts`; after `max_attempts` the planner is instructed
  to redesign or drop it rather than retry forever.
- **Malformed planner output:** validate `backlog.json` with `jq`; re-prompt once, else abort.
- **Merge conflicts:** never auto-resolve; item returns to `ready` with a conflict note.
- **Resumability:** all state on disk; re-running resumes from `backlog.json`. `--fresh` wipes state.
- **No-progress detector:** if an iteration completes 0 items AND the gate result is unchanged,
  increment a stall counter; after 2 stalls, stop and report.
- **Graceful Ctrl-C:** trap signals, kill child PIDs, flush `master.md`, exit non-zero.

## 8. CLI surface

```
agentloop.sh "<goal prompt>" [options]
  --workspace <dir>    target dir (default: ./ )
  --config <path>      config.yaml (default: .agentloop/config.yaml, created if absent)
  --fresh              wipe existing .agentloop state and start over
  --max-iterations N   override cap
  --dry-run            plan only; do not dispatch workers
```

First run with no `.agentloop/`: bootstrap config from a template, `git init` if needed,
freeze the goal into `goal.md`, then enter the loop.

## 9. Testing strategy

A **fake agent** stub makes the suite fast and offline: `FAKE_AGENT=1` makes `run_claude`/
`run_codex` invoke a stub script (canned result files + scripted git changes) instead of the
real CLIs.

Unit/integration tests (with fake agent):
- config parsing → command-line construction per role
- ready-item selection respecting `deps` and `max_parallel`
- timeout kill path; attempt cap; no-progress stall; `backlog.json` validation
- worktree create → merge → conflict-returns-item-to-`ready`
- termination logic (gate pass + empty backlog = DONE; cap = STOP)

One **live smoke test** against the real CLIs: a tiny goal ("a Python CLI that adds two
numbers, with a passing pytest") run end-to-end to confirm real wiring.

## 10. Out of scope (YAGNI)

- Distributed/multi-machine execution.
- A web dashboard (master.md + logs are the UI).
- Cost accounting/billing integration.
- Auto-resolving merge conflicts.
- Support for agent CLIs other than `claude` and `codex`.
