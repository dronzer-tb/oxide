#!/bin/sh
# Benchmarks this working tree's terrain generation on the bench VPS (ssh host s5, 8-core
# Zen 1): copies the sources there, builds with that machine's own CPU as the target, and runs
# oxide-harness --time-only. Prints one BENCH line: chunks per second and a digest of every
# generated block, and appends it to docs/perf/bench-<name>.tsv. Commit and push that file with
# your log: the progress page reads it from GitHub.
#
#   scripts/bench-vps.sh [name] [radius] [threads]
#
# `name` keeps each caller's sources and build cache apart on the VPS (default: the current
# branch). One run at a time across all callers -- the lock is on the VPS -- so numbers from
# parallel work do not disturb each other. Stop anything heavy on the VPS before trusting a
# number; the Minecraft bench server idles at under 1% CPU and is fine to leave up.
set -eu
cd "$(dirname "$0")/.."
name="${1:-$(git branch --show-current | tr '/' '-')}"
radius="${2:-16}"
threads="${3:-8}"
host="${OXIDE_BENCH_HOST:-s5}"
# Cargo profile to build with: release by default; `fatbench` (Cargo.toml) adds fat LTO.
profile="${OXIDE_BENCH_PROFILE:-release}"
# Relative to the remote home: /root for the laptop (host s5), /home/bench for the agents on
# oci-v3 (host bench-s5).
dir="oxide-perf/$name"
# reference/ is gitignored game data, so a git worktree has none: take it from the main checkout.
reference="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)/reference"

ssh -o BatchMode=yes "$host" "mkdir -p '$dir'"
rsync -a --delete \
    --exclude /target --exclude /.git --exclude /graphify-out --exclude /logs \
    --exclude /website --exclude /plugin --exclude /client-mod \
    --exclude /reference --exclude /out \
    -e "ssh -o BatchMode=yes" ./ "$host:$dir/"
if [ -d "$reference" ]; then
    rsync -a --delete \
        --exclude /oracle --exclude /decompiled --exclude '/block_survival*.tsv' \
        -e "ssh -o BatchMode=yes" "$reference/" "$host:$dir/reference/"
else
    # No game data on this machine (the agent host): use the copy kept on the bench box.
    ssh -o BatchMode=yes "$host" "test -e '$dir/reference' || ln -s ~/oxide-perf/reference '$dir/reference'"
fi
out="$(ssh -o BatchMode=yes "$host" "
    set -eu
    cd '$dir'
    # Builds share the lock, so several tracks compile at once; the timed run takes it alone,
    # so it waits for builds in progress and nothing else runs while it is measured. Any other
    # build or profile on this box must also hold it shared: flock -s ~/oxide-perf/.lock ...
    flock -s ~/oxide-perf/.lock nice -n 10 ~/.cargo/bin/cargo build --profile $profile -p oxide-harness 2>&1 | tail -n 3
    flock -x ~/oxide-perf/.lock ./target/$profile/oxide-harness --datapack reference --seed 12345 \
        --radius $radius --time-only --threads $threads | grep ^BENCH
" | tee /dev/stderr)"
# Cargo's own build time (from its Finished line), so time spent waiting on the lock is not
# counted; a cached build reports well under a second.
build="$(printf '%s\n' "$out" | sed -n 's/.*Finished .* in //p' | tr -d ' ')"
line="$(printf '%s\n' "$out" | grep '^BENCH') profile=$profile build_s=$build"
# unix time, commit (with + when the tree has uncommitted changes), radius, then the BENCH fields.
commit="$(git rev-parse --short HEAD)"
if [ -n "$(git status --porcelain -- crates)" ]; then
    commit="$commit+"
fi
mkdir -p docs/perf
printf '%s\t%s\t%s\t%s\n' "$(date +%s)" "$commit" "$radius" "${line#BENCH }" >>"docs/perf/bench-$name.tsv"
