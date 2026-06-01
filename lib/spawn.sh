# Spawning + timeout. No timeout(1) on this platform, so we background + poll + kill.
. "$AGENTLOOP_HOME/lib/config.sh"

# Run a command with a wall-clock cap. Returns command's exit code, or 124 on timeout.
run_with_timeout() { # timeout_sec logfile -- cmd args...
  local t="$1" log="$2"; shift 2
  ( "$@" ) >"$log" 2>&1 &
  local pid=$!
  local waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge "$t" ]; then
      kill "$pid" 2>/dev/null
      sleep 1
      kill -9 "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      return 124
    fi
    sleep 1
    waited=$((waited+1))
  done
  wait "$pid"
}

# Resolve a role and run the matching CLI (or fake) in cwd, capped by timeout.
agent_run() { # config_json role prompt cwd logfile timeout_sec
  local cfg="$1" role rrole tool model effort flags prompt="$3" cwd="$4" log="$5" t="$6"
  rrole="$(config_resolve_role "$cfg" "$2")"
  tool="$(config_role_field "$cfg" "$rrole" tool)"
  model="$(config_role_field "$cfg" "$rrole" model)"
  effort="$(config_role_field "$cfg" "$rrole" effort)"
  flags="$(config_role_field "$cfg" "$rrole" flags)"

  # Build the real argv as an indexed array (bash 3.2 supports these).
  local argv=()
  if [ "$tool" = "claude" ]; then
    argv=( claude -p "$prompt" )
  elif [ "$tool" = "codex" ]; then
    argv=( codex exec "$prompt" )
  else
    echo "agent_run: unknown tool [$tool]" >&2; return 2
  fi

  [ -n "$model" ]  && { [ "$tool" = "codex" ] && argv+=( -m "$model" ) || argv+=( --model "$model" ); }
  [ -n "$effort" ] && { [ "$tool" = "codex" ] && argv+=( -c "model_reasoning_effort=$effort" ) || argv+=( --effort "$effort" ); }
  [ -n "$flags" ] && argv+=( $flags )

  # In fake mode, intercept by prepending the stub; it receives the genuine real argv,
  # so the real command construction above is what tests actually exercise.
  [ "${FAKE_AGENT:-0}" = "1" ] && argv=( "$FAKE_AGENT_BIN" "${argv[@]}" )

  ( cd "$cwd" && run_with_timeout "$t" "$log" "${argv[@]}" )
}
