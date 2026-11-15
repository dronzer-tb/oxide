#!/bin/sh
# Benchmarks this working tree's whole offline pregeneration path on the bench VPS: terrain,
# decoration, heightmaps, NBT, compression and region writes, through oxide-pregen. Builds on
# the VPS like bench-vps.sh: the build holds the bench lock shared, the timed run exclusive. Prints one BENCH line (the harness's fields
# plus per-stage milliseconds) and appends it to docs/perf/bench-<name>.tsv.
#
#   scripts/bench-pregen-vps.sh [name] [radius] [threads] [extra oxide-pregen arguments...]
#   scripts/bench-pregen-vps.sh pipeline 16 8 --compression none
#
# The digest is blake3 over every written chunk's uncompressed NBT: a change that keeps it
# keeps the output byte for byte (compression aside, which it does not cover). The world goes
# to <dir>/out/pregen-world on the VPS and is replaced each run.
set -eu
cd "$(dirname "$0")/.."
name="${1:-$(git branch --show-current | tr '/' '-')}"
radius="${2:-16}"
threads="${3:-8}"
shift 3 2>/dev/null || shift $#
extra="$*"
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
line="$(ssh -o BatchMode=yes "$host" "
    set -eu
    cd '$dir'
    # Build shared, beside other tracks' builds; the timed run takes the lock alone.
    flock -s ~/oxide-perf/.lock nice -n 10 ~/.cargo/bin/cargo build --release -p oxide-pregen 2>&1 | tail -n 3
    rm -rf out/pregen-world
    flock -x ~/oxide-perf/.lock ./target/release/oxide-pregen --datapack reference --seed 12345 \
        --world out/pregen-world --center-x 0 --center-z 0 --radius $radius \
        --threads $threads --bench $extra | grep -E '^(BENCH|[0-9]+ chunks)'
" | tee /dev/stderr | grep '^BENCH')"
commit="$(git rev-parse --short HEAD)"
if [ -n "$(git status --porcelain -- crates)" ]; then
    commit="$commit+"
fi
mkdir -p docs/perf
printf '%s\t%s\t%s\t%s\n' "$(date +%s)" "$commit" "$radius" "${line#BENCH } pregen=1" >>"docs/perf/bench-$name.tsv"
