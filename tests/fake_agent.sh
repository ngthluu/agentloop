#!/bin/bash
# Stand-in for claude/codex when FAKE_AGENT=1. Behavior driven by env:
#   FAKE_SLEEP  - seconds to sleep before exiting (default 0)
#   FAKE_EXIT   - exit code (default 0)
# Echoes its argv so tests can assert command construction.
echo "FAKE_ARGS: $*"
sleep "${FAKE_SLEEP:-0}"
exit "${FAKE_EXIT:-0}"
