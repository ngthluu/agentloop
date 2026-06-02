# TUI entry screen, persistent input, and enhanced planner — design

Date: 2026-06-02

## Summary

Three changes to agentloop:

1. **Goal-entry screen.** Interactive launches open on a goal-entry view instead of
   immediately running the loop. The user types (or edits the existing) goal and presses
   Continue; nothing runs until then.
2. **Persistent wrapping bottom input.** Replace the two modal inputs (answer / add-task)
   with a single always-present, text-wrapping input bar at the bottom, like Claude Code.
3. **Enhanced planner, trimmed roles.** Remove the hollow `architect`, `fix`, and `trivial`
   roles. The planner absorbs technical design: it maintains a `design.md` and emits a
   dependency-aware task graph of `build` items. Routing keeps `planner`, `build`,
   `resolver`.

## Motivation

- The CLI required (or accepted) a goal argument and started the loop immediately. There is
  no chance to review or compose the goal interactively before tokens are spent.
- Today there are two separate modal inputs (`Mode::Answering`, `Mode::AddingTask`), each a
  single non-wrapping line. Long answers/tasks are awkward and the modal switching is clumsy.
- `architect`, `fix`, and `trivial` are **routing labels with no distinct behavior** — every
  non-planner role receives the identical generic `worker_prompt`; only the spawned
  tool/model differs. `architect` in particular implied design work that never happened. The
  planner is the only agent with real, distinct behavior, so design ownership belongs there.

## Section 1 — Goal-entry screen

### Behavior

- A new TUI view `View::GoalEntry` is the **initial** view on every interactive (TTY) launch,
  including resumes. It is pre-filled with the current `.agentloop/state/goal.md` text (empty
  on a fresh workspace).
- Nothing runs — no planner, no workers — until the user commits the goal.
- Layout: a centered title/prompt ("Describe what to build — or edit the existing goal:"),
  a bordered multiline input pre-filled with the existing goal, and a `[ Continue ]` button
  below it.
- Keys on this view:
  - printable chars → type into the input
  - `Shift+Enter` (or `Alt+Enter`) → insert a newline (text wraps)
  - `Enter`, or `Tab` to focus `[ Continue ]` then `Enter` → commit and start
  - `Ctrl-C` → quit without running (the always-available quit; `q` types a literal `q` into
    the pre-filled input, so it is not a quit key here — consistent with Section 2)
- Committing with empty text is allowed only when a prior goal exists (plain resume). On a
  fresh workspace with empty input, Continue is a no-op (stay on the screen) until text is
  entered.

### Wiring

- The TTY path must defer the orchestrator until the goal is committed. Add a new command
  `Command::StartRun { goal: String }`.
- `app.rs` / `cli.rs`: instead of calling `orchestrator::run` immediately for the TTY path,
  start the TUI in `GoalEntry`. When the TUI emits `StartRun`, fold the goal text through the
  existing `fold_rerun_goal` / `bootstrap_workspace` path, then launch the orchestrator and
  switch the TUI to its normal `View::List`.
- Headless / non-TTY and `--dry-run` paths are unchanged (they have no entry screen; they use
  the goal arg / persisted goal as today).
- A goal passed on the CLI still works: it pre-fills the entry input rather than bypassing the
  screen.

## Section 2 — Persistent wrapping bottom input

### Behavior

- Remove the `Mode` enum (`Mode::Normal` / `Mode::Answering` / `Mode::AddingTask`) and the
  `is_editing` / `mode_is_adding` / `mode_is_answering` helpers. The input is always live.
- A single `input: String` is always editable. A one-line label above the input shows the
  current submission target, derived from selection state:
  - **`Answering <id>`** when the Inbox pane is focused and a question is selected.
  - **`Add task`** otherwise (Jobs focused, or empty inbox).
- Keys in the normal list view:
  - printable chars → append to `input`
  - `Tab` → switch focus between Jobs and Inbox
  - `↑` / `↓` → move the selection within the focused pane
  - `Enter` → submit `input`; route to `Command::AnswerQuestion { item_id, text }` when the
    target is a selected Inbox question, else `Command::AddTask { request: text }`. Clear the
    input after submit. Submitting empty input is a no-op.
  - `Shift+Enter` (or `Alt+Enter`) → insert a newline; text wraps
  - `Esc` → clear the input
  - `q` → quit (only when the input is empty; otherwise `q` types a literal `q`). Quit when
    input is non-empty is reachable via `Ctrl-C`.
  - `Backspace` → delete the last char
- Job-detail view (`View::JobDetail`) keeps its own keys (`Esc` back, `↑`/`↓` scroll the log),
  and the bottom input bar stays rendered beneath it. While in job-detail, printable keys
  still type into the input so a task/answer can be composed without leaving the log.

### Rendering

- The input is a `Paragraph` with `.wrap(Wrap { trim: false })` inside a bordered block at the
  bottom. The footer region height is computed from the wrapped line count of the current
  input (plus the label line and a hint line), capped at a maximum (e.g. 8 lines); beyond the
  cap the input scrolls to keep the cursor/end visible.
- The status/hint line shows the relevant keys (`[enter] submit  [shift+enter] newline
  [tab] switch pane  [↑↓] navigate  [esc] clear  [q] quit`).

### Quit affordance

Because `q` now types into the input, the canonical always-available quit is `Ctrl-C`. `q`
quits only when the input is empty. This is documented in the README and the footer hint.

## Section 3 — Enhanced planner, trimmed roles

### Config changes (`templates/config.yaml`)

Remove `architect`, `fix`, and `trivial`. Final routing:

```yaml
routing:
  planner:  { tool: claude, model: opus,   effort: high,   flags: "--dangerously-skip-permissions" }
  build:    { tool: codex,  model: gpt-5.5, effort: high,   flags: "--dangerously-bypass-approvals-and-sandbox" }
  resolver: { tool: claude, model: sonnet, effort: medium, flags: "--dangerously-skip-permissions" }

defaults: { role: build }
```

### Planner behavior (`planner.rs`)

The planner becomes the single owner of both technical design and the backlog. The
`planner_prompt` is updated so that each round the planner:

1. Writes/maintains `.agentloop/state/design.md` — the technical solution: chosen stack,
   architecture/structure, key decisions and constraints. On the first round it authors it;
   later rounds keep it current as the design evolves.
2. Continues to maintain `master.md` (status board) and `backlog.json` (machine state) as today.
3. Emits a **dependency-aware task graph**: every work item is tagged `role: build` with
   realistic `deps` (ids of items that must finish first) and a concrete `acceptance` string,
   so build workers run in the correct order. The role list in the prompt is reduced to
   `build` (the planner no longer assigns `architect`/`fix`/`trivial`). `resolver` is spawned
   automatically by the orchestrator on merge conflicts and is not planner-assigned.

The output contract (overwrite `backlog.json`, rewrite `master.md`) is unchanged except that
the role enumeration in the prompt narrows to `build`, and the prompt instructs the planner to
also write `design.md`.

### Worker behavior (`worker.rs`)

`worker_prompt` gains a reference to `.agentloop/state/design.md` so build workers implement
against the documented design. The prompt includes the design content (or instructs the worker
to read the file) as context, in addition to the per-item task. No behavioral change to the
resolver prompt.

### Non-goals

- No separate architect agent, no `architect.rs`, no extra orchestrator phase or gate. The
  planner runs as it does today; only its prompt/outputs are richer.
- No change to the merge-conflict resolver flow.

## Affected files

- `src/cli.rs` — defer orchestrator start for TTY; goal arg pre-fills entry screen.
- `src/app.rs` — start TUI in goal-entry; handle `StartRun` to launch the orchestrator.
- `src/events.rs` — add `Command::StartRun { goal }`.
- `src/tui.rs` — `View::GoalEntry`; remove `Mode`; persistent wrapping input; key routing;
  render goal screen + bottom input bar.
- `src/planner.rs` — enhanced planner prompt (design.md + dependency-aware build graph).
- `src/worker.rs` — worker prompt references `design.md`.
- `templates/config.yaml` — remove `architect`/`fix`/`trivial`.
- `README.md` — document the entry screen, the persistent input + key model (incl. `Ctrl-C`
  to quit), and the trimmed role set.
- `tests/` — update any tests asserting the old modal input, removed roles, or immediate
  start.

## Testing

- TUI unit tests for `on_key`: typing accumulates into `input`; `Tab`/`↑`/`↓` navigate without
  consuming printable input; `Enter` routes to `AnswerQuestion` when an Inbox question is
  selected and `AddTask` otherwise; `Shift+Enter` adds a newline; `Esc` clears; `q` quits only
  when input is empty.
- Goal-entry: committing emits `StartRun` with the typed goal; empty commit on fresh workspace
  is a no-op; pre-fill reflects existing `goal.md`.
- Planner: offline test (fake agent / scripted stub) asserts the planner is prompted to write
  `design.md` and that produced items use `role: build`; backlog validation still passes.
- Existing offline integration suite continues to pass with the trimmed role set.
