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
