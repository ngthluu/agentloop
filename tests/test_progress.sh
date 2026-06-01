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

test_summary
