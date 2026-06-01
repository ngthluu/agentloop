#!/bin/bash
set -u
AGENTLOOP_HOME="$(cd "$(dirname "$0")" && pwd)"
export AGENTLOOP_HOME
. "$AGENTLOOP_HOME/lib/config.sh"
. "$AGENTLOOP_HOME/lib/loop.sh"

usage() {
  cat <<EOF
Usage: agentloop.sh "<goal prompt>" [options]
  --workspace <dir>     target dir (default: current dir)
  --config <path>       config.yaml (default: <workspace>/.agentloop/config.yaml)
  --fresh               wipe existing .agentloop state and start over
  --max-iterations N    override caps.max_iterations
  --dry-run             plan only; do not dispatch workers
EOF
}

GOAL=""; WORKSPACE="$PWD"; CONFIG=""; FRESH=0; OVR_MAXIT=""; DRYRUN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --workspace) WORKSPACE="$2"; shift 2;;
    --config) CONFIG="$2"; shift 2;;
    --fresh) FRESH=1; shift;;
    --max-iterations) OVR_MAXIT="$2"; shift 2;;
    --dry-run) DRYRUN=1; shift;;
    -h|--help) usage; exit 0;;
    -*) echo "unknown option $1" >&2; usage; exit 2;;
    *) GOAL="$1"; shift;;
  esac
done
[ -n "$GOAL" ] || { echo "ERROR: goal prompt required" >&2; usage; exit 2; }

mkdir -p "$WORKSPACE"
WORKSPACE="$(cd "$WORKSPACE" && pwd)"
META="$WORKSPACE/.agentloop"
[ "$FRESH" = "1" ] && rm -rf "$META"
mkdir -p "$META/state" "$META/results" "$META/logs" "$META/worktrees"

# git repo is required for worktrees
[ -d "$WORKSPACE/.git" ] || git -C "$WORKSPACE" init -q
git -C "$WORKSPACE" config user.email >/dev/null 2>&1 || git -C "$WORKSPACE" config user.email agentloop@local
git -C "$WORKSPACE" config user.name  >/dev/null 2>&1 || git -C "$WORKSPACE" config user.name  agentloop
grep -q '^.agentloop/$' "$WORKSPACE/.gitignore" 2>/dev/null || echo '.agentloop/' >> "$WORKSPACE/.gitignore"
# ensure at least one commit exists so `worktree add HEAD` works
git -C "$WORKSPACE" rev-parse HEAD >/dev/null 2>&1 || { git -C "$WORKSPACE" add -A; git -C "$WORKSPACE" commit -qm "agentloop: initial commit"; }

[ -n "$CONFIG" ] || CONFIG="$META/config.yaml"
[ -f "$CONFIG" ] || cp "$AGENTLOOP_HOME/templates/config.yaml" "$CONFIG"
[ -f "$META/state/master.md" ] || cp "$AGENTLOOP_HOME/templates/master.md" "$META/state/master.md"
[ -f "$META/state/goal.md" ] || printf '%s\n' "$GOAL" > "$META/state/goal.md"
[ -f "$META/state/backlog.json" ] || echo '{"items":[]}' > "$META/state/backlog.json"

CFG_JSON="$(config_to_json "$CONFIG")"
[ -n "$OVR_MAXIT" ] && CFG_JSON="$(printf '%s' "$CFG_JSON" | jq --argjson v "$OVR_MAXIT" '.caps.max_iterations=$v')"

# Graceful shutdown: kill children, flush.
trap 'echo "interrupted; stopping" >&2; pkill -P $$ 2>/dev/null; exit 130' INT TERM

if [ "$DRYRUN" = "1" ]; then
  planner_run "$CFG_JSON" "$WORKSPACE" "$META/logs/dryrun-planner.log" "$(config_cap "$CFG_JSON" item_timeout_sec)"
  echo "dry-run: planned backlog ->"; jq . "$META/state/backlog.json"
  exit 0
fi

loop_run "$CFG_JSON" "$WORKSPACE"
rc=$?
echo "=== agentloop finished (rc=$rc). See $META/state/master.md ===" >&2
exit "$rc"
