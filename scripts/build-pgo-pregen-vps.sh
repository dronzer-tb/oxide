#!/bin/sh
# Builds a profile-guided oxide-pregen on the VPS from an existing bench dir's sources, so the
# result can be timed against the plain build with bench-ab-pregen-vps.sh:
#
#   scripts/build-pgo-pregen-vps.sh <src-name> <dst-name>
#   scripts/bench-ab-pregen-vps.sh 16 8 6 <src-name> <dst-name>
#
# ~/oxide-perf/<dst-name> is recreated from ~/oxide-perf/<src-name> (sources only; reference is
# symlinked). Training is decorated pregen on a different seed and a smaller area than the timed
# run (seed 999, r8, 8 threads, --no-write), so the profile is not fitted to the measured
# workload. The PGO binary lands at <dst-name>/target/release/oxide-pregen; the instrumented
# build and raw profiles are deleted (the box's disk is shared).
# Every step holds ~/oxide-perf/.lock shared at nice 10. Needs `rustup component add llvm-tools`.
set -eu
src="$1"; dst="$2"
host="${OXIDE_BENCH_HOST:-s5}"
ssh -o BatchMode=yes "$host" sh -s -- "$src" "$dst" <<'REMOTE'
set -eu
src="$1"; dst="$2"
cd ~/oxide-perf
test -d "$src/crates"
rm -rf "$dst" && mkdir "$dst"
(cd "$src" && tar cf - --exclude=./target --exclude=./out --exclude=./reference .) | (cd "$dst" && tar xf -)
ln -s ~/oxide-perf/reference "$dst/reference"
cd "$dst"; root=$(pwd)
profdata=$(ls ~/.rustup/toolchains/stable-*/lib/rustlib/*/bin/llvm-profdata | head -n 1)
# RUSTFLAGS replaces [build] rustflags from .cargo/config.toml, so repeat target-cpu=native.
base='-C target-cpu=native'
mkdir -p target/pgo-data out
RUSTFLAGS="$base -Cprofile-generate=$root/target/pgo-data" CARGO_TARGET_DIR=target/pgo-gen \
    flock -s ~/oxide-perf/.lock nice -n 10 ~/.cargo/bin/cargo build --release -p oxide-pregen 2>&1 | tail -n 1
flock -s ~/oxide-perf/.lock nice -n 10 ./target/pgo-gen/release/oxide-pregen --datapack reference \
    --seed 999 --world out/train --center-x 0 --center-z 0 --radius 8 --threads 8 --bench --no-write \
    | grep ^BENCH | sed 's/^BENCH/TRAIN/'
flock -s ~/oxide-perf/.lock nice -n 10 "$profdata" merge -o "$root/target/pgo.profdata" "$root/target/pgo-data"
rm -rf target/pgo-gen target/pgo-data out/train
RUSTFLAGS="$base -Cprofile-use=$root/target/pgo.profdata" \
    flock -s ~/oxide-perf/.lock nice -n 10 ~/.cargo/bin/cargo build --release -p oxide-pregen 2>&1 | tail -n 1
REMOTE
