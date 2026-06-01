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

# a non-zero agent exit propagates through agent_run (full stack)
log5="$ws/e.log"
( export FAKE_EXIT=1; agent_run "$cfg" build "fail" "$ws" "$log5" 30 )
assert_fail $? "agent_run propagates non-zero exit"

test_summary
