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
assert_contains "$p" "PLANNER" "planner prompt has PLANNER marker"

item='{"id":"it-7","title":"add tests","desc":"write pytest","acceptance":"pytest passes"}'
w="$(worker_prompt "$ws" "$item")"
assert_contains "$w" "write pytest" "worker sees desc"
assert_contains "$w" "it-7" "worker sees id"
assert_contains "$w" "results/it-7.json" "worker told result contract"
assert_contains "$w" "WORKER" "worker prompt has WORKER marker"

test_summary
