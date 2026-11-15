#!/bin/sh
# Times several already-built oxide-pregen trees against each other in ONE exclusive hold,
# alternating them round by round, so box load hits every variant alike. Each <name> is a
# bench dir ~/oxide-perf/<name> whose target/release/oxide-pregen is built, e.g. with
# `scripts/vps-cargo.sh <name> build --release -p oxide-pregen` from that tree.
#
#   scripts/bench-ab-pregen-vps.sh <radius> <threads> <rounds> <name> <name> [<name>...]
#
# Prints one `AB <name> BENCH...` line per run and appends each to
# docs/perf/bench-pipeline-ab.tsv as `time  <name>  radius  fields`. Writes no region files
# (--no-write), so it measures generation, decoration, serialise and compress only.
set -eu
cd "$(dirname "$0")/.."
radius="$1"; threads="$2"; rounds="$3"; shift 3
host="${OXIDE_BENCH_HOST:-s5}"
names="$*"
out="$(ssh -o BatchMode=yes "$host" "
    set -eu
    cd ~/oxide-perf
    for n in $names; do test -x \$n/target/release/oxide-pregen; done
    flock -x ~/oxide-perf/.lock sh -c 'for i in \$(seq $rounds); do for n in $names; do
        cd ~/oxide-perf/\$n
        ./target/release/oxide-pregen --datapack reference --seed 12345 --world out/ab-world \
            --center-x 0 --center-z 0 --radius $radius --threads $threads --bench --no-write \
            | grep ^BENCH | sed \"s/^/AB \$n /\"
        rm -rf out/ab-world
    done; done'
")"
mkdir -p docs/perf
printf '%s\n' "$out" | while read -r _ n rest; do
    echo "AB $n $rest"
    printf '%s\t%s\t%s\t%s\n' "$(date +%s)" "$n" "$radius" "${rest#BENCH }" >>docs/perf/bench-pipeline-ab.tsv
done
