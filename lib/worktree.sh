# git worktree helpers for parallel workers.
wt_create() { # repo branch path
  git -C "$1" worktree add -q -b "$2" "$3" HEAD
}

# Merge branch into repo's current branch. On conflict, abort and return non-zero.
wt_merge() { # repo branch
  if git -C "$1" merge --no-edit -q "$2"; then
    return 0
  else
    git -C "$1" merge --abort 2>/dev/null
    return 1
  fi
}

wt_remove() { # repo path branch
  git -C "$1" worktree remove --force "$2" 2>/dev/null
  git -C "$1" branch -D "$3" 2>/dev/null
  return 0
}
