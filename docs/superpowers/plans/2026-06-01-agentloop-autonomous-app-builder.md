# agentloop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a bash orchestrator that takes one goal prompt and drives a planner→parallel-workers→integrate→verify loop spawning `claude`/`codex`, until a gate command passes or a safety cap trips.

**Architecture:** Bash handles only control flow. All structured data lives in JSON (queried with `jq`) and config in YAML (converted to JSON with a PyYAML helper). The planner agent owns product decisions and rewrites `backlog.json` + `master.md`; the orchestrator dispatches workers in git worktrees, merges sequentially, runs a planner-authored `verify.sh` gate, and enforces caps.

**Tech Stack:** bash 3.2 (no associative arrays), `jq` 1.7, `python3`+PyYAML 6, `git` worktrees, headless `claude -p` / `codex exec`. Tests use a hand-rolled assert harness (no bats) and a fake-agent stub.

---

## Conventions (read once)

- `AGENTLOOP_HOME` = directory containing `agentloop.sh`. All libs are sourced relative to it.
- Every lib file starts with no shebang (it is sourced), uses `set -u` guards via callers, and defines only functions.
- All state mutations use the **temp-file + `mv`** pattern so a crash never leaves half-written JSON.
- Run all commands from the repo root unless stated. Commit after every task with the exact message shown.

## File Structure

```
agentloop.sh                 # entrypoint: arg parse, bootstrap, main loop driver
helpers/yaml2json.py         # YAML file -> JSON (PyYAML)
lib/config.sh                # YAML->JSON, role field resolution with defaults
lib/spawn.sh                 # run_with_timeout, agent_run (claude/codex), FAKE_AGENT hook
lib/state.sh                 # backlog.json validate/query/mutate, master.md writes
lib/worktree.sh              # worktree create / merge / cleanup
lib/planner.sh               # build planner prompt, invoke, validate output
lib/worker.sh                # build worker prompt, dispatch in worktree, collect result
lib/loop.sh                  # one_iteration + termination + no-progress detector
templates/config.yaml        # default Moderate-profile config
templates/master.md          # initial master doc
tests/lib.sh                 # assert helpers + temp workspace
tests/run.sh                 # discover + run all test_*.sh, aggregate
tests/fake_agent.sh          # stub agent for FAKE_AGENT=1
tests/test_config.sh
tests/test_spawn.sh
tests/test_state.sh
tests/test_worktree.sh
tests/test_loop.sh
tests/smoke_live.sh          # opt-in real end-to-end
```

---

## Task 0: Repo scaffold + test harness

**Files:**
- Create: `tests/lib.sh`, `tests/run.sh`

- [ ] **Step 1: Write the assert harness** — Create `tests/lib.sh`:

```bash
# Sourced by every test_*.sh. Provides assertions + temp workspace helpers.
TESTS_RUN=0
TESTS_FAIL=0

assert_eq() { # actual expected msg
  TESTS_RUN=$((TESTS_RUN+1))
  if [ "$1" != "$2" ]; then
    echo "  FAIL: $3: expected [$2] got [$1]"; TESTS_FAIL=$((TESTS_FAIL+1))
  fi
}
assert_contains() { # haystack needle msg
  TESTS_RUN=$((TESTS_RUN+1))
  case "$1" in
    *"$2"*) ;;
    *) echo "  FAIL: $3: [$1] missing [$2]"; TESTS_FAIL=$((TESTS_FAIL+1));;
  esac
}
assert_ok()   { TESTS_RUN=$((TESTS_RUN+1)); [ "$1" -eq 0 ] || { echo "  FAIL: $2: exit $1 != 0"; TESTS_FAIL=$((TESTS_FAIL+1)); }; }
assert_fail() { TESTS_RUN=$((TESTS_RUN+1)); [ "$1" -ne 0 ] || { echo "  FAIL: $2: expected non-zero"; TESTS_FAIL=$((TESTS_FAIL+1)); }; }

test_summary() { echo "  ran $TESTS_RUN, failed $TESTS_FAIL"; [ "$TESTS_FAIL" -eq 0 ]; }
mktmpws() { mktemp -d "${TMPDIR:-/tmp}/agentloop.XXXXXX"; }
```

- [ ] **Step 2: Write the runner** — Create `tests/run.sh`:

```bash
#!/bin/bash
# Runs each tests/test_*.sh in its own process; aggregates pass/fail.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
fails=0
for t in "$HERE"/test_*.sh; do
  [ -e "$t" ] || continue
  echo "== $(basename "$t") =="
  if /bin/bash "$t"; then :; else fails=$((fails+1)); fi
done
echo "================"
if [ "$fails" -eq 0 ]; then echo "ALL SUITES PASSED"; else echo "$fails SUITE(S) FAILED"; exit 1; fi
```

- [ ] **Step 3: Verify the harness runs with zero suites**

Run: `/bin/bash tests/run.sh`
Expected: prints `================` then `ALL SUITES PASSED` (no `test_*.sh` yet).

- [ ] **Step 4: Commit**

```bash
chmod +x tests/run.sh
git add tests/lib.sh tests/run.sh
git commit -m "test: add bash assert harness and runner"
```

---

## Task 1: YAML→JSON helper + config resolution

**Files:**
- Create: `helpers/yaml2json.py`, `lib/config.sh`, `tests/test_config.sh`

- [ ] **Step 1: Write the failing test** — Create `tests/test_config.sh`:

```bash
#!/bin/bash
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
. "$HERE/lib.sh"
. "$ROOT/lib/config.sh"

ws="$(mktmpws)"; trap 'rm -rf "$ws"' EXIT
cat > "$ws/c.yaml" <<'YAML'
caps: { max_parallel: 3 }
routing:
  build: { tool: codex, model: gpt-5, effort: high, flags: "--x" }
  fix:   { tool: claude, model: sonnet, effort: medium, flags: "" }
defaults: { role: build }
YAML

json="$(config_to_json "$ws/c.yaml")"
assert_contains "$json" '"max_parallel"' "yaml converts to json"

assert_eq "$(config_role_field "$json" build tool)"   "codex"  "build.tool"
assert_eq "$(config_role_field "$json" build effort)" "high"   "build.effort"
assert_eq "$(config_role_field "$json" fix model)"    "sonnet" "fix.model"
# unknown role falls back to defaults.role
assert_eq "$(config_resolve_role "$json" zzz)"        "build"  "unknown role -> default"
assert_eq "$(config_resolve_role "$json" fix)"        "fix"    "known role kept"

test_summary
```

- [ ] **Step 2: Run it to verify it fails**

Run: `/bin/bash tests/test_config.sh`
Expected: FAIL — `config_to_json: command not found` (function undefined).

- [ ] **Step 3: Write the helper** — Create `helpers/yaml2json.py`:

```python
#!/usr/bin/env python3
import sys, json, yaml
with open(sys.argv[1]) as f:
    json.dump(yaml.safe_load(f), sys.stdout)
```

- [ ] **Step 4: Write the config lib** — Create `lib/config.sh`:

```bash
# Config helpers. AGENTLOOP_HOME must be set by the caller.
config_to_json() { # yaml_path -> json on stdout
  python3 "$AGENTLOOP_HOME/helpers/yaml2json.py" "$1"
}

# Echo a role's field, or empty if absent.
config_role_field() { # config_json role field
  printf '%s' "$1" | jq -r --arg r "$2" --arg f "$3" '.routing[$r][$f] // empty'
}

# Echo the role to actually use: the role if present in routing, else defaults.role.
config_resolve_role() { # config_json role
  local present
  present="$(printf '%s' "$1" | jq -r --arg r "$2" '.routing | has($r)')"
  if [ "$present" = "true" ]; then printf '%s' "$2"
  else printf '%s' "$1" | jq -r '.defaults.role'
  fi
}

config_cap() { # config_json cap_key
  printf '%s' "$1" | jq -r --arg k "$2" '.caps[$k] // empty'
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `/bin/bash tests/test_config.sh`
Expected: `ran 6, failed 0`, suite exits 0.

- [ ] **Step 6: Commit**

```bash
git add helpers/yaml2json.py lib/config.sh tests/test_config.sh
git commit -m "feat: config yaml->json and role resolution"
```

---

## Task 2: backlog.json state (validate, query, mutate)

**Files:**
- Create: `lib/state.sh`, `tests/test_state.sh`

- [ ] **Step 1: Write the failing test** — Create `tests/test_state.sh`:

```bash
#!/bin/bash
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
. "$HERE/lib.sh"
. "$ROOT/lib/state.sh"

ws="$(mktmpws)"; trap 'rm -rf "$ws"' EXIT
bk="$ws/backlog.json"
cat > "$bk" <<'JSON'
{ "items": [
  {"id":"it-1","status":"done","deps":[]},
  {"id":"it-2","status":"ready","deps":["it-1"]},
  {"id":"it-3","status":"ready","deps":["it-2"]},
  {"id":"it-4","status":"ready","deps":[]}
]}
JSON

state_backlog_valid "$bk"; assert_ok $? "valid backlog passes"
echo 'not json' > "$ws/bad.json"
state_backlog_valid "$ws/bad.json"; assert_fail $? "bad backlog rejected"

# ready = deps all done; it-2 (dep it-1 done) and it-4 (no deps); NOT it-3 (dep it-2 ready)
ready="$(state_ready_items "$bk" 10 | tr '\n' ' ')"
assert_eq "$ready" "it-2 it-4 " "ready items respect deps"

# max_parallel limits count
ready2="$(state_ready_items "$bk" 1 | tr '\n' ' ')"
assert_eq "$ready2" "it-2 " "ready items honor max_parallel"

# open count = ready+in_progress+blocked = it-2,it-3,it-4 = 3
assert_eq "$(state_open_count "$bk")" "3" "open count"

state_set_status "$bk" it-2 done
assert_eq "$(jq -r '.items[]|select(.id=="it-2").status' "$bk")" "done" "set status"

state_increment_attempts "$bk" it-3
assert_eq "$(jq -r '.items[]|select(.id=="it-3").attempts' "$bk")" "1" "increment attempts"

test_summary
```

- [ ] **Step 2: Run it to verify it fails**

Run: `/bin/bash tests/test_state.sh`
Expected: FAIL — `state_backlog_valid: command not found`.

- [ ] **Step 3: Write the state lib** — Create `lib/state.sh`:

```bash
# backlog.json read/mutate helpers. All writes are atomic (temp + mv).
state_backlog_valid() { # path -> exit 0 if valid
  jq -e 'has("items") and (.items|type=="array")' "$1" >/dev/null 2>&1
}

# Emit ids of ready items whose deps are all done, capped at max_parallel.
state_ready_items() { # path max_parallel
  jq -r '
    ([.items[] | select(.status=="done") | .id]) as $done
    | .items[]
    | select(.status=="ready")
    | select(all((.deps // [])[]; . as $d | ($done | index($d)) != null))
    | .id
  ' "$1" | head -n "$2"
}

state_open_count() { # path
  jq '[.items[] | select(.status=="ready" or .status=="in_progress" or .status=="blocked")] | length' "$1"
}

state_set_status() { # path id status [note]
  local tmp; tmp="$(mktemp)"
  jq --arg id "$2" --arg st "$3" --arg note "${4:-}" '
    .items |= map(if .id==$id then .status=$st | (if $note!="" then .notes=$note else . end) else . end)
  ' "$1" > "$tmp" && mv "$tmp" "$1"
}

state_increment_attempts() { # path id
  local tmp; tmp="$(mktemp)"
  jq --arg id "$2" '.items |= map(if .id==$id then .attempts=((.attempts//0)+1) else . end)' "$1" > "$tmp" && mv "$tmp" "$1"
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `/bin/bash tests/test_state.sh`
Expected: `ran 9, failed 0`, suite exits 0.

- [ ] **Step 5: Commit**

```bash
git add lib/state.sh tests/test_state.sh
git commit -m "feat: backlog.json state validate/query/mutate"
```

---

## Task 3: Spawn layer — timeout + agent_run + fake agent

**Files:**
- Create: `lib/spawn.sh`, `tests/fake_agent.sh`, `tests/test_spawn.sh`

- [ ] **Step 1: Write the fake agent** — Create `tests/fake_agent.sh`:

```bash
#!/bin/bash
# Stand-in for claude/codex when FAKE_AGENT=1. Behavior driven by env:
#   FAKE_SLEEP  - seconds to sleep before exiting (default 0)
#   FAKE_EXIT   - exit code (default 0)
# Echoes its argv so tests can assert command construction.
echo "FAKE_ARGS: $*"
sleep "${FAKE_SLEEP:-0}"
exit "${FAKE_EXIT:-0}"
```

- [ ] **Step 2: Write the failing test** — Create `tests/test_spawn.sh`:

```bash
#!/bin/bash
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
export AGENTLOOP_HOME="$ROOT"
. "$HERE/lib.sh"
. "$ROOT/lib/spawn.sh"

ws="$(mktmpws)"; trap 'rm -rf "$ws"' EXIT
export FAKE_AGENT=1
export FAKE_AGENT_BIN="$HERE/fake_agent.sh"

# fast command succeeds within timeout
log="$ws/a.log"
run_with_timeout 5 "$log" /bin/bash -c 'echo hi; exit 0'
assert_ok $? "fast command ok"
assert_contains "$(cat "$log")" "hi" "log captured"

# slow command is killed and returns 124
log2="$ws/b.log"
run_with_timeout 1 "$log2" /bin/bash -c 'sleep 10'
assert_eq "$?" "124" "timeout returns 124"

# agent_run builds a claude command line (fake)
cfg='{"routing":{"build":{"tool":"claude","model":"sonnet","effort":"high","flags":"--dangerously-skip-permissions"}},"defaults":{"role":"build"}}'
log3="$ws/c.log"
agent_run "$cfg" build "do the thing" "$ws" "$log3" 30
assert_ok $? "agent_run ok"
out="$(cat "$log3")"
assert_contains "$out" "FAKE_ARGS:" "fake agent invoked"
assert_contains "$out" "claude" "tool name passed"
assert_contains "$out" "--model sonnet" "model flag"
assert_contains "$out" "--effort high" "effort flag"

# codex path uses exec + -m
cfg2='{"routing":{"build":{"tool":"codex","model":"gpt-5","effort":"high","flags":"--dangerously-bypass-approvals-and-sandbox"}},"defaults":{"role":"build"}}'
log4="$ws/d.log"
agent_run "$cfg2" build "do" "$ws" "$log4" 30
assert_contains "$(cat "$log4")" "exec" "codex exec subcommand"
assert_contains "$(cat "$log4")" "-m gpt-5" "codex model flag"

test_summary
```

- [ ] **Step 3: Run it to verify it fails**

Run: `/bin/bash tests/test_spawn.sh`
Expected: FAIL — `run_with_timeout: command not found`.

- [ ] **Step 4: Write the spawn lib** — Create `lib/spawn.sh`:

```bash
# Spawning + timeout. No timeout(1) on this platform, so we background + poll + kill.
. "$AGENTLOOP_HOME/lib/config.sh"

# Run a command with a wall-clock cap. Returns command's exit code, or 124 on timeout.
run_with_timeout() { # timeout_sec logfile -- cmd args...
  local t="$1" log="$2"; shift 2
  ( "$@" ) >"$log" 2>&1 &
  local pid=$!
  local waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge "$t" ]; then
      kill "$pid" 2>/dev/null
      sleep 1
      kill -9 "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      return 124
    fi
    sleep 1
    waited=$((waited+1))
  done
  wait "$pid"
}

# Resolve a role and run the matching CLI (or fake) in cwd, capped by timeout.
agent_run() { # config_json role prompt cwd logfile timeout_sec
  local cfg="$1" role rrole tool model effort flags prompt="$3" cwd="$4" log="$5" t="$6"
  rrole="$(config_resolve_role "$cfg" "$2")"
  tool="$(config_role_field "$cfg" "$rrole" tool)"
  model="$(config_role_field "$cfg" "$rrole" model)"
  effort="$(config_role_field "$cfg" "$rrole" effort)"
  flags="$(config_role_field "$cfg" "$rrole" flags)"

  # Build argv as an indexed array (bash 3.2 supports these).
  local argv=()
  if [ "${FAKE_AGENT:-0}" = "1" ]; then
    argv=( "${FAKE_AGENT_BIN}" "$tool" )
  elif [ "$tool" = "claude" ]; then
    argv=( claude -p "$prompt" )
  elif [ "$tool" = "codex" ]; then
    argv=( codex exec "$prompt" )
  else
    echo "agent_run: unknown tool [$tool]" >&2; return 2
  fi

  [ -n "$model" ]  && { [ "$tool" = "codex" ] && argv+=( -m "$model" ) || argv+=( --model "$model" ); }
  [ -n "$effort" ] && { [ "$tool" = "codex" ] && argv+=( -c "model_reasoning_effort=$effort" ) || argv+=( --effort "$effort" ); }
  # shellcheck disable=SC2206
  [ -n "$flags" ] && argv+=( $flags )

  ( cd "$cwd" && run_with_timeout "$t" "$log" "${argv[@]}" )
}
```

Note: when `FAKE_AGENT=1`, real flags are still appended so tests can assert on `--model`/`-m`. The fake binary just echoes argv and exits.

- [ ] **Step 5: Run the test to verify it passes**

Run: `chmod +x tests/fake_agent.sh && /bin/bash tests/test_spawn.sh`
Expected: `ran 10, failed 0`.

- [ ] **Step 6: Commit**

```bash
git add lib/spawn.sh tests/fake_agent.sh tests/test_spawn.sh
git commit -m "feat: spawn layer with bg-kill timeout and claude/codex command building"
```

---

## Task 4: Worktree create / merge / cleanup

**Files:**
- Create: `lib/worktree.sh`, `tests/test_worktree.sh`

- [ ] **Step 1: Write the failing test** — Create `tests/test_worktree.sh`:

```bash
#!/bin/bash
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
. "$HERE/lib.sh"
. "$ROOT/lib/worktree.sh"

ws="$(mktmpws)"; trap 'rm -rf "$ws"' EXIT
repo="$ws/repo"; mkdir -p "$repo"
git -C "$repo" init -q
git -C "$repo" config user.email t@t; git -C "$repo" config user.name t
echo base > "$repo/file.txt"
git -C "$repo" add -A && git -C "$repo" commit -qm init

# create a worktree, make a change, merge it back cleanly
wt="$ws/wt-1"
wt_create "$repo" "item/it-1" "$wt"; assert_ok $? "worktree created"
echo "feature" > "$wt/new.txt"
git -C "$wt" add -A && git -C "$wt" commit -qm "add new"
wt_merge "$repo" "item/it-1"; assert_ok $? "clean merge ok"
assert_eq "$(cat "$repo/new.txt")" "feature" "merged file present"
wt_remove "$repo" "$wt" "item/it-1"; assert_ok $? "worktree removed"

# conflicting change must fail and leave repo clean (merge aborted)
wt2="$ws/wt-2"
wt_create "$repo" "item/it-2" "$wt2"
echo "theirs" > "$wt2/file.txt"; git -C "$wt2" add -A && git -C "$wt2" commit -qm theirs
echo "ours" > "$repo/file.txt"; git -C "$repo" add -A && git -C "$repo" commit -qm ours
wt_merge "$repo" "item/it-2"; assert_fail $? "conflicting merge fails"
# repo must not be mid-merge
[ -f "$repo/.git/MERGE_HEAD" ]; assert_fail $? "merge was aborted (no MERGE_HEAD)"

test_summary
```

- [ ] **Step 2: Run it to verify it fails**

Run: `/bin/bash tests/test_worktree.sh`
Expected: FAIL — `wt_create: command not found`.

- [ ] **Step 3: Write the worktree lib** — Create `lib/worktree.sh`:

```bash
# git worktree helpers for parallel workers.
wt_create() { # repo branch path
  git -C "$1" worktree add -q -b "$2" "$3" HEAD
}

# Merge branch into repo's current branch. On conflict, abort and return non-zero.
wt_merge() { # repo branch
  if git -C "$1" merge --no-edit -q "$2"; then
    return 0
  else
    git -C "$1" merge --abort 2>/dev/null
    return 1
  fi
}

wt_remove() { # repo path branch
  git -C "$1" worktree remove --force "$2" 2>/dev/null
  git -C "$1" branch -D "$3" 2>/dev/null
  return 0
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `/bin/bash tests/test_worktree.sh`
Expected: `ran 7, failed 0`.

- [ ] **Step 5: Commit**

```bash
git add lib/worktree.sh tests/test_worktree.sh
git commit -m "feat: worktree create/merge/cleanup with conflict abort"
```

---

## Task 5: Planner + worker prompt builders and invocation

**Files:**
- Create: `lib/planner.sh`, `lib/worker.sh`

These build prompts and reuse `agent_run`. They are exercised end-to-end (with the fake agent) in Task 6's loop test, so this task adds the functions and a focused prompt-content test.

- [ ] **Step 1: Write the failing test** — Create `tests/test_prompts.sh`:

```bash
#!/bin/bash
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
export AGENTLOOP_HOME="$ROOT"
. "$HERE/lib.sh"
. "$ROOT/lib/planner.sh"
. "$ROOT/lib/worker.sh"

ws="$(mktmpws)"; trap 'rm -rf "$ws"' EXIT
mkdir -p "$ws/.agentloop/state" "$ws/.agentloop/results"
echo "Build a CLI calculator" > "$ws/.agentloop/state/goal.md"
echo "# status" > "$ws/.agentloop/state/master.md"
echo '{"items":[]}' > "$ws/.agentloop/state/backlog.json"
echo "gate: exit 0" > "$ws/gate.txt"

p="$(planner_prompt "$ws")"
assert_contains "$p" "Build a CLI calculator" "planner sees goal"
assert_contains "$p" "backlog.json" "planner told output contract"

item='{"id":"it-7","title":"add tests","desc":"write pytest","acceptance":"pytest passes"}'
w="$(worker_prompt "$ws" "$item")"
assert_contains "$w" "write pytest" "worker sees desc"
assert_contains "$w" "it-7" "worker sees id"
assert_contains "$w" "results/it-7.json" "worker told result contract"

test_summary
```

- [ ] **Step 2: Run it to verify it fails**

Run: `/bin/bash tests/test_prompts.sh`
Expected: FAIL — `planner_prompt: command not found`.

- [ ] **Step 3: Write the planner lib** — Create `lib/planner.sh`:

```bash
# Planner prompt construction + invocation. Reuses spawn + state.
. "$AGENTLOOP_HOME/lib/spawn.sh"
. "$AGENTLOOP_HOME/lib/state.sh"

planner_prompt() { # workspace
  local ws="$1" goal master backlog
  goal="$(cat "$ws/.agentloop/state/goal.md")"
  master="$(cat "$ws/.agentloop/state/master.md" 2>/dev/null)"
  backlog="$(cat "$ws/.agentloop/state/backlog.json" 2>/dev/null)"
  cat <<EOF
You are the PLANNER for an autonomous app build. Working dir: $ws (a git repo).

GOAL:
$goal

CURRENT master.md:
$master

CURRENT backlog.json:
$backlog

Your job each round:
1. Read worker results in .agentloop/results/ and the latest gate output in
   .agentloop/state/last_gate.txt (if present). Mark finished items status="done".
2. Add/split/refine items so the GOAL gets built. First round: scaffold the project
   and write an executable .agentloop/verify.sh that builds/tests the app (start simple).
3. For any item with attempts >= max, redesign or drop it instead of repeating.
4. Assign each open item a role from the config routing (planner|architect|build|fix|trivial),
   realistic deps (ids of items that must finish first), and a concrete acceptance string.

OUTPUT CONTRACT — you MUST overwrite .agentloop/state/backlog.json with valid JSON:
{"items":[{"id","title","desc","role","deps":[],"status":"ready|in_progress|done|failed|blocked","attempts":0,"acceptance"}]}
Also rewrite .agentloop/state/master.md as a human-readable status board.
Do not print the JSON to stdout; write the files.
EOF
}

# Invoke the planner agent, then validate backlog.json (re-prompt once on invalid).
planner_run() { # config_json workspace logfile timeout_sec
  local cfg="$1" ws="$2" log="$3" t="$4" bk="$2/.agentloop/state/backlog.json"
  agent_run "$cfg" planner "$(planner_prompt "$ws")" "$ws" "$log" "$t"
  if state_backlog_valid "$bk"; then return 0; fi
  echo "planner produced invalid backlog.json; re-prompting once" >&2
  agent_run "$cfg" planner "$(planner_prompt "$ws")
NOTE: your previous backlog.json was invalid JSON. Write valid JSON this time." "$ws" "$log" "$t"
  state_backlog_valid "$bk"
}
```

- [ ] **Step 4: Write the worker lib** — Create `lib/worker.sh`:

```bash
# Worker prompt construction + dispatch inside a worktree.
. "$AGENTLOOP_HOME/lib/spawn.sh"

worker_prompt() { # workspace item_json
  local ws="$1" item="$2" id title desc acc
  id="$(printf '%s' "$item" | jq -r '.id')"
  title="$(printf '%s' "$item" | jq -r '.title')"
  desc="$(printf '%s' "$item" | jq -r '.desc')"
  acc="$(printf '%s' "$item" | jq -r '.acceptance // "the change builds and tests pass"')"
  cat <<EOF
You are a WORKER on an autonomous app build. You are in a git worktree of the project.
Implement exactly this item and nothing else:

  id:    $id
  title: $title
  task:  $desc
  done when: $acc

Rules:
- Make focused commits in this worktree as you go.
- Verify your work against the acceptance criteria before finishing.
- When finished, write $ws/.agentloop/results/$id.json:
  {"status":"done|failed","summary":"one line","files_changed":["..."]}
EOF
}

# Dispatch one item: returns agent_run's exit code; result file is the source of truth.
worker_dispatch() { # config_json workspace item_json worktree logfile timeout_sec
  local cfg="$1" ws="$2" item="$3" wt="$4" log="$5" t="$6" role
  role="$(printf '%s' "$item" | jq -r '.role // "build"')"
  agent_run "$cfg" "$role" "$(worker_prompt "$ws" "$item")" "$wt" "$log" "$t"
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `/bin/bash tests/test_prompts.sh`
Expected: `ran 5, failed 0`.

- [ ] **Step 6: Commit**

```bash
git add lib/planner.sh lib/worker.sh tests/test_prompts.sh
git commit -m "feat: planner and worker prompt builders and invocation"
```

---

## Task 6: The iteration loop + termination

**Files:**
- Create: `lib/loop.sh`, `tests/test_loop.sh`

- [ ] **Step 1: Write the failing test** — Create `tests/test_loop.sh`. It uses a scripted fake agent (via `FAKE_AGENT_BIN` pointing at a per-test stub) so an entire run completes offline:

```bash
#!/bin/bash
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
export AGENTLOOP_HOME="$ROOT"
. "$HERE/lib.sh"
. "$ROOT/lib/loop.sh"

ws="$(mktmpws)"; trap 'rm -rf "$ws"' EXIT
mkdir -p "$ws/.agentloop/state" "$ws/.agentloop/results" "$ws/.agentloop/logs"
git -C "$ws" init -q; git -C "$ws" config user.email t@t; git -C "$ws" config user.name t
echo seed > "$ws/seed.txt"; git -C "$ws" add -A; git -C "$ws" commit -qm init
echo "make one file" > "$ws/.agentloop/state/goal.md"
echo "# status" > "$ws/.agentloop/state/master.md"
echo '{"items":[]}' > "$ws/.agentloop/state/backlog.json"

# Scripted fake agent: as planner, seed one ready item then mark it done on later calls.
# as worker, create a file + write its result file.
stub="$ws/stub.sh"
cat > "$stub" <<'STUB'
#!/bin/bash
tool="$1"; shift
ws_state="$WS/.agentloop/state"; res="$WS/.agentloop/results"
# Detect planner vs worker by the presence of a flag in argv we can't see here,
# so use a marker file to count planner calls.
prompt="$*"
case "$prompt" in
  *PLANNER*)
    n=$(cat "$WS/.plan_n" 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > "$WS/.plan_n"
    if [ "$n" -eq 1 ]; then
      echo '{"items":[{"id":"it-1","title":"f","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/backlog.json"
      printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$WS/.agentloop/verify.sh"; chmod +x "$WS/.agentloop/verify.sh"
    else
      # mark done if worker produced a result
      if [ -f "$res/it-1.json" ]; then
        jq '.items|=map(.status="done")' "$ws_state/backlog.json" > "$ws_state/b.tmp" && mv "$ws_state/b.tmp" "$ws_state/backlog.json"
      fi
    fi
    echo "# updated" > "$ws_state/master.md"
    ;;
  *WORKER*)
    echo made > "$PWD/made.txt"; git add -A; git commit -qm "worker" 2>/dev/null
    echo '{"status":"done","summary":"made file","files_changed":["made.txt"]}' > "$res/it-1.json"
    ;;
esac
exit 0
STUB
chmod +x "$stub"
export FAKE_AGENT=1 FAKE_AGENT_BIN="$stub" WS="$ws"

cfg='{"caps":{"max_iterations":5,"max_parallel":2,"item_timeout_sec":30,"total_budget_sec":300,"max_attempts":3},"routing":{"planner":{"tool":"claude","model":"opus","effort":"high","flags":""},"build":{"tool":"codex","model":"gpt-5","effort":"high","flags":""}},"defaults":{"role":"build"}}'

loop_run "$cfg" "$ws"
rc=$?
assert_eq "$rc" "0" "loop_run reports DONE"
assert_eq "$(cat "$ws/made.txt" 2>/dev/null)" "made" "worker output merged to main"
assert_eq "$(state_open_count "$ws/.agentloop/state/backlog.json")" "0" "no open items at end"

test_summary
```

> The stub keys off the literal words `PLANNER`/`WORKER`, which the prompt builders already emit ("You are the PLANNER…", "You are a WORKER…"). Keep those words in the prompts.

- [ ] **Step 2: Run it to verify it fails**

Run: `/bin/bash tests/test_loop.sh`
Expected: FAIL — `loop_run: command not found`.

- [ ] **Step 3: Write the loop lib** — Create `lib/loop.sh`:

```bash
# The orchestration loop. Pure control flow over the other libs.
. "$AGENTLOOP_HOME/lib/state.sh"
. "$AGENTLOOP_HOME/lib/planner.sh"
. "$AGENTLOOP_HOME/lib/worker.sh"
. "$AGENTLOOP_HOME/lib/worktree.sh"

# Run the gate; write output to last_gate.txt; return its exit code (0 if no verify.sh yet).
loop_gate() { # workspace
  local ws="$1" gate="$1/.agentloop/verify.sh" out="$1/.agentloop/state/last_gate.txt"
  if [ -x "$gate" ]; then
    ( cd "$ws" && /bin/bash "$gate" ) > "$out" 2>&1
    return $?
  fi
  echo "no verify.sh yet" > "$out"; return 1
}

# One iteration: plan, select, dispatch in parallel, integrate, gate.
loop_iterate() { # config_json workspace iter_n -> sets LOOP_DONE_ITEMS (count merged)
  local cfg="$1" ws="$2" n="$3"
  local sdir="$ws/.agentloop/state" ldir="$ws/.agentloop/logs/iter-$n"
  mkdir -p "$ldir" "$ws/.agentloop/results"
  local itimeout; itimeout="$(config_cap "$cfg" item_timeout_sec)"; : "${itimeout:=1200}"
  local maxpar; maxpar="$(config_cap "$cfg" max_parallel)"; : "${maxpar:=3}"

  planner_run "$cfg" "$ws" "$ldir/planner.log" "$itimeout" || { echo "planner failed/invalid" >&2; return 2; }

  local ids; ids="$(state_ready_items "$sdir/backlog.json" "$maxpar")"
  LOOP_DONE_ITEMS=0
  [ -z "$ids" ] && return 0

  # Dispatch each ready item in its own worktree, in parallel.
  local id item wt pids="" map=""
  for id in $ids; do
    item="$(jq -c --arg id "$id" '.items[]|select(.id==$id)' "$sdir/backlog.json")"
    wt="$ws/.agentloop/worktrees/$id"
    rm -rf "$wt"; wt_remove "$ws" "$wt" "item/$id" >/dev/null 2>&1
    wt_create "$ws" "item/$id" "$wt" || { state_set_status "$sdir/backlog.json" "$id" failed "worktree create failed"; continue; }
    state_set_status "$sdir/backlog.json" "$id" in_progress
    state_increment_attempts "$sdir/backlog.json" "$id"
    ( worker_dispatch "$cfg" "$ws" "$item" "$wt" "$ldir/item-$id.log" "$itimeout" ) &
    pids="$pids $!"
    map="$map $!:$id"
  done
  wait

  # Integrate sequentially based on each worker's result file.
  for id in $ids; do
    local rfile="$ws/.agentloop/results/$id.json" st="failed"
    if [ -f "$rfile" ] && jq -e '.status=="done"' "$rfile" >/dev/null 2>&1; then
      if wt_merge "$ws" "item/$id"; then st="done"; LOOP_DONE_ITEMS=$((LOOP_DONE_ITEMS+1));
      else st="ready"; fi
      [ "$st" = "ready" ] && state_set_status "$sdir/backlog.json" "$id" ready "merge conflict; replan"
      [ "$st" = "done" ]  && state_set_status "$sdir/backlog.json" "$id" done
    else
      state_set_status "$sdir/backlog.json" "$id" ready "worker did not report done"
    fi
    wt_remove "$ws" "$ws/.agentloop/worktrees/$id" "item/$id" >/dev/null 2>&1
    rm -f "$rfile"
  done
  return 0
}

# Drive iterations until DONE (0), cap/stall (returns 1), or hard error (2).
loop_run() { # config_json workspace
  local cfg="$1" ws="$2" sdir="$2/.agentloop/state"
  local maxit; maxit="$(config_cap "$cfg" max_iterations)"; : "${maxit:=25}"
  local budget; budget="$(config_cap "$cfg" total_budget_sec)"; : "${budget:=21600}"
  local start; start="$(date +%s)"
  local n=0 stalls=0 prev_gate="init"

  while [ "$n" -lt "$maxit" ]; do
    n=$((n+1))
    if [ $(( $(date +%s) - start )) -ge "$budget" ]; then echo "STOP: time budget exceeded" >&2; return 1; fi

    loop_iterate "$cfg" "$ws" "$n"; local irc=$?
    [ "$irc" -eq 2 ] && return 2

    loop_gate "$ws"; local grc=$?
    local gate_state="fail"; [ "$grc" -eq 0 ] && gate_state="pass"
    local open; open="$(state_open_count "$sdir/backlog.json")"

    echo "iter $n: merged=${LOOP_DONE_ITEMS:-0} gate=$gate_state open=$open" >&2

    if [ "$gate_state" = "pass" ] && [ "$open" -eq 0 ]; then echo "DONE" >&2; return 0; fi

    if [ "${LOOP_DONE_ITEMS:-0}" -eq 0 ] && [ "$gate_state" = "$prev_gate" ]; then
      stalls=$((stalls+1))
      if [ "$stalls" -ge 2 ]; then echo "STOP: no progress for 2 iterations" >&2; return 1; fi
    else
      stalls=0
    fi
    prev_gate="$gate_state"
  done
  echo "STOP: max_iterations reached" >&2
  return 1
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `/bin/bash tests/test_loop.sh`
Expected: `ran 3, failed 0`. (Planner round 1 seeds `it-1` + writes verify.sh; worker creates `made.txt`; merge succeeds; planner round 2 marks done; gate passes; open=0 → DONE.)

- [ ] **Step 5: Commit**

```bash
git add lib/loop.sh tests/test_loop.sh
git commit -m "feat: iteration loop, parallel dispatch, integration, termination"
```

---

## Task 7: Entrypoint, templates, bootstrap

**Files:**
- Create: `agentloop.sh`, `templates/config.yaml`, `templates/master.md`

- [ ] **Step 1: Write the default config template** — Create `templates/config.yaml`:

```yaml
# agentloop run config. Edit freely; re-running picks up changes.
caps:
  max_iterations: 25
  max_parallel: 3
  item_timeout_sec: 1200      # 20 min per worker
  total_budget_sec: 21600     # 6 h whole run
  max_attempts: 3

routing:                      # role -> how to spawn
  planner:   { tool: claude, model: opus,   effort: high,   flags: "--dangerously-skip-permissions" }
  architect: { tool: claude, model: opus,   effort: high,   flags: "--dangerously-skip-permissions" }
  build:     { tool: codex,  model: gpt-5,  effort: high,   flags: "--dangerously-bypass-approvals-and-sandbox" }
  fix:       { tool: claude, model: sonnet, effort: medium, flags: "--dangerously-skip-permissions" }
  trivial:   { tool: claude, model: haiku,  effort: low,    flags: "--dangerously-skip-permissions" }

defaults: { role: build }
```

- [ ] **Step 2: Write the master.md template** — Create `templates/master.md`:

```markdown
# agentloop — Master Status

**Goal:** (frozen in .agentloop/state/goal.md)

This file is rewritten by the planner each iteration. Below is the live status board.

_(no iterations yet)_
```

- [ ] **Step 3: Write the entrypoint** — Create `agentloop.sh`:

```bash
#!/bin/bash
set -u
AGENTLOOP_HOME="$(cd "$(dirname "$0")" && pwd)"
export AGENTLOOP_HOME
. "$AGENTLOOP_HOME/lib/config.sh"
. "$AGENTLOOP_HOME/lib/loop.sh"

usage() {
  cat <<EOF
Usage: agentloop.sh "<goal prompt>" [options]
  --workspace <dir>     target dir (default: current dir)
  --config <path>       config.yaml (default: <workspace>/.agentloop/config.yaml)
  --fresh               wipe existing .agentloop state and start over
  --max-iterations N    override caps.max_iterations
  --dry-run             plan only; do not dispatch workers
EOF
}

GOAL=""; WORKSPACE="$PWD"; CONFIG=""; FRESH=0; OVR_MAXIT=""; DRYRUN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --workspace) WORKSPACE="$2"; shift 2;;
    --config) CONFIG="$2"; shift 2;;
    --fresh) FRESH=1; shift;;
    --max-iterations) OVR_MAXIT="$2"; shift 2;;
    --dry-run) DRYRUN=1; shift;;
    -h|--help) usage; exit 0;;
    -*) echo "unknown option $1" >&2; usage; exit 2;;
    *) GOAL="$1"; shift;;
  esac
done
[ -n "$GOAL" ] || { echo "ERROR: goal prompt required" >&2; usage; exit 2; }

mkdir -p "$WORKSPACE"
WORKSPACE="$(cd "$WORKSPACE" && pwd)"
META="$WORKSPACE/.agentloop"
[ "$FRESH" = "1" ] && rm -rf "$META"
mkdir -p "$META/state" "$META/results" "$META/logs" "$META/worktrees"

# git repo is required for worktrees
[ -d "$WORKSPACE/.git" ] || git -C "$WORKSPACE" init -q
git -C "$WORKSPACE" config user.email >/dev/null 2>&1 || git -C "$WORKSPACE" config user.email agentloop@local
git -C "$WORKSPACE" config user.name  >/dev/null 2>&1 || git -C "$WORKSPACE" config user.name  agentloop
grep -q '^.agentloop/$' "$WORKSPACE/.gitignore" 2>/dev/null || echo '.agentloop/' >> "$WORKSPACE/.gitignore"
# ensure at least one commit exists so `worktree add HEAD` works
git -C "$WORKSPACE" rev-parse HEAD >/dev/null 2>&1 || { git -C "$WORKSPACE" add -A; git -C "$WORKSPACE" commit -qm "agentloop: initial commit"; }

[ -n "$CONFIG" ] || CONFIG="$META/config.yaml"
[ -f "$CONFIG" ] || cp "$AGENTLOOP_HOME/templates/config.yaml" "$CONFIG"
[ -f "$META/state/master.md" ] || cp "$AGENTLOOP_HOME/templates/master.md" "$META/state/master.md"
[ -f "$META/state/goal.md" ] || printf '%s\n' "$GOAL" > "$META/state/goal.md"
[ -f "$META/state/backlog.json" ] || echo '{"items":[]}' > "$META/state/backlog.json"

CFG_JSON="$(config_to_json "$CONFIG")"
[ -n "$OVR_MAXIT" ] && CFG_JSON="$(printf '%s' "$CFG_JSON" | jq --argjson v "$OVR_MAXIT" '.caps.max_iterations=$v')"

# Graceful shutdown: kill children, flush.
trap 'echo "interrupted; stopping" >&2; pkill -P $$ 2>/dev/null; exit 130' INT TERM

if [ "$DRYRUN" = "1" ]; then
  planner_run "$CFG_JSON" "$WORKSPACE" "$META/logs/dryrun-planner.log" "$(config_cap "$CFG_JSON" item_timeout_sec)"
  echo "dry-run: planned backlog ->"; jq . "$META/state/backlog.json"
  exit 0
fi

loop_run "$CFG_JSON" "$WORKSPACE"
rc=$?
echo "=== agentloop finished (rc=$rc). See $META/state/master.md ===" >&2
exit "$rc"
```

- [ ] **Step 4: Smoke-test the entrypoint with the fake agent (offline)**

Run:
```bash
chmod +x agentloop.sh
tmp="$(mktemp -d)"; cp tests/test_loop.sh /dev/null 2>/dev/null
FAKE_AGENT=1 FAKE_AGENT_BIN="$PWD/tests/fake_agent.sh" \
  ./agentloop.sh "make a file" --workspace "$tmp" --dry-run
```
Expected: bootstraps `.agentloop/`, the fake agent runs as planner, prints `dry-run: planned backlog ->` and a JSON document. (The default fake_agent.sh just echoes argv and writes nothing, so backlog stays `{"items":[]}` — that is fine; this step only proves the entrypoint wires up and exits 0.)

- [ ] **Step 5: Commit**

```bash
git add agentloop.sh templates/config.yaml templates/master.md
git commit -m "feat: agentloop entrypoint, bootstrap, and templates"
```

---

## Task 8: Full suite green + README

**Files:**
- Create: `README.md`

- [ ] **Step 1: Run the entire test suite**

Run: `/bin/bash tests/run.sh`
Expected: every `test_*.sh` prints `ran N, failed 0`; final line `ALL SUITES PASSED`.

- [ ] **Step 2: Write the README** — Create `README.md`:

```markdown
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
- State: `.agentloop/state/master.md` (human board) + `backlog.json` (machine state).
- Routing: edit `.agentloop/config.yaml` to map roles -> tool/model/effort/flags.
- Caps: `max_iterations`, `max_parallel`, `item_timeout_sec`, `total_budget_sec`, `max_attempts`.

## Tests
```bash
bash tests/run.sh          # offline, uses a fake agent
bash tests/smoke_live.sh   # opt-in: real CLIs, builds a tiny app
```
```

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: add README"
```

---

## Task 9: Live smoke test (opt-in, real CLIs)

**Files:**
- Create: `tests/smoke_live.sh`

- [ ] **Step 1: Write the live smoke test** — Create `tests/smoke_live.sh`:

```bash
#!/bin/bash
# OPT-IN: actually spends tokens. Builds a tiny real app end-to-end.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
ws="$(mktemp -d "${TMPDIR:-/tmp}/agentloop-live.XXXXXX")"
echo "workspace: $ws"
"$ROOT/agentloop.sh" \
  "Create a Python CLI 'addcli' that adds two integers from argv and prints the sum, with a pytest test that passes. Provide a verify.sh that runs pytest." \
  --workspace "$ws" --max-iterations 8
rc=$?
echo "rc=$rc"; echo "--- master.md ---"; cat "$ws/.agentloop/state/master.md"
echo "--- tree ---"; ls -R "$ws" | head -50
exit "$rc"
```

- [ ] **Step 2: Make executable + commit (do NOT auto-run in CI)**

```bash
chmod +x tests/smoke_live.sh
git add tests/smoke_live.sh
git commit -m "test: opt-in live smoke test"
```

- [ ] **Step 3: (Manual, when you're ready to spend tokens) run it**

Run: `bash tests/smoke_live.sh`
Expected: finishes with `rc=0`, `addcli` exists, `verify.sh` runs pytest green. If it stops on a cap, read `master.md` to see where it got stuck.

---

## Self-Review Notes (against the spec)

- **Spec §1 two sources of truth** → Tasks 2 (backlog.json), 5/6 (master.md rewritten by planner).
- **§3 workspace layout** → Task 7 bootstrap creates exactly this tree; `.agentloop/` git-ignored.
- **§4 loop steps 1–6** → Task 6 `loop_iterate` + `loop_run` (plan, select, parallel dispatch, sequential integrate, gate, terminate).
- **§5 backlog schema** → Task 2 + planner contract in Task 5; validated with `jq`.
- **§6 config + routing + wrappers** → Tasks 1 + 3 (`run_claude`/`run_codex` realized as `agent_run`).
- **§7 safety** → timeout (Task 3), attempt cap (Task 6 increments; planner instructed to redesign), malformed-JSON re-prompt (Task 5 `planner_run`), merge-conflict→ready (Task 6), resumability (Task 7 reuses existing state unless `--fresh`), no-progress detector (Task 6 `loop_run`), graceful Ctrl-C (Task 7 trap).
- **§8 CLI surface** → Task 7 arg parsing incl. `--dry-run`, `--fresh`, `--max-iterations`.
- **§9 testing** → fake agent (Task 3), unit/integration suites (Tasks 1–6), live smoke (Task 9).
- **Placeholder scan**: none — every code step contains complete, runnable content.
- **Naming consistency**: `agent_run`, `run_with_timeout`, `state_*`, `wt_*`, `config_*`, `planner_run`, `worker_dispatch`, `loop_iterate`, `loop_run`, `loop_gate` used identically across tasks.
