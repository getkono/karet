#!/bin/sh
# Validate a commit message against the Conventional Commits spec with convco.
#
# Invoked by the hk `commit-msg` hook with the path to the pending commit message
# ($1). Merge- and revert-in-progress commits are exempt: git generates their
# default "Merge branch ..." / "Revert ..." messages, which are not conventional
# and must not block conflict resolution.
set -eu

msg_file=$1

if [ -f "$(git rev-parse --git-path MERGE_HEAD)" ] ||
    [ -f "$(git rev-parse --git-path REVERT_HEAD)" ]; then
    exit 0
fi

# Validate just this message (not git history). --strip drops comment lines and
# trailing whitespace first, matching `git commit --cleanup=strip`.
exec convco check --from-stdin --strip < "$msg_file"
