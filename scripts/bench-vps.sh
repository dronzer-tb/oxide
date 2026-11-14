#!/bin/sh
# Benchmarks this working tree's terrain generation on the bench VPS (ssh host s5, 8-core
# Zen 1): copies the sources there, builds with that machine's own CPU as the target, and runs
# oxide-harness --time-only. Prints one BENCH line: chunks per second and a digest of every
# generated block.
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
dir="/root/oxide-perf/$name"
# reference/ is gitignored game data, so a git worktree has none: take it from the main checkout.
reference="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)/reference"

ssh -o BatchMode=yes "$host" "mkdir -p '$dir'"
rsync -a --delete \
    --exclude /target --exclude /.git --exclude /graphify-out --exclude /logs \
    --exclude /website --exclude /plugin --exclude /client-mod \
    --exclude /reference \
    -e "ssh -o BatchMode=yes" ./ "$host:$dir/"
rsync -a --delete \
    --exclude /oracle --exclude /decompiled --exclude '/block_survival*.tsv' \
    -e "ssh -o BatchMode=yes" "$reference/" "$host:$dir/reference/"
ssh -o BatchMode=yes "$host" "
    set -eu
    cd '$dir'
    flock /root/oxide-perf/.lock sh -c '
        ~/.cargo/bin/cargo build --release -p oxide-harness 2>&1 | tail -n 3
        ./target/release/oxide-harness --datapack reference --seed 12345 \
            --radius $radius --time-only --threads $threads | grep ^BENCH
    '
"
