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
#
# At the start of every round it samples the box's java processes over one second from
# /proc/<pid>/stat (read only) and adds `java_n=` and `java_cpu_pct=` (percent of one core)
# to that round's rows, so a hold taken while the Java server is busy shows it.
set -eu
cd "$(dirname "$0")/.."
radius="$1"; threads="$2"; rounds="$3"; shift 3
host="${OXIDE_BENCH_HOST:-s5}"
names="$*"
out="$(ssh -o BatchMode=yes "$host" sh -s -- "$radius" "$threads" "$rounds" $names <<'REMOTE'
set -eu
radius="$1"; threads="$2"; rounds="$3"; shift 3
cd ~/oxide-perf
for n in "$@"; do test -x "$n/target/release/oxide-pregen"; done
hz="$(getconf CLK_TCK)"
java_ticks() {
    t=0
    for p in $(pgrep -x java || true); do
        s="$(cat "/proc/$p/stat" 2>/dev/null)" || continue
        set -- ${s#*) }
        t=$((t + ${12} + ${13}))
    done
    echo "$t"
}
(
    flock -x 9
    i=0
    while [ "$i" -lt "$rounds" ]; do
        i=$((i + 1))
        jn="$(pgrep -cx java || true)"
        a="$(java_ticks)"; sleep 1; b="$(java_ticks)"
        jp=$(( (b - a) * 100 / hz ))
        for n in "$@"; do
            cd ~/oxide-perf/"$n"
            ./target/release/oxide-pregen --datapack reference --seed 12345 --world out/ab-world \
                --center-x 0 --center-z 0 --radius "$radius" --threads "$threads" --bench --no-write \
                | grep ^BENCH | sed "s/^/AB $n /; s/\$/ java_n=${jn:-0} java_cpu_pct=$jp/"
            rm -rf out/ab-world
        done
    done
) 9>>~/oxide-perf/.lock
REMOTE
)"
mkdir -p docs/perf
printf '%s\n' "$out" | while read -r _ n rest; do
    echo "AB $n $rest"
    printf '%s\t%s\t%s\t%s\n' "$(date +%s)" "$n" "$radius" "${rest#BENCH }" >>docs/perf/bench-pipeline-ab.tsv
done
