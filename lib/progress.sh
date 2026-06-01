# Live progress display + per-job state. Sourced by loop.sh.
# All cursor control is TTY-only; non-TTY callers get append-only event lines.
PROGRESS_REFRESH=1   # seconds between redraws on a TTY

# "Xm YYs" from a whole number of seconds.
progress_fmt_elapsed() { # seconds
  printf '%dm%02ds' "$(( $1 / 60 ))" "$(( $1 % 60 ))"
}

# Strip ANSI CSI escapes from stdin.
progress_strip_ansi() {
  sed $'s/\033\\[[0-9;]*[A-Za-z]//g'
}

# Read one line from stdin; if longer than width, cut and append an ellipsis.
progress_truncate() { # width
  local w="$1" line; IFS= read -r line || true
  [ "$w" -le 0 ] && return 0
  if [ "${#line}" -gt "$w" ]; then printf '%s' "${line:0:$((w-1))}…"; else printf '%s' "$line"; fi
}

# Last non-empty, ANSI-stripped line of a log, or a placeholder when empty.
progress_tail_log() { # logfile
  local line=""
  if [ -s "$1" ]; then
    line="$(tail -n 20 "$1" | progress_strip_ansi | grep -v '^[[:space:]]*$' | tail -n 1)"
  fi
  [ -n "$line" ] && printf '%s' "$line" || printf 'starting…'
}

# Detect once whether stderr is an interactive terminal.
progress_init() {
  if [ -t 2 ]; then PROGRESS_TTY=1; else PROGRESS_TTY=0; fi
  PROGRESS_LAST_LINES=0
}

# Path to the per-iteration job-state directory.
progress_dir() { printf '%s/progress' "$1"; }   # statedir

# Map an id to a filesystem-safe key for job/sentinel filenames (ids come from
# AI-generated backlog.json and may contain '/', tabs, or newlines).
progress_key() { # id
  local k="$1"; k="${k//\//_}"; k="${k//$'\t'/_}"; k="${k//$'\n'/_}"; printf '%s' "$k"
}

# Wipe and recreate the job-state dir for a fresh iteration.
progress_reset() { # statedir
  [ -n "$1" ] || return 1
  local d; d="$(progress_dir "$1")"
  rm -rf "$d"; mkdir -p "$d"
  PROGRESS_LAST_LINES=0
}

# Register a running job. Parent calls this right before backgrounding the work.
progress_register() { # statedir id label tool model log
  local d safe key; d="$(progress_dir "$1")"; mkdir -p "$d"
  key="$(progress_key "$2")"
  safe="$3"; safe="${safe//$'\t'/ }"; safe="${safe//$'\n'/ }"
  printf '%s\t%s\t%s\t%s\t%s\t%s\trunning\n' \
    "$key" "$safe" "$4" "$5" "$6" "$(date +%s)" > "$d/$key.job"
  [ "${PROGRESS_TTY:-0}" = "1" ] || \
    printf '%s  dispatch %-10s %s/%s  %s\n' "$(date +%H:%M:%S)" "$key" "$4" "$5" "$3" >&2
}

# Update a job's status field (id stays the same). No-op if the job is unknown.
progress_set_status() { # statedir id status
  local d f key id label tool model log start st
  d="$(progress_dir "$1")"; key="$(progress_key "$2")"; f="$d/$key.job"
  [ -f "$f" ] || return 0
  IFS=$'\t' read -r id label tool model log start st < "$f"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$id" "$label" "$tool" "$model" "$log" "$start" "$3" > "$f"
  [ "${PROGRESS_TTY:-0}" = "1" ] || \
    printf '%s  %-9s %-10s %s/%s\n' "$(date +%H:%M:%S)" "$3" "$key" "$tool" "$model" >&2
}

# Background a command; write a "<exit_code> <end_epoch>" sentinel when it exits.
# Sentinels (not kill -0) are how progress_wait detects completion: an un-reaped
# child becomes a zombie whose pid still answers kill -0, which would hang a poll.
# A SIGKILL landing between the command finishing and the sentinel write is the only
# way no sentinel appears; the orchestrator sends SIGTERM first, so the window is tiny.
progress_spawn() { # statedir id -- cmd...
  local d key; d="$(progress_dir "$1")"; key="$(progress_key "$2")"; shift 2
  [ "${1:-}" = "--" ] && shift
  ( "$@"; printf '%s %s\n' "$?" "$(date +%s)" > "$d/$key.done" ) &
}

# Emit job-file paths: active jobs (no sentinel) first, then finished ones.
progress_sort_jobs() { # progress_dir
  local jf
  for jf in "$1"/*.job; do [ -e "$jf" ] || continue; [ -f "${jf%.job}.done" ] || printf '%s\n' "$jf"; done
  for jf in "$1"/*.job; do [ -e "$jf" ] || continue; [ -f "${jf%.job}.done" ] && printf '%s\n' "$jf"; done
}

# Draw one dashboard frame to stderr. TTY-only: a no-op otherwise.
# Redraws in place by moving up the previous frame's line count and clearing down.
progress_render() { # statedir iter budget_start budget_total
  [ "${PROGRESS_TTY:-0}" = "1" ] || return 0
  local d iter bstart btot now cols out nlines act fin jf
  local id label tool model log start st glyph el endep tl row
  d="$(progress_dir "$1")"; iter="$2"; bstart="$3"; btot="$4"
  now="$(date +%s)"
  cols="$(tput cols 2>/dev/null)"; cols="${cols:-80}"
  case "$cols" in *[!0-9]*|"") cols=80;; esac
  [ "$cols" -gt 100 ] && cols=100

  act=0; fin=0
  for jf in "$d"/*.job; do [ -e "$jf" ] || continue
    if [ -f "${jf%.job}.done" ]; then fin=$((fin+1)); else act=$((act+1)); fi
  done

  out="iter $iter | elapsed $(progress_fmt_elapsed $((now-bstart)))/$(progress_fmt_elapsed "$btot") | $act running, $fin done"$'\n'
  nlines=1
  out="$out$(printf '%*s' "$cols" '' | tr ' ' '-')"$'\n'; nlines=$((nlines+1))

  while IFS= read -r jf; do
    [ -e "$jf" ] || continue
    IFS=$'\t' read -r id label tool model log start st < "$jf"
    if [ -f "${jf%.job}.done" ] && [ "$st" = "running" ]; then
      endep="$(cut -d' ' -f2 "${jf%.job}.done" 2>/dev/null)"; : "${endep:=$now}"
      el=$((endep-start)); st="finishing"; glyph='◍'
    elif [ "$st" = "running" ]; then
      el=$((now-start)); glyph='●'
    else
      if [ -f "${jf%.job}.done" ]; then
        endep="$(cut -d' ' -f2 "${jf%.job}.done" 2>/dev/null)"; : "${endep:=$now}"
        el=$((endep-start))
      else
        el=$((now-start))
      fi
      case "$st" in merged|done) glyph='✓';; failed) glyph='✗';; bounced) glyph='↺';; *) glyph='·';; esac
    fi
    row="$(printf '%s %-8s %-16.16s %s/%s  %-9s %s' "$glyph" "$id" "$label" "$tool" "$model" "$st" "$(progress_fmt_elapsed "$el")")"
    out="$out$(printf '%s' "$row" | progress_truncate "$cols")"$'\n'; nlines=$((nlines+1))
    if [ "$glyph" = '●' ] || [ "$glyph" = '◍' ]; then
      tl="$(progress_tail_log "$log")"
      out="$out$(printf '   └ %s' "$tl" | progress_truncate "$cols")"$'\n'; nlines=$((nlines+1))
    fi
  done <<EOF
$(progress_sort_jobs "$d")
EOF

  [ "${PROGRESS_LAST_LINES:-0}" -gt 0 ] && printf '\033[%dA\033[J' "$PROGRESS_LAST_LINES" >&2
  printf '%s' "$out" >&2
  PROGRESS_LAST_LINES="$nlines"
}

# Block until every tracked id has a completion sentinel. On a TTY, redraw the
# dashboard each tick; otherwise just poll quietly (event lines already emitted).
# Contract: every background job in the caller's shell must have been started via
# progress_spawn for one of these ids (the trailing `wait` reaps all children).
# Always returns 0-ish; callers should not rely on its exit status.
progress_wait() { # statedir iter budget_start budget_total -- id...
  local sdir="$1" iter="$2" bstart="$3" btot="$4"; shift 4
  [ "${1:-}" = "--" ] && shift
  local ids="$*" d id pending; d="$(progress_dir "$sdir")"
  while :; do
    pending=0
    for id in $ids; do [ -f "$d/$(progress_key "$id").done" ] || pending=1; done
    [ "${PROGRESS_TTY:-0}" = "1" ] && progress_render "$sdir" "$iter" "$bstart" "$btot"
    [ "$pending" = "0" ] && break
    sleep "${PROGRESS_REFRESH:-1}"
  done
  wait 2>/dev/null   # reap the now-finished children
}
