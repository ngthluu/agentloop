#!/bin/bash
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
. "$HERE/lib.sh"
. "$ROOT/lib/worktree.sh"

ws="$(mktmpws)"; trap 'rm -rf "$ws"' EXIT
repo="$ws/repo"; mkdir -p "$repo"
git -C "$repo" init -q
git -C "$repo" config user.email t@t; git -C "$repo" config user.name t
echo base > "$repo/file.txt"
git -C "$repo" add -A && git -C "$repo" commit -qm init

# create a worktree, make a change, merge it back cleanly
wt="$ws/wt-1"
wt_create "$repo" "item/it-1" "$wt"; assert_ok $? "worktree created"
echo "feature" > "$wt/new.txt"
git -C "$wt" add -A && git -C "$wt" commit -qm "add new"
wt_merge "$repo" "item/it-1"; assert_ok $? "clean merge ok"
assert_eq "$(cat "$repo/new.txt")" "feature" "merged file present"
wt_remove "$repo" "$wt" "item/it-1"; assert_ok $? "worktree removed"

# conflicting change must fail and leave repo clean (merge aborted)
wt2="$ws/wt-2"
wt_create "$repo" "item/it-2" "$wt2"
echo "theirs" > "$wt2/file.txt"; git -C "$wt2" add -A && git -C "$wt2" commit -qm theirs
echo "ours" > "$repo/file.txt"; git -C "$repo" add -A && git -C "$repo" commit -qm ours
wt_merge "$repo" "item/it-2"; assert_fail $? "conflicting merge fails"
# repo must not be mid-merge
[ -f "$repo/.git/MERGE_HEAD" ]; assert_fail $? "merge was aborted (no MERGE_HEAD)"

test_summary
