#!/bin/bash
# Offline TUI demo / manual-verification harness. Spends no API tokens: it points
# agentloop at a fake agent (FAKE_AGENT) that scripts a small run exercising the
# question inbox, answering, standby, and add-task.
#
# Run it from a real terminal (the TUI only activates on a TTY):
#   ./scripts/tui_demo.sh
#
# What to verify by eye:
#   1. The planner runs, then worker it-1 raises a question -> it appears in the inbox
#      (the status bar shows "? 1").
#   2. Press enter on the question, type  yes , press enter. it-1 then builds and merges.
#   3. The gate passes and the UI flips to "DONE - standby".
#   4. Press  a , type  also build it-2 , press enter. The planner folds it into it-2,
#      the worker builds it, gate passes, back to standby.
#   5. Press  q  to quit; the terminal is restored cleanly (no leftover raw mode).
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
  *PLANNER*)
    python3 - "$ws" <<'PY'
import json, os, sys
ws = sys.argv[1]; st = os.path.join(ws, '.agentloop', 'state'); res = os.path.join(ws, '.agentloop', 'results')
bkp = os.path.join(st, 'backlog.json'); d = json.load(open(bkp))
def ensure(idn, acc):
    if not any(i['id'] == idn for i in d['items']):
        d['items'].append({"id": idn, "title": idn, "desc": "make "+idn, "role": "build",
                            "deps": [], "status": "ready", "attempts": 0, "acceptance": acc})
if not d['items']:
    ensure('it-1', 'it-1.txt exists')
# mark finished items done
for i in d['items']:
    if os.path.exists(os.path.join(res, i['id'] + '.json')):
        i['status'] = 'done'
# fold any pending user request into a new item
reqp = os.path.join(st, 'requests.jsonl')
if os.path.exists(reqp):
    pend = [l for l in open(reqp).read().splitlines()
            if l.strip() and json.loads(l)['status'] == 'pending']
    if pend:
        ensure('it-2', 'it-2.txt exists')
# verify.sh requires a <id>.txt for every item
expected = ' '.join(i['id'] + '.txt' for i in d['items'])
open(os.path.join(ws, '.agentloop', 'verify.sh'), 'w').write(
    '#!/bin/bash\nfor f in ' + expected + '; do [ -f "$PWD/$f" ] || exit 1; done\nexit 0\n')
os.chmod(os.path.join(ws, '.agentloop', 'verify.sh'), 0o755)
json.dump(d, open(bkp, 'w'))
open(os.path.join(st, 'master.md'), 'w').write('# demo status\n')
PY
    ;;
  *WORKER*)
    id=$(printf '%s' "$prompt" | sed -n 's/.*id:    \([a-z0-9-]*\).*/\1/p' | head -1)
    [ -z "$id" ] && id=it-1
    if [ "$id" = "it-1" ] && [ ! -f "$ws/.agentloop/answers/it-1.json" ]; then
      mkdir -p "$ws/.agentloop/questions"
      echo '{"question":"Proceed and create it-1.txt?","context":"first file"}' > "$ws/.agentloop/questions/it-1.json"
      echo '{"status":"needs_input","summary":"confirm before creating the file"}' > "$res/it-1.json"
    else
      echo made > "$PWD/$id.txt"; git add -A; git commit -qm "worker $id" 2>/dev/null
      echo "{\"status\":\"done\",\"summary\":\"made $id\",\"files_changed\":[\"$id.txt\"]}" > "$res/$id.json"
    fi
    ;;
esac
exit 0
STUB
chmod +x "$WS/stub.sh"

export FAKE_AGENT=1
export FAKE_AGENT_BIN="$WS/stub.sh"

echo "demo workspace: $WS"
echo "launching TUI (no tokens spent). Follow the steps in this script's header."
echo
exec "$BIN" "build a demo app" --workspace "$WS"
