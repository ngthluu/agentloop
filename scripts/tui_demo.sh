#!/bin/bash
# Offline TUI demo / manual-verification harness. Spends no API tokens: it points
# agentloop at a fake agent (FAKE_AGENT) that scripts a small run exercising the
# question inbox, answering, standby, and add-task.
#
# Run it from a real terminal (the TUI only activates on a TTY):
#   ./scripts/tui_demo.sh
#
# What to verify by eye:
#   1. The manager and architect run, then builder task-1-b1 raises a question -> it appears in the inbox
#      (the status bar shows "? 1").
#   2. Press enter on the question, type  yes , press enter. task-1-b1 then builds and merges.
#   3. The gate passes and the UI flips to "DONE - standby".
#   4. Press  a , type  also build task-2 , press enter. The manager folds it into task-2,
#      the architect plans it, the builder builds it, customer approves it, gate passes, back to standby.
#   5. Press  q  to quit; the terminal is restored cleanly (no leftover raw mode).
#   6. The frame stays in one place — no panel piling up in scrollback while the
#      agents run (stderr now goes to .agentloop/logs/run.log).
#   7. Press  tab  to focus the Jobs pane (its border highlights). Use  up/down  to
#      pick a job, then  enter  to open its detail view: a header with status, role,
#      tool/model, and a live ticking working-time, plus the tail of the job's log.
#   8. Press  esc  to return to the two-pane list.
#   9. Each job row shows a working-time that ticks while running and freezes when the
#      job merges/fails.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
BIN="$ROOT/target/release/agentloop"
[ -x "$BIN" ] || { echo "building release binary..."; (cd "$ROOT" && cargo build --release); }

# Canonical workspace path so the binary's canonicalize() matches $WS in the stub.
WS="$(cd "$(mktemp -d "${TMPDIR:-/tmp}/agentloop-demo.XXXXXX")" && pwd -P)"
export WS

cat > "$WS/stub.sh" <<'STUB'
#!/bin/bash
tool="$1"; shift
prompt="$*"
ws="$WS"; st="$ws/.agentloop/state"; res="$ws/.agentloop/results"
sleep 2   # make jobs visible in the panel
case "$prompt" in
  *MANAGER*)
    python3 - "$ws" <<'PY'
import json, os, sys
ws = sys.argv[1]; st = os.path.join(ws, '.agentloop', 'state'); res = os.path.join(ws, '.agentloop', 'results')
bkp = os.path.join(st, 'backlog.json'); d = json.load(open(bkp))
def ensure(idn, title, desc, acc):
    if not any(i['id'] == idn for i in d['items']):
        d['items'].append({"id": idn, "title": title, "desc": desc,
                            "deps": [], "status": "ready", "attempts": 0, "acceptance": acc})
if not d['items']:
    ensure('task-1', 'Create first demo file', 'Create task-1.txt after user confirmation', 'task-1.txt exists')
# fold any pending user request into a new item
reqp = os.path.join(st, 'requests.jsonl')
if os.path.exists(reqp):
    pend = [l for l in open(reqp).read().splitlines()
            if l.strip() and json.loads(l)['status'] == 'pending']
    if pend:
        ensure('task-2', 'Create second demo file', 'Create task-2.txt from an added request', 'task-2.txt exists')
# verify.sh requires a <id>.txt for every item
expected = ' '.join(i['id'] + '.txt' for i in d['items'])
open(os.path.join(ws, '.agentloop', 'verify.sh'), 'w').write(
    '#!/bin/bash\nfor f in ' + expected + '; do [ -f "$PWD/$f" ] || exit 1; done\nexit 0\n')
os.chmod(os.path.join(ws, '.agentloop', 'verify.sh'), 0o755)
json.dump(d, open(bkp, 'w'))
open(os.path.join(st, 'master.md'), 'w').write('# demo status\n')
PY
    ;;
  *ARCHITECT*)
    task=$(printf '%s' "$prompt" | sed -n 's/^  id: \([a-z0-9-]*\)$/\1/p' | head -1)
    [ -z "$task" ] && task=task-1
    mkdir -p "$st/tasks/$task"
    echo "Create $task.txt with simple demo content. Keep the implementation to one file." > "$st/tasks/$task/design.md"
    cat > "$st/tasks/$task/builders.json" <<JSON
{"items":[{"id":"$task-b1","title":"Create $task.txt","desc":"Write $task.txt and commit it","deps":[],"status":"ready","attempts":0,"acceptance":"$task.txt exists"}]}
JSON
    ;;
  *BUILDER*)
    id=$(printf '%s' "$prompt" | sed -n 's/^  id:    \([a-z0-9-]*\)$/\1/p' | tail -1)
    [ -z "$id" ] && id=task-1-b1
    task=${id%-b1}
    if [ "$id" = "task-1-b1" ] && [ ! -f "$ws/.agentloop/answers/task-1-b1.json" ]; then
      mkdir -p "$ws/.agentloop/questions"
      echo '{"question":"Proceed and create task-1.txt?","context":"first file"}' > "$ws/.agentloop/questions/task-1-b1.json"
      echo '{"status":"needs_input","summary":"confirm before creating the file"}' > "$res/task-1-b1.json"
    else
      echo made > "$PWD/$task.txt"; git add -A; git commit -qm "builder $id" 2>/dev/null
      echo "{\"status\":\"done\",\"summary\":\"made $task\",\"files_changed\":[\"$task.txt\"]}" > "$res/$id.json"
    fi
    ;;
  *"SILLY CUSTOMER"*)
    task=$(printf '%s' "$prompt" | sed -n 's/^  id: \([a-z0-9-]*\)$/\1/p' | head -1)
    [ -z "$task" ] && task=task-1
    mkdir -p "$st/tasks/$task"
    echo "{\"status\":\"approved\",\"summary\":\"accepted $task\",\"acceptance_notes\":\"$task.txt exists\"}" > "$st/tasks/$task/customer.json"
    echo "{\"status\":\"approved\",\"summary\":\"accepted $task\"}" > "$res/$task-customer.json"
    ;;
esac
exit 0
STUB
chmod +x "$WS/stub.sh"

cat > "$WS/config.json" <<'JSON'
{
  "caps": {
    "max_iterations": 25,
    "max_parallel": 3,
    "item_timeout_sec": 1200,
    "total_budget_sec": 21600,
    "max_attempts": 3
  },
  "routing": {
    "manager": { "tool": "claude", "model": "demo-manager", "effort": "high" },
    "architect": { "tool": "claude", "model": "demo-architect", "effort": "high" },
    "builder": { "tool": "codex", "model": "demo-builder", "effort": "high" },
    "customer": { "tool": "claude", "model": "demo-customer", "effort": "medium" },
    "resolver": { "tool": "claude", "model": "demo-resolver", "effort": "medium" }
  },
  "defaults": { "role": "builder" }
}
JSON

export FAKE_AGENT=1
export FAKE_AGENT_BIN="$WS/stub.sh"

echo "demo workspace: $WS"
echo "launching TUI (no tokens spent). Follow the steps in this script's header."
echo
exec "$BIN" "build a demo app" --workspace "$WS" --config "$WS/config.json"
