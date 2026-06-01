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

# Wipe and recreate the job-state dir for a fresh iteration.
progress_reset() { # statedir
  local d; d="$(progress_dir "$1")"
  rm -rf "$d"; mkdir -p "$d"
  PROGRESS_LAST_LINES=0
}

# Register a running job. Parent calls this right before backgrounding the work.
progress_register() { # statedir id label tool model log
  local d; d="$(progress_dir "$1")"; mkdir -p "$d"
  printf '%s\t%s\t%s\t%s\t%s\t%s\trunning\n' \
    "$2" "$3" "$4" "$5" "$6" "$(date +%s)" > "$d/$2.job"
  [ "${PROGRESS_TTY:-0}" = "1" ] || \
    printf '%s  dispatch %-10s %s/%s  %s\n' "$(date +%H:%M:%S)" "$2" "$4" "$5" "$3" >&2
}

# Update a job's status field (id stays the same). No-op if the job is unknown.
progress_set_status() { # statedir id status
  local d f id label tool model log start st
  d="$(progress_dir "$1")"; f="$d/$2.job"
  [ -f "$f" ] || return 0
  IFS=$'\t' read -r id label tool model log start st < "$f"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$id" "$label" "$tool" "$model" "$log" "$start" "$3" > "$f"
  [ "${PROGRESS_TTY:-0}" = "1" ] || \
    printf '%s  %-9s %-10s %s/%s\n' "$(date +%H:%M:%S)" "$3" "$2" "$tool" "$model" >&2
}

# Background a command; write a "<exit_code> <end_epoch>" sentinel when it exits.
# Sentinels (not kill -0) are how progress_wait detects completion: an un-reaped
# child becomes a zombie whose pid still answers kill -0, which would hang a poll.
progress_spawn() { # statedir id -- cmd...
  local d; d="$(progress_dir "$1")"; local id="$2"; shift 2
  [ "$1" = "--" ] && shift
  ( "$@"; printf '%s %s\n' "$?" "$(date +%s)" > "$d/$id.done" ) &
}
