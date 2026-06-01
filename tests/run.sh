#!/bin/bash
# Runs each tests/test_*.sh in its own process; aggregates pass/fail.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
fails=0
for t in "$HERE"/test_*.sh; do
  [ -e "$t" ] || continue
  echo "== $(basename "$t") =="
  if /bin/bash "$t"; then :; else fails=$((fails+1)); fi
done
echo "================"
if [ "$fails" -eq 0 ]; then echo "ALL SUITES PASSED"; else echo "$fails SUITE(S) FAILED"; exit 1; fi
