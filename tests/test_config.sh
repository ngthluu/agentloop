#!/bin/bash
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
. "$HERE/lib.sh"
. "$ROOT/lib/config.sh"

ws="$(mktmpws)"; trap 'rm -rf "$ws"' EXIT
cat > "$ws/c.yaml" <<'YAML'
caps: { max_parallel: 3 }
routing:
  build: { tool: codex, model: gpt-5, effort: high, flags: "--x" }
  fix:   { tool: claude, model: sonnet, effort: medium, flags: "" }
defaults: { role: build }
YAML

json="$(config_to_json "$ws/c.yaml")"
assert_contains "$json" '"max_parallel"' "yaml converts to json"

assert_eq "$(config_role_field "$json" build tool)"   "codex"  "build.tool"
assert_eq "$(config_role_field "$json" build effort)" "high"   "build.effort"
assert_eq "$(config_role_field "$json" fix model)"    "sonnet" "fix.model"
# empty-string field and absent field both resolve to "" (callers treat them identically)
assert_eq "$(config_role_field "$json" fix flags)"    ""       "fix.flags empty string -> empty"
assert_eq "$(config_role_field "$json" zzz tool)"     ""       "absent role field -> empty"
# unknown role falls back to defaults.role
assert_eq "$(config_resolve_role "$json" zzz)"        "build"  "unknown role -> default"
assert_eq "$(config_resolve_role "$json" fix)"        "fix"    "known role kept"

test_summary
