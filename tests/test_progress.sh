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
assert_eq "$(printf '\033[1;32mbold-green\033[0m' | progress_strip_ansi)" "bold-green" "strip multi-param ansi"

# --- progress_truncate ---
assert_eq "$(printf 'hello'      | progress_truncate 10)" "hello"  "no truncation when short"
assert_eq "$(printf 'helloworld' | progress_truncate 5)"  "hell…"  "truncate adds ellipsis"
assert_eq "$(printf 'abcde' | progress_truncate 5)" "abcde" "no truncation at exact width"
assert_eq "$(printf 'ab'    | progress_truncate 0)" ""      "width 0 yields empty"

# --- progress_tail_log ---
: > "$ws/empty.log"
assert_eq "$(progress_tail_log "$ws/empty.log")" "starting…" "empty log -> placeholder"
printf 'line1\n\nediting foo.py\n' > "$ws/a.log"
assert_eq "$(progress_tail_log "$ws/a.log")" "editing foo.py" "tail last non-empty line"

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

# --- on a TTY, register/set_status stay silent (dashboard handles display) ---
PROGRESS_TTY=1
quiet_reg="$(progress_register "$sdir" it-q "q" claude opus "$ws/a.log" 2>&1)"
assert_eq "$quiet_reg" "" "register is silent on TTY"
quiet_st="$(progress_set_status "$sdir" it-q merged 2>&1)"
assert_eq "$quiet_st" "" "set_status is silent on TTY"
PROGRESS_TTY=0

# --- a tab in the label is sanitized so the TSV stays well-formed ---
PROGRESS_TTY=0
printf 'reg with tab\n' >/dev/null
progress_register "$sdir" it-tab "$(printf 'a\tb')" claude opus "$ws/a.log" >/dev/null 2>&1
assert_eq "$(cut -f2 "$sdir/progress/it-tab.job")" "a b" "tab in label replaced with space"
assert_eq "$(cut -f3 "$sdir/progress/it-tab.job")" "claude" "fields stay aligned after sanitize"

# --- reset clears the progress dir ---
progress_reset "$sdir"
assert_eq "$(ls "$sdir/progress" 2>/dev/null | wc -l | tr -d ' ')" "0" "reset empties progress dir"

# --- spawn backgrounds a command and writes a sentinel with exit code + end time ---
progress_register "$sdir" w1 lbl codex gpt-5 "$ws/a.log" >/dev/null 2>&1
progress_spawn "$sdir" w1 -- /bin/bash -c 'exit 3'
wait
assert_eq "$(cut -d' ' -f1 "$sdir/progress/w1.done")" "3" "spawn sentinel records exit code"

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

# --- render: finishing state + PROGRESS_LAST_LINES equals printed line count ---
progress_reset "$sdir"
PROGRESS_TTY=1; PROGRESS_LAST_LINES=0
now="$(date +%s)"
# a running job (no sentinel): row + tail = 2 lines
printf 'r1\twork\tcodex\tgpt-5.5\t%s\t%s\trunning\n' "$ws/a.log" "$((now-30))" > "$sdir/progress/r1.job"
# a finishing job: sentinel present but status still "running": row + tail = 2 lines
printf 'f1\twork\tcodex\tgpt-5.5\t%s\t%s\trunning\n' "$ws/a.log" "$((now-40))" > "$sdir/progress/f1.job"
printf '0 %s\n' "$((now-10))" > "$sdir/progress/f1.done"
fin_out="$(progress_render "$sdir" 4 "$((now-130))" 360 2>&1)"
assert_contains "$fin_out" "finishing" "render: finishing state shown for sentinel+running job"
# Run again in the CURRENT shell (no command-substitution subshell) so the global update is visible.
PROGRESS_LAST_LINES=0
progress_render "$sdir" 4 "$((now-130))" 360 2>/dev/null
# header(1) + separator(1) + r1 row(1) + r1 tail(1) + f1 row(1) + f1 tail(1) = 6
assert_eq "$PROGRESS_LAST_LINES" "6" "render: PROGRESS_LAST_LINES equals printed line count"

test_summary
