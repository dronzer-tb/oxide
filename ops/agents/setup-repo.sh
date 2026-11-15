#!/bin/sh
# Second `agents` step, after the deploy key exists: clone and make the track worktrees.
set -eu
export GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=accept-new"
[ -d ~/oxide/.git ] || git clone -q git@github.com:dronzer-tb/oxide.git ~/oxide
cd ~/oxide
git fetch -q origin
for t in noise datapath pipeline; do
    [ -d ~/oxide-wt/$t ] || git worktree add -q ~/oxide-wt/$t -B perf/$t origin/perf/$t
    git -C ~/oxide-wt/$t branch -q --set-upstream-to=origin/perf/$t
done
ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new bench-s5 true && echo "bench-s5 reachable"
git worktree list
