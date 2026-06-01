# Live Progress Tracking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace agentloop's static, iteration-boundary-only output with a live redrawing dashboard (on a TTY) that shows every in-flight job — planner and workers — with tool/model, a live elapsed timer, and a tail of its log; degrade to append-only event lines when output is not a TTY.

**Architecture:** A new `lib/progress.sh` owns all display logic and a small per-job state directory (`.agentloop/state/progress/`). The parent writes one TSV `*.job` file per job and replaces its blind `wait` (`lib/loop.sh:53`) with a poll-and-redraw loop. Job completion is detected via per-job `*.done` sentinel files (written by a wrapper subshell) rather than `kill -0`, which is unreliable against un-reaped child zombies on bash 3.2. All cursor control is TTY-only; non-TTY visibility comes from event lines emitted on register/status-change.

**Tech Stack:** POSIX-ish bash (targets bash 3.2 on macOS), `jq`, `git`, ANSI cursor control (`\033[<n>A`, `\033[J`), `tput`. Tests are standalone `tests/test_*.sh` scripts auto-globbed by `tests/run.sh`, sourcing `tests/lib.sh` for `assert_*` helpers.

---

## File Structure

- **Create `lib/progress.sh`** — all progress display + per-job state I/O + helpers. Sole owner of terminal output for live progress.
- **Create `tests/test_progress.sh`** — unit tests for `lib/progress.sh` (helpers, non-TTY event lines, forced-TTY render, sentinel wait). Auto-discovered by `tests/run.sh`.
- **Modify `lib/loop.sh`** — source `progress.sh`; init in `loop_run`; in `loop_iterate` track the planner as a job, register each worker before dispatch, replace `wait` with the render loop, and update job status during integration.
- **Modify `README.md`** — one line in the Layout section for `lib/progress.sh`.

State directory contract (`.agentloop/state/progress/`):
- `<id>.job` — one tab-separated line, 7 fields, in order: `id`, `label`, `tool`, `model`, `log`, `start_epoch`, `status`. `status` ∈ `running | merged | failed | bounced | done`.
- `<id>.done` — sentinel written by the spawn wrapper when the job exits; content is one line: `<exit_code> <end_epoch>`.

Glyphs: `●` running, `◍` finishing (exited, awaiting integration), `✓` merged/done, `✗` failed, `↺` bounced, `·` other. Tail line prefix: `   └ `.

---

### Task 1: Scaffold `lib/progress.sh` with the pure helpers

**Files:**
- Create: `lib/progress.sh`
- Test: `tests/test_progress.sh`

- [ ] **Step 1: Write the failing test**

Create `tests/test_progress.sh`:

```bash
#!/bin/bash
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
export AGENTLOOP_HOME="$ROOT"
. "$HERE/lib.sh"
. "$ROOT/lib/progress.sh"

ws="$(mktmpws)"; trap 'rm -rf "$ws"' EXIT

# --- progress_fmt_elapsed ---
assert_eq "$(progress_fmt_elapsed 0)"   "0m00s"  "fmt 0s"
assert_eq "$(progress_fmt_elapsed 65)"  "1m05s"  "fmt 65s"
assert_eq "$(progress_fmt_elapsed 600)" "10m00s" "fmt 600s"

# --- progress_strip_ansi ---
assert_eq "$(printf '\033[31mred\033[0m' | progress_strip_ansi)" "red" "strip ansi colors"

# --- progress_truncate ---
assert_eq "$(printf 'hello'      | progress_truncate 10)" "hello"  "no truncation when short"
assert_eq "$(printf 'helloworld' | progress_truncate 5)"  "hell…"  "truncate adds ellipsis"

# --- progress_tail_log ---
: > "$ws/empty.log"
assert_eq "$(progress_tail_log "$ws/empty.log")" "starting…" "empty log -> placeholder"
printf 'line1\n\nediting foo.py\n' > "$ws/a.log"
assert_eq "$(progress_tail_log "$ws/a.log")" "editing foo.py" "tail last non-empty line"

test_summary
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash tests/test_progress.sh`
Expected: FAIL — `progress.sh` does not exist yet, so sourcing errors / functions undefined.

- [ ] **Step 3: Write minimal implementation**

Create `lib/progress.sh`:

```bash
# Live progress display + per-job state. Sourced by loop.sh.
# All cursor control is TTY-only; non-TTY callers get append-only event lines.
PROGRESS_REFRESH=1   # seconds between redraws on a TTY

# "Xm YYs" from a whole number of seconds.
progress_fmt_elapsed() { # seconds
  printf '%dm%02ds' "$(( $1 / 60 ))" "$(( $1 % 60 ))"
}

# Strip ANSI CSI escapes from stdin.
progress_strip_ansi() {
  sed $'s/\033\\[[0-9;]*[A-Za-z]//g'
}

# Read one line from stdin; if longer than width, cut and append an ellipsis.
progress_truncate() { # width
  local w="$1" line; IFS= read -r line || true
  if [ "${#line}" -gt "$w" ]; then printf '%s' "${line:0:$((w-1))}…"; else printf '%s' "$line"; fi
}

# Last non-empty, ANSI-stripped line of a log, or a placeholder when empty.
progress_tail_log() { # logfile
  local line=""
  if [ -s "$1" ]; then
    line="$(tail -n 20 "$1" | progress_strip_ansi | grep -v '^[[:space:]]*$' | tail -n 1)"
  fi
  [ -n "$line" ] && printf '%s' "$line" || printf 'starting…'
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash tests/test_progress.sh`
Expected: PASS — `ran 9, failed 0`.

- [ ] **Step 5: Commit**

```bash
git add lib/progress.sh tests/test_progress.sh
git commit -m "feat(progress): pure display helpers (elapsed, strip-ansi, truncate, tail-log)"
```

---

### Task 2: Per-job state files + lifecycle + non-TTY event lines

**Files:**
- Modify: `lib/progress.sh`
- Test: `tests/test_progress.sh`

- [ ] **Step 1: Write the failing test**

In `tests/test_progress.sh`, insert these blocks immediately **before** the final `test_summary` line:

```bash
# --- init defaults to non-TTY under the test harness ---
progress_init
assert_eq "$PROGRESS_TTY" "0" "init detects non-TTY"

# --- register writes a job file and emits a dispatch event line (non-TTY) ---
sdir="$ws/state"; mkdir -p "$sdir"
PROGRESS_TTY=0
reg_out="$(progress_register "$sdir" it-1 "do thing" codex gpt-5.5 "$ws/a.log" 2>&1)"
assert_contains "$reg_out" "dispatch" "register emits dispatch line"
assert_contains "$reg_out" "it-1"     "register names the id"
assert_eq "$(cut -f1 "$sdir/progress/it-1.job")" "it-1"    "job file: id field"
assert_eq "$(cut -f3 "$sdir/progress/it-1.job")" "codex"   "job file: tool field"
assert_eq "$(cut -f7 "$sdir/progress/it-1.job")" "running" "job file: status field"

# --- set_status rewrites status (preserving other fields) and emits a line ---
st_out="$(progress_set_status "$sdir" it-1 merged 2>&1)"
assert_contains "$st_out" "merged" "set_status emits status line"
assert_eq "$(cut -f1 "$sdir/progress/it-1.job")" "it-1"   "set_status preserves id"
assert_eq "$(cut -f7 "$sdir/progress/it-1.job")" "merged" "set_status updates status"

# --- set_status on an unknown id is a silent no-op ---
progress_set_status "$sdir" nope merged >/dev/null 2>&1
assert_ok $? "set_status unknown id is a no-op"

# --- reset clears the progress dir ---
progress_reset "$sdir"
assert_eq "$(ls "$sdir/progress" 2>/dev/null | wc -l | tr -d ' ')" "0" "reset empties progress dir"

# --- spawn backgrounds a command and writes a sentinel with exit code + end time ---
progress_register "$sdir" w1 lbl codex gpt-5 "$ws/a.log" >/dev/null 2>&1
progress_spawn "$sdir" w1 -- /bin/bash -c 'exit 3'
wait
assert_eq "$(cut -d' ' -f1 "$sdir/progress/w1.done")" "3" "spawn sentinel records exit code"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash tests/test_progress.sh`
Expected: FAIL — `progress_init` / `progress_register` / `progress_set_status` / `progress_reset` / `progress_spawn` undefined.

- [ ] **Step 3: Write minimal implementation**

Append to `lib/progress.sh`:

```bash
# Detect once whether stderr is an interactive terminal.
progress_init() {
  if [ -t 2 ]; then PROGRESS_TTY=1; else PROGRESS_TTY=0; fi
  PROGRESS_LAST_LINES=0
}

# Path to the per-iteration job-state directory.
progress_dir() { printf '%s/progress' "$1"; }   # statedir

# Wipe and recreate the job-state dir for a fresh iteration.
progress_reset() { # statedir
  local d; d="$(progress_dir "$1")"
  rm -rf "$d"; mkdir -p "$d"
  PROGRESS_LAST_LINES=0
}

# Register a running job. Parent calls this right before backgrounding the work.
progress_register() { # statedir id label tool model log
  local d; d="$(progress_dir "$1")"; mkdir -p "$d"
  printf '%s\t%s\t%s\t%s\t%s\t%s\trunning\n' \
    "$2" "$3" "$4" "$5" "$6" "$(date +%s)" > "$d/$2.job"
  [ "${PROGRESS_TTY:-0}" = "1" ] || \
    printf '%s  dispatch %-10s %s/%s  %s\n' "$(date +%H:%M:%S)" "$2" "$4" "$5" "$3" >&2
}

# Update a job's status field (id stays the same). No-op if the job is unknown.
progress_set_status() { # statedir id status
  local d f id label tool model log start st
  d="$(progress_dir "$1")"; f="$d/$2.job"
  [ -f "$f" ] || return 0
  IFS=$'\t' read -r id label tool model log start st < "$f"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$id" "$label" "$tool" "$model" "$log" "$start" "$3" > "$f"
  [ "${PROGRESS_TTY:-0}" = "1" ] || \
    printf '%s  %-9s %-10s %s/%s\n' "$(date +%H:%M:%S)" "$3" "$2" "$tool" "$model" >&2
}

# Background a command; write a "<exit_code> <end_epoch>" sentinel when it exits.
# Sentinels (not kill -0) are how progress_wait detects completion: an un-reaped
# child becomes a zombie whose pid still answers kill -0, which would hang a poll.
progress_spawn() { # statedir id -- cmd...
  local d; d="$(progress_dir "$1")"; local id="$2"; shift 2
  [ "$1" = "--" ] && shift
  ( "$@"; printf '%s %s\n' "$?" "$(date +%s)" > "$d/$id.done" ) &
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash tests/test_progress.sh`
Expected: PASS — all assertions, `failed 0`.

- [ ] **Step 5: Commit**

```bash
git add lib/progress.sh tests/test_progress.sh
git commit -m "feat(progress): job-state files, lifecycle, sentinel spawn, non-TTY event lines"
```

---

### Task 3: Frame rendering (`progress_sort_jobs` + `progress_render`)

**Files:**
- Modify: `lib/progress.sh`
- Test: `tests/test_progress.sh`

- [ ] **Step 1: Write the failing test**

In `tests/test_progress.sh`, insert this block immediately **before** the final `test_summary` line:

```bash
# --- render (forced TTY) draws a header + a running row with its log tail ---
progress_reset "$sdir"
PROGRESS_TTY=1; PROGRESS_LAST_LINES=0
now="$(date +%s)"
# a running job started 65s ago, logging to a.log ("editing foo.py" is its last line)
printf 'it-2\tbuild api\tcodex\tgpt-5.5\t%s\t%s\trunning\n' "$ws/a.log" "$((now-65))" \
  > "$sdir/progress/it-2.job"
# a merged (terminal) job with a sentinel
printf 'it-9\tscaffold\tcodex\tgpt-5.5\t%s\t%s\tmerged\n' "$ws/a.log" "$((now-120))" \
  > "$sdir/progress/it-9.job"
printf '0 %s\n' "$((now-100))" > "$sdir/progress/it-9.done"

render_out="$(progress_render "$sdir" 3 "$((now-130))" 360 2>&1)"
assert_contains "$render_out" "iter 3"         "render: header shows iteration"
assert_contains "$render_out" "it-2"           "render: running row id"
assert_contains "$render_out" "gpt-5.5"        "render: row shows tool/model"
assert_contains "$render_out" "editing foo.py" "render: running row shows log tail"
assert_contains "$render_out" "it-9"           "render: terminal row present"

# running jobs sort before jobs that have a sentinel
sort_out="$(progress_sort_jobs "$sdir/progress" | sed 's#.*/##' | tr '\n' ' ')"
assert_eq "$sort_out" "it-2.job it-9.job " "sort_jobs: active before finished"

# --- render is a no-op when not a TTY ---
PROGRESS_TTY=0
noop_out="$(progress_render "$sdir" 3 "$((now-130))" 360 2>&1)"
assert_eq "$noop_out" "" "render: non-TTY prints nothing"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash tests/test_progress.sh`
Expected: FAIL — `progress_render` / `progress_sort_jobs` undefined.

- [ ] **Step 3: Write minimal implementation**

Append to `lib/progress.sh`:

```bash
# Emit job-file paths: active jobs (no sentinel) first, then finished ones.
progress_sort_jobs() { # progress_dir
  local jf
  for jf in "$1"/*.job; do [ -e "$jf" ] || continue; [ -f "${jf%.job}.done" ] || printf '%s\n' "$jf"; done
  for jf in "$1"/*.job; do [ -e "$jf" ] || continue; [ -f "${jf%.job}.done" ] && printf '%s\n' "$jf"; done
}

# Draw one dashboard frame to stderr. TTY-only: a no-op otherwise.
# Redraws in place by moving up the previous frame's line count and clearing down.
progress_render() { # statedir iter budget_start budget_total
  [ "${PROGRESS_TTY:-0}" = "1" ] || return 0
  local d iter bstart btot now cols out nlines act fin jf
  local id label tool model log start st glyph el endep tl row
  d="$(progress_dir "$1")"; iter="$2"; bstart="$3"; btot="$4"
  now="$(date +%s)"
  cols="$(tput cols 2>/dev/null || echo 80)"; [ "$cols" -gt 100 ] && cols=100

  act=0; fin=0
  for jf in "$d"/*.job; do [ -e "$jf" ] || continue
    if [ -f "${jf%.job}.done" ]; then fin=$((fin+1)); else act=$((act+1)); fi
  done

  out="iter $iter | elapsed $(progress_fmt_elapsed $((now-bstart)))/$(progress_fmt_elapsed "$btot") | $act running, $fin done"$'\n'
  nlines=1
  out="$out$(printf '%*s' "$cols" '' | tr ' ' '-')"$'\n'; nlines=$((nlines+1))

  while IFS= read -r jf; do
    [ -e "$jf" ] || continue
    IFS=$'\t' read -r id label tool model log start st < "$jf"
    if [ -f "${jf%.job}.done" ] && [ "$st" = "running" ]; then
      endep="$(cut -d' ' -f2 "${jf%.job}.done" 2>/dev/null)"; : "${endep:=$now}"
      el=$((endep-start)); st="finishing"; glyph='◍'
    elif [ "$st" = "running" ]; then
      el=$((now-start)); glyph='●'
    else
      el=$((now-start))
      case "$st" in merged|done) glyph='✓';; failed) glyph='✗';; bounced) glyph='↺';; *) glyph='·';; esac
    fi
    row="$(printf '%s %-8s %-16.16s %s/%s  %-9s %s' "$glyph" "$id" "$label" "$tool" "$model" "$st" "$(progress_fmt_elapsed "$el")")"
    out="$out$(printf '%s' "$row" | progress_truncate "$cols")"$'\n'; nlines=$((nlines+1))
    if [ "$glyph" = '●' ] || [ "$glyph" = '◍' ]; then
      tl="$(progress_tail_log "$log")"
      out="$out$(printf '   └ %s' "$tl" | progress_truncate "$cols")"$'\n'; nlines=$((nlines+1))
    fi
  done <<EOF
$(progress_sort_jobs "$d")
EOF

  [ "${PROGRESS_LAST_LINES:-0}" -gt 0 ] && printf '\033[%dA\033[J' "$PROGRESS_LAST_LINES" >&2
  printf '%s' "$out" >&2
  PROGRESS_LAST_LINES="$nlines"
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash tests/test_progress.sh`
Expected: PASS — `failed 0`.

- [ ] **Step 5: Commit**

```bash
git add lib/progress.sh tests/test_progress.sh
git commit -m "feat(progress): redrawing frame render with running-first sort and log tails"
```

---

### Task 4: The poll/redraw wait loop (`progress_wait`)

**Files:**
- Modify: `lib/progress.sh`
- Test: `tests/test_progress.sh`

- [ ] **Step 1: Write the failing test**

In `tests/test_progress.sh`, insert this block immediately **before** the final `test_summary` line:

```bash
# --- progress_wait returns once every tracked job has a sentinel (non-TTY) ---
progress_reset "$sdir"
PROGRESS_TTY=0
progress_register "$sdir" jw lbl codex gpt-5 "$ws/a.log" >/dev/null 2>&1
progress_spawn "$sdir" jw -- /bin/bash -c 'exit 0'
progress_wait "$sdir" 1 "$(date +%s)" 360 -- jw
assert_ok $? "progress_wait returns after the sentinel appears"
assert_eq "$(cut -d' ' -f1 "$sdir/progress/jw.done")" "0" "wait: job ran to completion"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash tests/test_progress.sh`
Expected: FAIL — `progress_wait` undefined.

- [ ] **Step 3: Write minimal implementation**

Append to `lib/progress.sh`:

```bash
# Block until every tracked id has a completion sentinel. On a TTY, redraw the
# dashboard each tick; otherwise just poll quietly (event lines already emitted).
progress_wait() { # statedir iter budget_start budget_total -- id...
  local sdir="$1" iter="$2" bstart="$3" btot="$4"; shift 4
  [ "$1" = "--" ] && shift
  local ids="$*" d id pending; d="$(progress_dir "$sdir")"
  while :; do
    pending=0
    for id in $ids; do [ -f "$d/$id.done" ] || pending=1; done
    [ "${PROGRESS_TTY:-0}" = "1" ] && progress_render "$sdir" "$iter" "$bstart" "$btot"
    [ "$pending" = "0" ] && break
    sleep "$PROGRESS_REFRESH"
  done
  wait 2>/dev/null   # reap the now-finished children
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash tests/test_progress.sh`
Expected: PASS — `failed 0`.

- [ ] **Step 5: Commit**

```bash
git add lib/progress.sh tests/test_progress.sh
git commit -m "feat(progress): sentinel-driven poll/redraw wait loop"
```

---

### Task 5: Wire progress tracking into `lib/loop.sh`

**Files:**
- Modify: `lib/loop.sh:1-7` (sourcing), `lib/loop.sh:19-75` (`loop_iterate`), `lib/loop.sh:78-110` (`loop_run`)
- Test: `tests/test_loop.sh` (existing regression — must still pass) plus a new assertion

- [ ] **Step 1: Add a regression assertion to the existing loop test**

In `tests/test_loop.sh`, immediately **before** the final `test_summary` line, add:

```bash
# progress event lines were emitted to stderr for the dispatched worker
plog="$ws/progress.out"
( loop_run "$cfg" "$ws" ) >/dev/null 2>"$plog" || true
assert_contains "$(cat "$plog")" "dispatch" "loop emits progress dispatch event lines (non-TTY)"
```

- [ ] **Step 2: Run it to verify it fails**

Run: `bash tests/test_loop.sh`
Expected: FAIL — no `dispatch` text yet (loop doesn't call progress functions). The earlier assertions in this file should still pass.

- [ ] **Step 3: Source `progress.sh` in `lib/loop.sh`**

In `lib/loop.sh`, change the sourcing block at the top (lines 1-6) from:

```bash
# The orchestration loop. Pure control flow over the other libs.
. "$AGENTLOOP_HOME/lib/config.sh"
. "$AGENTLOOP_HOME/lib/state.sh"
. "$AGENTLOOP_HOME/lib/planner.sh"
. "$AGENTLOOP_HOME/lib/worker.sh"
. "$AGENTLOOP_HOME/lib/worktree.sh"
```

to:

```bash
# The orchestration loop. Pure control flow over the other libs.
. "$AGENTLOOP_HOME/lib/config.sh"
. "$AGENTLOOP_HOME/lib/state.sh"
. "$AGENTLOOP_HOME/lib/planner.sh"
. "$AGENTLOOP_HOME/lib/worker.sh"
. "$AGENTLOOP_HOME/lib/worktree.sh"
. "$AGENTLOOP_HOME/lib/progress.sh"
```

- [ ] **Step 4: Replace `loop_iterate` with the progress-tracked version**

In `lib/loop.sh`, replace the entire `loop_iterate` function (lines 18-75) with:

```bash
# One iteration: plan, select, dispatch in parallel, integrate, gate.
loop_iterate() { # config_json workspace iter_n -> sets LOOP_DONE_ITEMS (count merged)
  local cfg="$1" ws="$2" n="$3"
  local sdir="$ws/.agentloop/state" ldir="$ws/.agentloop/logs/iter-$n"
  mkdir -p "$ldir" "$ws/.agentloop/results"
  local itimeout; itimeout="$(config_cap "$cfg" item_timeout_sec)"; : "${itimeout:=1200}"
  local maxpar; maxpar="$(config_cap "$cfg" max_parallel)"; : "${maxpar:=3}"
  local maxatt; maxatt="$(config_cap "$cfg" max_attempts)"; : "${maxatt:=3}"

  progress_reset "$sdir"

  # Track the planner as a job so its (often minute-long) run isn't a blank screen.
  local prole ptool pmodel prc
  prole="$(config_resolve_role "$cfg" planner)"
  ptool="$(config_role_field "$cfg" "$prole" tool)"
  pmodel="$(config_role_field "$cfg" "$prole" model)"
  progress_register "$sdir" planner planning "$ptool" "$pmodel" "$ldir/planner.log"
  progress_spawn "$sdir" planner -- planner_run "$cfg" "$ws" "$ldir/planner.log" "$itimeout"
  progress_wait "$sdir" "$n" "$PROGRESS_RUN_START" "$PROGRESS_RUN_BUDGET" -- planner
  prc="$(cut -d' ' -f1 "$sdir/progress/planner.done" 2>/dev/null)"
  [ "${prc:-1}" = "0" ] || { echo "planner failed/invalid" >&2; return 2; }
  progress_set_status "$sdir" planner done

  local ids; ids="$(state_ready_items "$sdir/backlog.json" "$maxpar")"
  LOOP_DONE_ITEMS=0
  [ -z "$ids" ] && { progress_render "$sdir" "$n" "$PROGRESS_RUN_START" "$PROGRESS_RUN_BUDGET"; return 0; }

  # Dispatch each ready item in its own worktree, in parallel. `dispatched` collects
  # only ids that actually got a worker, so integration never re-touches capped items.
  local id item wt att role rrole tool model dispatched=""
  for id in $ids; do
    item="$(jq -c --arg id "$id" '.items[]|select(.id==$id)' "$sdir/backlog.json")"
    att="$(printf '%s' "$item" | jq -r '.attempts // 0')"
    if [ "$att" -ge "$maxatt" ]; then
      state_set_status "$sdir/backlog.json" "$id" failed "exceeded max_attempts ($maxatt)"
      continue
    fi
    wt="$ws/.agentloop/worktrees/$id"
    rm -rf "$wt"; wt_remove "$ws" "$wt" "item/$id" >/dev/null 2>&1
    wt_create "$ws" "item/$id" "$wt" || { state_set_status "$sdir/backlog.json" "$id" failed "worktree create failed"; continue; }
    state_set_status "$sdir/backlog.json" "$id" in_progress
    state_increment_attempts "$sdir/backlog.json" "$id"
    role="$(printf '%s' "$item" | jq -r '.role // "build"')"
    rrole="$(config_resolve_role "$cfg" "$role")"
    tool="$(config_role_field "$cfg" "$rrole" tool)"
    model="$(config_role_field "$cfg" "$rrole" model)"
    progress_register "$sdir" "$id" "$(printf '%s' "$item" | jq -r '.title')" "$tool" "$model" "$ldir/item-$id.log"
    progress_spawn "$sdir" "$id" -- worker_dispatch "$cfg" "$ws" "$item" "$wt" "$ldir/item-$id.log" "$itimeout"
    dispatched="$dispatched $id"
  done

  [ -z "$dispatched" ] && { progress_render "$sdir" "$n" "$PROGRESS_RUN_START" "$PROGRESS_RUN_BUDGET"; return 0; }
  progress_wait "$sdir" "$n" "$PROGRESS_RUN_START" "$PROGRESS_RUN_BUDGET" -- $dispatched

  # Integrate sequentially based on each worker's result file (dispatched items only).
  for id in $dispatched; do
    local rfile="$ws/.agentloop/results/$id.json"
    if [ -f "$rfile" ] && jq -e '.status=="done"' "$rfile" >/dev/null 2>&1; then
      if [ -z "$(git -C "$ws" log --oneline "HEAD..item/$id" 2>/dev/null)" ]; then
        # Worker claimed done but committed nothing — merging would be a silent no-op.
        state_set_status "$sdir/backlog.json" "$id" ready "worker reported done but made no commits"
        progress_set_status "$sdir" "$id" bounced
      elif wt_merge "$ws" "item/$id"; then
        state_set_status "$sdir/backlog.json" "$id" done
        progress_set_status "$sdir" "$id" merged
        LOOP_DONE_ITEMS=$((LOOP_DONE_ITEMS+1))
      else
        state_set_status "$sdir/backlog.json" "$id" ready "merge conflict; replan"
        progress_set_status "$sdir" "$id" bounced
      fi
    else
      state_set_status "$sdir/backlog.json" "$id" ready "worker did not report done"
      progress_set_status "$sdir" "$id" failed
    fi
    wt_remove "$ws" "$ws/.agentloop/worktrees/$id" "item/$id" >/dev/null 2>&1
    rm -f "$rfile"
  done
  progress_render "$sdir" "$n" "$PROGRESS_RUN_START" "$PROGRESS_RUN_BUDGET"
  return 0
}
```

- [ ] **Step 5: Initialize progress state in `loop_run`**

In `lib/loop.sh`, in `loop_run`, find these lines (originally lines 82-83):

```bash
  local start; start="$(date +%s)"
  local n=0 stalls=0 prev_gate="init"
```

and replace them with (adds `progress_init` and exports the run window as globals so `loop_iterate` can render the budget bar — they are intentionally NOT `local`):

```bash
  local start; start="$(date +%s)"
  local n=0 stalls=0 prev_gate="init"
  progress_init
  PROGRESS_RUN_START="$start"; PROGRESS_RUN_BUDGET="$budget"
```

- [ ] **Step 6: Run the new loop test and the full suite**

Run: `bash tests/test_loop.sh`
Expected: PASS — including `loop emits progress dispatch event lines (non-TTY)`.

Run: `bash tests/run.sh`
Expected: PASS — `ALL SUITES PASSED` (existing suites unchanged; `test_progress.sh` and `test_loop.sh` green).

- [ ] **Step 7: Commit**

```bash
git add lib/loop.sh tests/test_loop.sh
git commit -m "feat(loop): track planner + workers as live progress jobs; replace blind wait"
```

---

### Task 6: Document `lib/progress.sh` in the README

**Files:**
- Modify: `README.md:35-47` (Layout block)

- [ ] **Step 1: Add the Layout entry**

In `README.md`, in the Layout code block, find the line:

```
lib/worker.sh           worker prompt + dispatch
```

and add a new line immediately after it:

```
lib/progress.sh         live progress dashboard (TTY) + event lines (non-TTY)
```

- [ ] **Step 2: Verify the suite still passes (docs change is inert)**

Run: `bash tests/run.sh`
Expected: PASS — `ALL SUITES PASSED`.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: note lib/progress.sh in the layout"
```

---

## Self-Review Notes

- **Spec coverage:** redrawing dashboard (Task 3 `progress_render` + Task 4 redraw loop); per-row id/title/tool/model/status/timer + log tail (Task 3); non-TTY event-line fallback (Task 2 register/set_status; Task 5 wiring; asserted in Task 5 Step 1); planner phase tracked (Task 5); state dir + parent-written job files (Task 2); `lib/loop.sh` stays control-flow with display isolated in `lib/progress.sh` (Tasks 1-5); existing `iter N`/`DONE`/`STOP` output preserved (unchanged in `loop_run`, verified by the untouched assertions in `test_loop.sh`); empty-log `starting…` placeholder (Task 1); Ctrl-C trap unaffected — cursor never hidden, no extra process (design honored: `progress.sh` emits no cursor-hide sequence). Testing section covered by `test_progress.sh` (Tasks 1-4) and the `test_loop.sh` regression (Task 5).
- **Known cosmetic caveats (from spec):** `${#line}` width counting assumes a UTF-8 locale (true on macOS default); `claude -p` may keep a worker's log near-empty until it finishes, so its tail shows `starting…` longer than `codex exec` would.
- **Type/name consistency:** job-file field order (`id,label,tool,model,log,start_epoch,status`) and sentinel format (`<exit_code> <end_epoch>`) are written in Task 2 and read identically in Tasks 3-4 and `loop_iterate`. Function names (`progress_init/dir/reset/register/set_status/spawn/sort_jobs/render/wait` + helpers) are used consistently across tasks and the loop wiring. Globals `PROGRESS_TTY`, `PROGRESS_LAST_LINES`, `PROGRESS_RUN_START`, `PROGRESS_RUN_BUDGET` are set before first use.
```
