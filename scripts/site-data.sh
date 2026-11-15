#!/bin/sh
# Regenerates website/lab/data-local (full, never published): one run file per seed and
# dimension under reference/oracle, plus the workspace test results. Run from anywhere; needs
# the oracle dumps to exist.
# PUBLIC=1 writes website/lab/data instead, without the game's own world: the only form to host.
# The page reads data-local when it is there and data otherwise.
# With an Overforge binary (OVERFORGE, default below) it also draws each run's Oxide world as an
# isometric tile map under website/lab/iso, from region files written to target/site-world.
set -eu
cd "$(dirname "$0")/.."
out="website/lab/data-local"
radius="${1:-12}"
commit="$(git rev-parse --short HEAD)"
if [ -n "$(git status --porcelain -- crates)" ]; then
    commit="$commit+dirty"
fi
public=""
if [ "${PUBLIC:-0}" = 1 ]; then
    public="--site-public"
    out="website/lab/data"
fi
overforge="${OVERFORGE:-$HOME/Projects/OVERFORGE/overforge-rs/target/release/overforge}"
mkdir -p "$out"
cargo build --release -p oxide-harness
for dir in reference/oracle/*/*/; do
    seed="$(basename "$(dirname "$dir")")"
    dimension="minecraft:$(basename "$dir")"
    name="${seed}_$(basename "$dir")"
    world="target/site-world/$name"
    rm -rf "$world"
    # A failed comparison still writes its run file, so one bad dimension does not stop the rest.
    ./target/release/oxide-harness --datapack reference --seed "$seed" \
        --dimension "$dimension" --radius "$radius" --oracle reference/oracle \
        --site "$out" --site-commit "$commit" --site-world "$world" $public >/dev/null ||
        echo "comparison failed: $seed $dimension" >&2
    if [ -x "$overforge" ] && [ -d "$world/region" ]; then
        roof=""
        if [ "$(basename "$dir")" = the_nether ]; then
            roof="--cut-roof"
        fi
        "$overforge" --world "$world" --output "website/lab/iso/$name" $roof
    fi
done
{
    echo "commit $commit"
    cargo test --workspace --no-fail-fast 2>&1 |
        grep -E '^(test |     Running |   Doc-tests )' || true
} >"$out/tests.txt"
