# Worker prompt construction + dispatch inside a worktree.
. "$AGENTLOOP_HOME/lib/spawn.sh"

worker_prompt() { # workspace item_json
  local ws="$1" item="$2" id title desc acc
  id="$(printf '%s' "$item" | jq -r '.id')"
  title="$(printf '%s' "$item" | jq -r '.title')"
  desc="$(printf '%s' "$item" | jq -r '.desc')"
  acc="$(printf '%s' "$item" | jq -r '.acceptance // "the change builds and tests pass"')"
  cat <<EOF
You are a WORKER on an autonomous app build. You are in a git worktree of the project.
Implement exactly this item and nothing else:

  id:    $id
  title: $title
  task:  $desc
  done when: $acc

Rules:
- Make focused commits in this worktree as you go.
- Verify your work against the acceptance criteria before finishing.
- When finished, write $ws/.agentloop/results/$id.json:
  {"status":"done|failed","summary":"one line","files_changed":["..."]}
EOF
}

# Dispatch one item: returns agent_run's exit code; result file is the source of truth.
worker_dispatch() { # config_json workspace item_json worktree logfile timeout_sec
  local cfg="$1" ws="$2" item="$3" wt="$4" log="$5" t="$6" role
  role="$(printf '%s' "$item" | jq -r '.role // "build"')"
  agent_run "$cfg" "$role" "$(worker_prompt "$ws" "$item")" "$wt" "$log" "$t"
}
