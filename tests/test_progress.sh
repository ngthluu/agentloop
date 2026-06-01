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

test_summary
