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
