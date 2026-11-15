#!/bin/sh
# Runs a cargo command for this working tree on the bench VPS, for machines with no C
# toolchain of their own (the agent host cannot link, so `cargo check` and `cargo test` fail
# there). Copies the sources the way bench-vps.sh does and holds the bench lock shared at
# nice 10, as every build, test or profile on the box must.
#
#   scripts/vps-cargo.sh <name> <cargo arguments...>
#   scripts/vps-cargo.sh pipeline test -p oxide-anvil
set -eu
cd "$(dirname "$0")/.."
name="$1"
shift
host="${OXIDE_BENCH_HOST:-s5}"
dir="oxide-perf/$name"
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
    ssh -o BatchMode=yes "$host" "test -e '$dir/reference' || ln -s ~/oxide-perf/reference '$dir/reference'"
fi
args=""
for a in "$@"; do
    args="$args '$(printf %s "$a" | sed "s/'/'\\\\''/g")'"
done
ssh -o BatchMode=yes "$host" "cd '$dir' && flock -s ~/oxide-perf/.lock nice -n 10 ~/.cargo/bin/cargo $args"
