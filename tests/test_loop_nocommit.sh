#!/bin/bash
# Guards the "worker reported done but committed nothing" path in loop_iterate:
# a no-op merge must NOT mark the item done; it bounces back to ready with a note.
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

# Planner seeds one ready item; WORKER writes a "done" result but makes NO commit.
stub="$ws/stub.sh"
cat > "$stub" <<'STUB'
#!/bin/bash
tool="$1"; shift
ws_state="$WS/.agentloop/state"; res="$WS/.agentloop/results"
prompt="$*"
case "$prompt" in
  *PLANNER*)
    echo '{"items":[{"id":"it-1","title":"f","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"x"}]}' > "$ws_state/backlog.json"
    ;;
  *WORKER*)
    # lies: claims done without committing anything in the worktree
    echo '{"status":"done","summary":"did nothing","files_changed":[]}' > "$res/it-1.json"
    ;;
esac
exit 0
STUB
chmod +x "$stub"
export FAKE_AGENT=1 FAKE_AGENT_BIN="$stub" WS="$ws"

cfg='{"caps":{"max_iterations":5,"max_parallel":2,"item_timeout_sec":30,"total_budget_sec":300,"max_attempts":3},"routing":{"planner":{"tool":"claude","model":"opus","effort":"high","flags":""},"build":{"tool":"codex","model":"gpt-5","effort":"high","flags":""}},"defaults":{"role":"build"}}'

loop_iterate "$cfg" "$ws" 1
assert_eq "${LOOP_DONE_ITEMS}" "0" "no-commit worker is not counted as merged"
assert_eq "$(jq -r '.items[]|select(.id=="it-1").status' "$ws/.agentloop/state/backlog.json")" "ready" "no-commit item bounced back to ready"
assert_contains "$(jq -r '.items[]|select(.id=="it-1").notes' "$ws/.agentloop/state/backlog.json")" "no commits" "note explains no commits"

test_summary
