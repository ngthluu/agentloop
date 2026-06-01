#!/bin/bash
# OPT-IN: actually spends tokens. Builds a tiny real app end-to-end with the real CLIs.
# Not run by tests/run.sh (that only globs test_*.sh).
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
ws="$(mktemp -d "${TMPDIR:-/tmp}/agentloop-live.XXXXXX")"
echo "workspace: $ws"
"$ROOT/agentloop.sh" \
  "Create a Python CLI 'addcli' that adds two integers from argv and prints the sum, with a pytest test that passes. Provide a verify.sh that runs pytest." \
  --workspace "$ws" --max-iterations 8
rc=$?
echo "rc=$rc"
echo "--- master.md ---"; cat "$ws/.agentloop/state/master.md"
echo "--- tree ---"; ls -R "$ws" | head -50
exit "$rc"
