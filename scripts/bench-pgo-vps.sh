#!/bin/sh
# Like bench-vps.sh, but with profile-guided optimisation: builds an instrumented
# oxide-harness on the VPS, runs it once to collect a profile, merges it with llvm-profdata,
# rebuilds with the profile and times that build. Prints the same BENCH line with
# profile=pgo[+<profile>] and build_s = cargo's own times for the instrumented and final
# builds (lock waits and the training run not counted).
#
#   scripts/bench-pgo-vps.sh [name] [radius] [threads]
#
# OXIDE_BENCH_PROFILE picks the cargo profile under PGO (default release).
# OXIDE_PGO_TRAIN overrides the training arguments. The default trains on a different seed and
# a smaller area than the timed run, so the profile is not fitted to the measured workload;
# it timed as fast as training on the workload itself (LOG-pipeline.md).
# Everything on the box holds ~/oxide-perf/.lock: shared at nice 10, exclusive for the timed run.
# Needs `rustup component add llvm-tools` on the VPS. Build dirs live under target/ so the
# rsync --delete in bench-vps.sh leaves them alone and the plain release cache is untouched.
set -eu
cd "$(dirname "$0")/.."
name="${1:-$(git branch --show-current | tr '/' '-')}"
radius="${2:-16}"
threads="${3:-8}"
host="${OXIDE_BENCH_HOST:-s5}"
profile="${OXIDE_BENCH_PROFILE:-release}"
train="${OXIDE_PGO_TRAIN:---seed 999 --radius 8 --threads $threads}"
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
out="$(ssh -o BatchMode=yes "$host" "
    set -eu
    cd '$dir'
    root=\$(pwd)
    profdata=\$(ls ~/.rustup/toolchains/stable-*/lib/rustlib/*/bin/llvm-profdata | head -n 1)
    # RUSTFLAGS replaces [build] rustflags from .cargo/config.toml, so repeat target-cpu=native.
    base='-C target-cpu=native'
    rm -rf \"\$root/target/pgo-data\" \"\$root/target/pgo.profdata\"
    mkdir -p \"\$root/target/pgo-data\"
    RUSTFLAGS=\"\$base -Cprofile-generate=\$root/target/pgo-data\" CARGO_TARGET_DIR=target/pgo-gen \
        flock -s ~/oxide-perf/.lock nice -n 10 ~/.cargo/bin/cargo build --profile $profile -p oxide-harness 2>&1 | tail -n 3
    flock -s ~/oxide-perf/.lock nice -n 10 ./target/pgo-gen/$profile/oxide-harness --datapack reference \
        $train --time-only | grep ^BENCH | sed 's/^BENCH/TRAIN/'
    flock -s ~/oxide-perf/.lock nice -n 10 \"\$profdata\" merge -o \"\$root/target/pgo.profdata\" \"\$root/target/pgo-data\"
    # Each PGO dir is about 1 GB and the box's disk is shared and nearly full, so delete them.
    rm -rf \"\$root/target/pgo-gen\" \"\$root/target/pgo-data\"
    RUSTFLAGS=\"\$base -Cprofile-use=\$root/target/pgo.profdata\" \
        CARGO_TARGET_DIR=target/pgo-use \
        flock -s ~/oxide-perf/.lock nice -n 10 ~/.cargo/bin/cargo build --profile $profile -p oxide-harness 2>&1 | tail -n 3
    flock -x ~/oxide-perf/.lock ./target/pgo-use/$profile/oxide-harness --datapack reference --seed 12345 \
        --radius $radius --time-only --threads $threads | grep ^BENCH
    rm -rf \"\$root/target/pgo-use\"
" | tee /dev/stderr)"
build="$(printf '%s\n' "$out" | sed -n 's/.*Finished .* in //p' | tr -d ' ' | paste -sd+ -)"
tag=pgo
[ "$profile" = release ] || tag="pgo+$profile"
line="$(printf '%s\n' "$out" | grep '^BENCH') profile=$tag build_s=$build"
echo "$line"
commit="$(git rev-parse --short HEAD)"
if [ -n "$(git status --porcelain -- crates)" ]; then
    commit="$commit+"
fi
mkdir -p docs/perf
printf '%s\t%s\t%s\t%s\n' "$(date +%s)" "$commit" "$radius" "${line#BENCH }" >>"docs/perf/bench-$name.tsv"
