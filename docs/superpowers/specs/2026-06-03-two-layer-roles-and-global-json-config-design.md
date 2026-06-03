# Two-layer roles and global JSON config - design

## Context

agentloop currently has one primary planning role named `planner`. The planner owns
both business decomposition and technical design, writes `design.md`, emits build
items directly into `.agentloop/state/backlog.json`, and the orchestrator dispatches
those build items in parallel worktrees.

The new model separates business ownership from technical execution:

- `manager` replaces `planner`.
- `architect` creates a technical design for one business task and decomposes it
  into builder subitems.
- `builder` implements one architect-authored subitem.
- `customer` acts as a silly customer and approves a business task only by its
  acceptance criteria.

Configuration also moves from a workspace-local YAML file to a global JSON file.

## Goals

- Remove the workspace-local `templates/config.yaml` and stop creating
  `.agentloop/config.yaml`.
- Use a global `config.json` by default, with `--config <path>` still overriding it.
- Remove user-configurable role flags. The spawn layer always injects the required
  unsafe permission flag for the selected tool.
- Rename planner responsibility to `manager`, and keep manager limited to business
  task management.
- Add a per-business-task `architect` phase that writes design and builder subitems.
- Add a `customer` approval gate per business task.
- Declare the whole goal done only when all business tasks are customer-approved and
  the global verification gate passes.

## Non-goals

- No settings TUI or settings command in this change. The app only resolves,
  creates, and reads the global JSON config.
- No silent YAML compatibility. Passing an old YAML config should fail with a clear
  message that config must be JSON.
- No manager-authored technical implementation tasks. That remains architect-owned.

## Role Model

The role flow is:

```text
manager -> architect -> builders -> customer -> manager
```

`manager` owns `.agentloop/state/backlog.json`. It manages business tasks only:
title, business description, dependencies, status, attempts, and acceptance
criteria. It reads user requests, previous task results, gate output, and customer
feedback, then updates business task state and `.agentloop/state/master.md`.

`architect` runs for one business task when that task has no valid technical plan.
It writes task-local technical state under `.agentloop/state/tasks/<task-id>/`:
`design.md` and `builders.json`.

`builder` runs for one subitem in `builders.json`. It receives the parent business
task, task-local technical design, the subitem, and prior Q&A. It commits focused
changes in its worktree and writes a result file.

`customer` runs after all builder subitems for a business task are done and
`verify.sh` passes. It evaluates only the parent business task's acceptance
criteria. It writes an approval or rejection result.

`resolver` remains an orchestrator-spawned merge conflict role. It is not assigned
by manager or architect.

## State Files

Business backlog:

```text
.agentloop/state/backlog.json
```

Shape:

```json
{
  "items": [
    {
      "id": "task-1",
      "title": "Checkout flow",
      "desc": "Business-level task description",
      "deps": [],
      "status": "ready",
      "attempts": 0,
      "acceptance": "Customer can complete checkout with card payment"
    }
  ]
}
```

Business task statuses:

- `ready`: manager says the task can enter or re-enter the architect/build/customer
  pipeline when dependencies are done.
- `in_progress`: the orchestrator is actively designing, building, or reviewing the
  task.
- `blocked`: the task is waiting on user input or manager redesign.
- `done`: the task has latest customer approval.
- `failed`: the task exceeded caps or hit an unrecoverable error.

Task-local technical state:

```text
.agentloop/state/tasks/<task-id>/design.md
.agentloop/state/tasks/<task-id>/builders.json
.agentloop/state/tasks/<task-id>/customer.json
```

`builders.json` shape:

```json
{
  "items": [
    {
      "id": "task-1-b1",
      "title": "Add checkout UI",
      "desc": "Implementation-level task",
      "deps": [],
      "status": "ready",
      "attempts": 0,
      "acceptance": "Checkout form renders and validates required fields"
    }
  ]
}
```

Builder subitem statuses mirror the current worker statuses:
`ready`, `in_progress`, `blocked`, `done`, and `failed`.

`customer.json` shape:

```json
{
  "status": "approved",
  "summary": "Checkout AC is satisfied"
}
```

or:

```json
{
  "status": "rejected",
  "summary": "Card payment path is missing",
  "missing_acceptance": [
    "Customer can complete checkout with card payment"
  ]
}
```

Result files are namespaced:

- `.agentloop/results/<task-id>-architect.json`
- `.agentloop/results/<builder-id>.json`
- `.agentloop/results/<task-id>-customer.json`

## Orchestrator Flow

Each iteration:

1. Run `manager`.
   It reads business backlog, pending user requests, previous results, gate output,
   and task-local customer feedback. It updates `backlog.json` and `master.md`.

2. Select dispatchable business tasks.
   A business task is dispatchable when it is `ready`, has no pending user question,
   and all business dependencies are `done`.

3. For each selected business task without a valid task-local plan, run `architect`.
   Architect writes `design.md` and `builders.json`. If the plan is invalid, the
   business task is returned to `ready` with notes for manager.

4. Dispatch ready builder subitems from all active business tasks, capped by
   `caps.max_parallel`.
   Each subitem runs in a worktree and is merged back like current workers.
   Resolver behavior for merge conflicts remains unchanged.

5. When every builder subitem for a business task is `done`, run `verify.sh`.
   If the gate fails, leave the task available for manager/architect repair.

6. If the gate passes, run `customer` for that business task.
   Customer writes `customer.json` and a customer result file.

7. If customer approves, mark the business task `done`.

8. If customer rejects, mark the business task `ready` with the rejection summary as
   notes. The next manager run sees the feedback and can revise the business task or
   force a new architect plan.

The whole run is done only when:

- global `verify.sh` passes,
- every business backlog item is `done`,
- every done business task has latest `customer.json.status == "approved"`.

## Prompt Contracts

Manager prompt:

- Identifies the role as `MANAGER`.
- States that manager owns business tasks only.
- Forbids writing technical designs or builder subitems.
- Requires `backlog.json` and `master.md` output.
- Omits `role` for new business tasks. Legacy business-task role fields may be
  tolerated while reading old state, but builder roles are architect-owned.

Architect prompt:

- Receives one parent business task and current workspace context.
- Writes `.agentloop/state/tasks/<task-id>/design.md`.
- Writes `.agentloop/state/tasks/<task-id>/builders.json`.
- Produces builder subitems with concrete acceptance criteria and dependencies.
- Does not edit app source code.

Builder prompt:

- Receives parent business task, task-local design, and one builder subitem.
- Implements exactly that builder subitem.
- Writes `.agentloop/results/<builder-id>.json`.

Customer prompt:

- Receives parent business task, its acceptance criteria, relevant summaries, and
  latest gate output.
- Acts like a silly customer: ignores internal implementation details and judges
  only the acceptance criteria.
- Writes approval or rejection JSON.

## Global JSON Config

Default config path resolution:

1. `--config <path>` when supplied.
2. `$AGENTLOOP_CONFIG` when set.
3. Platform config directory:
   - macOS: `~/Library/Application Support/agentloop/config.json`
   - Linux and other Unix: `$XDG_CONFIG_HOME/agentloop/config.json`, or
     `~/.config/agentloop/config.json`

If the selected default global path does not exist, agentloop creates it with
defaults before loading. If `--config` points to a missing file, fail clearly rather
than silently creating an arbitrary user-specified file.

Default config:

```json
{
  "caps": {
    "max_iterations": 25,
    "max_parallel": 3,
    "item_timeout_sec": 1200,
    "total_budget_sec": 21600,
    "max_attempts": 3
  },
  "routing": {
    "manager": { "tool": "claude", "model": "opus", "effort": "high" },
    "architect": { "tool": "claude", "model": "opus", "effort": "high" },
    "builder": { "tool": "codex", "model": "gpt-5.5", "effort": "high" },
    "customer": { "tool": "claude", "model": "sonnet", "effort": "medium" },
    "resolver": { "tool": "claude", "model": "sonnet", "effort": "medium" }
  },
  "defaults": { "role": "builder" }
}
```

Role config no longer has `flags`.

Spawn behavior:

- For `claude`, always pass `--dangerously-skip-permissions`.
- For `codex`, always pass `--yolo`.
- Continue to pass model and effort using existing per-tool conventions.

If a config file is not valid JSON, loading fails with a migration-oriented error
such as: `config must be JSON; migrate config.yaml to config.json`.

## Migration And Docs

- Delete `templates/config.yaml`.
- Remove `include_str!("../templates/config.yaml")`.
- Stop creating `.agentloop/config.yaml` during bootstrap.
- Update CLI help from `config.yaml` to `config.json`.
- Update README routing docs from workspace-local YAML to global JSON.
- Update layout docs to remove `templates/config.yaml`.
- Keep `.agentloop/state/master.md`, `goal.md`, `backlog.json`, requests,
  questions, answers, logs, results, and worktrees workspace-local.

## Testing

Focused tests should cover:

- Default global config path creation and JSON loading.
- `--config` override loading JSON and failing clearly on YAML.
- `flags` removed from config parsing expectations.
- spawn argv injects `--dangerously-skip-permissions` for `claude`.
- spawn argv injects `--yolo` for `codex`.
- manager prompt contains the business-only contract and no technical-design
  ownership.
- architect prompt writes task-local `design.md` and `builders.json`.
- builder prompt includes parent business task and task-local design.
- customer prompt requires AC-only approval/rejection JSON.
- valid `builders.json` detection.
- customer approval marks a business task done.
- customer rejection returns the business task to manager with feedback.
- final completion requires approved customer state for every done business task.

## Risks

- This is a larger orchestrator change than a role rename. Keeping business backlog
  and builder subitems in separate files is what preserves the manager boundary.
- Re-running architect too often would waste tokens. The orchestrator should reuse an
  existing valid task-local plan until manager/customer feedback requires a new one.
- The customer role can only be as good as the acceptance criteria. Manager prompts
  must require concrete AC for each business task.
