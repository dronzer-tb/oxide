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
# Each run is wrapped in /usr/bin/time and its row gains `cpu_user_s=` and `cpu_sys_s=`. The
# box's java processes are read from /proc/<pid>/stat (read only) before and after each run,
# and the row gains `java_n=`, `java_cpu_s=` (java CPU seconds during that run) and
# `java_cpu_s_per_10s=`. Rounds whose java load is above about 2 CPU-s per 10 s are dropped
# from decisions (auditor's rule while the Java server is on the box).
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
        for n in "$@"; do
            cd ~/oxide-perf/"$n"
            mkdir -p out
            jn="$(pgrep -cx java || true)"
            a="$(java_ticks)"
            /usr/bin/time -o out/ab-time -f '%U %S' \
                ./target/release/oxide-pregen --datapack reference --seed 12345 --world out/ab-world \
                --center-x 0 --center-z 0 --radius "$radius" --threads "$threads" --bench --no-write \
                >out/ab-bench
            b="$(java_ticks)"
            read -r cu cs <out/ab-time
            grep ^BENCH out/ab-bench | awk -v n="$n" -v cu="$cu" -v cs="$cs" -v jn="${jn:-0}" \
                -v jt="$((b - a))" -v hz="$hz" '{
                    secs = 0
                    for (i = 1; i <= NF; i++) if ($i ~ /^secs=/) secs = substr($i, 6) + 0
                    js = jt / hz
                    printf "AB %s %s cpu_user_s=%s cpu_sys_s=%s java_n=%s java_cpu_s=%.2f java_cpu_s_per_10s=%.2f\n",
                        n, $0, cu, cs, jn, js, (secs > 0 ? js * 10 / secs : 0)
                }'
            rm -rf out/ab-world out/ab-time out/ab-bench
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
