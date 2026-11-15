#!/bin/sh
# Runs a command on the bench box under its locks, fairly. Runs on the box, from a synced tree.
#
#   scripts/bench-lock.sh build <command...>   build, test or profile: shared, at nice 10
#   scripts/bench-lock.sh timed <command...>   a timed run: exclusive for the whole command
#   scripts/bench-lock.sh timed-inner <command...>
#                                              a timed run whose command takes the exclusive
#                                              lock itself (oxide-harness --bench-lock)
#
# Two locks under ~/oxide-perf. `.lock` is the bench lock: shared by builds, exclusive for a
# timed run. flock(1) is not fair, so with builds arriving all the time an exclusive waiter
# can starve; `.gate` fixes that. A timed run holds the gate from when it starts waiting until
# it ends, and a build must pass through the gate before it takes `.lock` shared. A waiting
# timed run therefore stops new builds from starting; builds already running finish, then it
# runs. The gate is released by a build as soon as its shared lock is held.
#
# Anything that takes `.lock` directly bypasses the gate and can still starve a timed run, so
# every script on the box should come through here.
set -eu
mode="${1:?usage: bench-lock.sh build|timed|timed-inner <command...>}"
shift
locks="$HOME/oxide-perf"
exec 8>>"$locks/.gate"
case "$mode" in
build)
    flock -x 8
    exec 9>>"$locks/.lock"
    flock -s 9
    flock -u 8
    exec 8>&-
    exec nice -n 10 "$@"
    ;;
timed)
    flock -x 8
    exec 9>>"$locks/.lock"
    flock -x 9
    exec "$@"
    ;;
timed-inner)
    flock -x 8
    exec "$@"
    ;;
*)
    echo "bench-lock.sh: unknown mode $mode" >&2
    exit 2
    ;;
esac
