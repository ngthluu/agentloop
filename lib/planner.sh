# Planner prompt construction + invocation. Reuses spawn + state.
. "$AGENTLOOP_HOME/lib/spawn.sh"
. "$AGENTLOOP_HOME/lib/state.sh"

planner_prompt() { # workspace [max_attempts]
  local ws="$1" maxatt="${2:-3}" goal master backlog
  goal="$(cat "$ws/.agentloop/state/goal.md")"
  master="$(cat "$ws/.agentloop/state/master.md" 2>/dev/null)"
  backlog="$(cat "$ws/.agentloop/state/backlog.json" 2>/dev/null)"
  cat <<EOF
You are the PLANNER for an autonomous app build. Working dir: $ws (a git repo).

GOAL:
$goal

CURRENT master.md:
$master

CURRENT backlog.json:
$backlog

Your job each round:
1. Read worker results in .agentloop/results/ and the latest gate output in
   .agentloop/state/last_gate.txt (if present). Mark finished items status="done".
2. Add/split/refine items so the GOAL gets built. First round: scaffold the project
   and write an executable .agentloop/verify.sh that builds/tests the app (start simple).
3. The orchestrator FAILS any item once its attempts reach $maxatt (the max_attempts cap).
   So for any item nearing attempts=$maxatt, redesign it (smaller/different) or drop it
   instead of re-queueing the same work.
4. Assign each open item a role from the config routing (planner|architect|build|fix|trivial),
   realistic deps (ids of items that must finish first), and a concrete acceptance string.

OUTPUT CONTRACT — you MUST overwrite .agentloop/state/backlog.json with valid JSON:
{"items":[{"id","title","desc","role","deps":[],"status":"ready|in_progress|done|failed|blocked","attempts":0,"acceptance"}]}
Also rewrite .agentloop/state/master.md as a human-readable status board.
Do not print the JSON to stdout; write the files.
EOF
}

# Invoke the planner agent, then validate backlog.json (re-prompt once on invalid).
planner_run() { # config_json workspace logfile timeout_sec
  local cfg="$1" ws="$2" log="$3" t="$4" bk="$2/.agentloop/state/backlog.json" maxatt
  maxatt="$(config_cap "$cfg" max_attempts)"; : "${maxatt:=3}"
  agent_run "$cfg" planner "$(planner_prompt "$ws" "$maxatt")" "$ws" "$log" "$t"
  if state_backlog_valid "$bk"; then return 0; fi
  echo "planner produced invalid backlog.json; re-prompting once" >&2
  agent_run "$cfg" planner "$(planner_prompt "$ws" "$maxatt")
NOTE: your previous backlog.json was invalid JSON. Write valid JSON this time." "$ws" "$log" "$t"
  state_backlog_valid "$bk"
}
