#!/bin/bash
# Guards the per-item attempt cap: an item that has already reached max_attempts must be
# marked failed and NOT dispatched to a worker again.
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
echo "g" > "$ws/.agentloop/state/goal.md"
echo "# status" > "$ws/.agentloop/state/master.md"
# item already at the cap (attempts=3, max_attempts=3)
echo '{"items":[{"id":"it-1","title":"t","desc":"d","role":"build","deps":[],"status":"ready","attempts":3,"acceptance":"x"}]}' > "$ws/.agentloop/state/backlog.json"

# Planner stub is a no-op (leaves the seeded backlog); Worker stub records if it ran.
stub="$ws/stub.sh"
cat > "$stub" <<'STUB'
#!/bin/bash
tool="$1"; shift
case "$*" in
  *WORKER*) touch "$WS/.worker_ran" ;;
esac
exit 0
STUB
chmod +x "$stub"
export FAKE_AGENT=1 FAKE_AGENT_BIN="$stub" WS="$ws"

cfg='{"caps":{"max_iterations":5,"max_parallel":2,"item_timeout_sec":30,"total_budget_sec":300,"max_attempts":3},"routing":{"planner":{"tool":"claude","model":"opus","effort":"high","flags":""},"build":{"tool":"codex","model":"gpt-5","effort":"high","flags":""}},"defaults":{"role":"build"}}'

loop_iterate "$cfg" "$ws" 1
assert_eq "${LOOP_DONE_ITEMS}" "0" "capped item is not merged"
assert_eq "$(jq -r '.items[]|select(.id=="it-1").status' "$ws/.agentloop/state/backlog.json")" "failed" "capped item marked failed"
assert_contains "$(jq -r '.items[]|select(.id=="it-1").notes' "$ws/.agentloop/state/backlog.json")" "exceeded max_attempts" "note explains the cap"
[ -f "$ws/.worker_ran" ]; assert_fail $? "worker NOT dispatched for capped item"

test_summary
