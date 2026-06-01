# The orchestration loop. Pure control flow over the other libs.
. "$AGENTLOOP_HOME/lib/config.sh"
. "$AGENTLOOP_HOME/lib/state.sh"
. "$AGENTLOOP_HOME/lib/planner.sh"
. "$AGENTLOOP_HOME/lib/worker.sh"
. "$AGENTLOOP_HOME/lib/worktree.sh"

# Run the gate; write output to last_gate.txt; return its exit code (1 if no verify.sh yet).
loop_gate() { # workspace
  local ws="$1" gate="$1/.agentloop/verify.sh" out="$1/.agentloop/state/last_gate.txt"
  if [ -x "$gate" ]; then
    ( cd "$ws" && /bin/bash "$gate" ) > "$out" 2>&1
    return $?
  fi
  echo "no verify.sh yet" > "$out"; return 1
}

# One iteration: plan, select, dispatch in parallel, integrate, gate.
loop_iterate() { # config_json workspace iter_n -> sets LOOP_DONE_ITEMS (count merged)
  local cfg="$1" ws="$2" n="$3"
  local sdir="$ws/.agentloop/state" ldir="$ws/.agentloop/logs/iter-$n"
  mkdir -p "$ldir" "$ws/.agentloop/results"
  local itimeout; itimeout="$(config_cap "$cfg" item_timeout_sec)"; : "${itimeout:=1200}"
  local maxpar; maxpar="$(config_cap "$cfg" max_parallel)"; : "${maxpar:=3}"
  local maxatt; maxatt="$(config_cap "$cfg" max_attempts)"; : "${maxatt:=3}"

  planner_run "$cfg" "$ws" "$ldir/planner.log" "$itimeout" || { echo "planner failed/invalid" >&2; return 2; }

  local ids; ids="$(state_ready_items "$sdir/backlog.json" "$maxpar")"
  LOOP_DONE_ITEMS=0
  [ -z "$ids" ] && return 0

  # Dispatch each ready item in its own worktree, in parallel.
  # `dispatched` collects only the ids that actually got a worker, so the integration
  # pass below never re-touches items we capped or failed to set up here.
  local id item wt att dispatched=""
  for id in $ids; do
    item="$(jq -c --arg id "$id" '.items[]|select(.id==$id)' "$sdir/backlog.json")"
    # Enforce the per-item attempt cap: drop runaway items instead of retrying forever.
    att="$(printf '%s' "$item" | jq -r '.attempts // 0')"
    if [ "$att" -ge "$maxatt" ]; then
      state_set_status "$sdir/backlog.json" "$id" failed "exceeded max_attempts ($maxatt)"
      continue
    fi
    wt="$ws/.agentloop/worktrees/$id"
    rm -rf "$wt"; wt_remove "$ws" "$wt" "item/$id" >/dev/null 2>&1
    wt_create "$ws" "item/$id" "$wt" || { state_set_status "$sdir/backlog.json" "$id" failed "worktree create failed"; continue; }
    state_set_status "$sdir/backlog.json" "$id" in_progress
    state_increment_attempts "$sdir/backlog.json" "$id"
    ( worker_dispatch "$cfg" "$ws" "$item" "$wt" "$ldir/item-$id.log" "$itimeout" ) &
    dispatched="$dispatched $id"
  done
  wait

  # Integrate sequentially based on each worker's result file (dispatched items only).
  for id in $dispatched; do
    local rfile="$ws/.agentloop/results/$id.json"
    if [ -f "$rfile" ] && jq -e '.status=="done"' "$rfile" >/dev/null 2>&1; then
      if [ -z "$(git -C "$ws" log --oneline "HEAD..item/$id" 2>/dev/null)" ]; then
        # Worker claimed done but committed nothing — merging would be a silent no-op.
        state_set_status "$sdir/backlog.json" "$id" ready "worker reported done but made no commits"
      elif wt_merge "$ws" "item/$id"; then
        state_set_status "$sdir/backlog.json" "$id" done
        LOOP_DONE_ITEMS=$((LOOP_DONE_ITEMS+1))
      else
        state_set_status "$sdir/backlog.json" "$id" ready "merge conflict; replan"
      fi
    else
      state_set_status "$sdir/backlog.json" "$id" ready "worker did not report done"
    fi
    wt_remove "$ws" "$ws/.agentloop/worktrees/$id" "item/$id" >/dev/null 2>&1
    rm -f "$rfile"
  done
  return 0
}

# Drive iterations until DONE (0), cap/stall (returns 1), or hard error (2).
loop_run() { # config_json workspace
  local cfg="$1" ws="$2" sdir="$2/.agentloop/state"
  local maxit; maxit="$(config_cap "$cfg" max_iterations)"; : "${maxit:=25}"
  local budget; budget="$(config_cap "$cfg" total_budget_sec)"; : "${budget:=21600}"
  local start; start="$(date +%s)"
  local n=0 stalls=0 prev_gate="init"

  while [ "$n" -lt "$maxit" ]; do
    n=$((n+1))
    if [ $(( $(date +%s) - start )) -ge "$budget" ]; then echo "STOP: time budget exceeded" >&2; return 1; fi

    loop_iterate "$cfg" "$ws" "$n"; local irc=$?
    [ "$irc" -eq 2 ] && return 2

    loop_gate "$ws"; local grc=$?
    local gate_state="fail"; [ "$grc" -eq 0 ] && gate_state="pass"
    local open; open="$(state_open_count "$sdir/backlog.json")"

    echo "iter $n: merged=${LOOP_DONE_ITEMS:-0} gate=$gate_state open=$open" >&2

    if [ "$gate_state" = "pass" ] && [ "$open" -eq 0 ]; then echo "DONE" >&2; return 0; fi

    if [ "${LOOP_DONE_ITEMS:-0}" -eq 0 ] && [ "$gate_state" = "$prev_gate" ]; then
      stalls=$((stalls+1))
      if [ "$stalls" -ge 2 ]; then echo "STOP: no progress for 2 stalls (3 consecutive iterations)" >&2; return 1; fi
    else
      stalls=0
    fi
    prev_gate="$gate_state"
  done
  echo "STOP: max_iterations reached" >&2
  return 1
}
