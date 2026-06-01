# Sourced by every test_*.sh. Provides assertions + temp workspace helpers.
TESTS_RUN=0
TESTS_FAIL=0

assert_eq() { # actual expected msg
  TESTS_RUN=$((TESTS_RUN+1))
  if [ "$1" != "$2" ]; then
    echo "  FAIL: $3: expected [$2] got [$1]"; TESTS_FAIL=$((TESTS_FAIL+1))
  fi
}
assert_contains() { # haystack needle msg
  TESTS_RUN=$((TESTS_RUN+1))
  case "$1" in
    *"$2"*) ;;
    *) echo "  FAIL: $3: [$1] missing [$2]"; TESTS_FAIL=$((TESTS_FAIL+1));;
  esac
}
assert_ok()   { TESTS_RUN=$((TESTS_RUN+1)); [ "$1" -eq 0 ] || { echo "  FAIL: $2: exit $1 != 0"; TESTS_FAIL=$((TESTS_FAIL+1)); }; }
assert_fail() { TESTS_RUN=$((TESTS_RUN+1)); [ "$1" -ne 0 ] || { echo "  FAIL: $2: expected non-zero"; TESTS_FAIL=$((TESTS_FAIL+1)); }; }

test_summary() { echo "  ran $TESTS_RUN, failed $TESTS_FAIL"; [ "$TESTS_FAIL" -eq 0 ]; }
mktmpws() { mktemp -d "${TMPDIR:-/tmp}/agentloop.XXXXXX"; }
