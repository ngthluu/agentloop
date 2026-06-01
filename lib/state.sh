# backlog.json read/mutate helpers. All writes are atomic (temp + mv).
state_backlog_valid() { # path -> exit 0 if valid
  jq -e 'has("items") and (.items|type=="array")' "$1" >/dev/null 2>&1
}

# Emit ids of ready items whose deps are all done, capped at max_parallel.
state_ready_items() { # path max_parallel
  jq -r '
    ([.items[] | select(.status=="done") | .id]) as $done
    | .items[]
    | select(.status=="ready")
    | select(all((.deps // [])[]; . as $d | ($done | index($d)) != null))
    | .id
  ' "$1" | head -n "$2"
}

state_open_count() { # path
  jq '[.items[] | select(.status=="ready" or .status=="in_progress" or .status=="blocked")] | length' "$1"
}

state_set_status() { # path id status [note]
  local tmp; tmp="$(mktemp "$(dirname "$1")/state.XXXXXX")"   # same fs as target -> mv is atomic
  jq --arg id "$2" --arg st "$3" --arg note "${4:-}" '
    .items |= map(if .id==$id then .status=$st | (if $note!="" then .notes=$note else . end) else . end)
  ' "$1" > "$tmp" && mv "$tmp" "$1" || { rm -f "$tmp"; return 1; }
}

state_increment_attempts() { # path id
  local tmp; tmp="$(mktemp "$(dirname "$1")/state.XXXXXX")"
  jq --arg id "$2" '.items |= map(if .id==$id then .attempts=((.attempts//0)+1) else . end)' "$1" > "$tmp" && mv "$tmp" "$1" || { rm -f "$tmp"; return 1; }
}
