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
prompt="$*"
case "$prompt" in
  *PLANNER*)
    n=$(cat "$WS/.plan_n" 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > "$WS/.plan_n"
    if [ "$n" -eq 1 ]; then
      echo '{"items":[{"id":"it-1","title":"f","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/backlog.json"
      printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$WS/.agentloop/verify.sh"; chmod +x "$WS/.agentloop/verify.sh"
    else
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
