# Log: pipeline track

Owner of `crates/oxide-pregen`, `crates/oxide-anvil`, `scripts/`. Newest entries at the bottom.

## 2026-11-15

**Tooling.** The agent host (oci-v3, aarch64) has no C linker, so `cargo test` cannot run
locally. Added `scripts/vps-cargo.sh <name> <cargo args>`: syncs the tree like `bench-vps.sh`
and runs cargo on the bench box under the same lock. Every `cargo test` in this log ran there.

**Baseline (terrain only, this branch at 3627ca2; no terrain code changed by this track).**

    BENCH chunks=1089 threads=8 secs=11.083 cps=98.3  ms_per_chunk_core=81.41 digest=a977961a58122047
    BENCH chunks=1089 threads=8 secs=7.930  cps=137.3 ms_per_chunk_core=58.25 digest=a977961a58122047

Digest matches PLAN. Both are under PLAN's 160.5: the box had a `java` at ~24% CPU and
another track's harness had just run. Run-to-run spread here is far above the 8% PLAN
quotes, so every comparison in this track takes at least two runs.

**Kept: oxide-anvil bulk region writer** (e0816ed, 8ebc929). `compress::Compression`
(zlib at any level, or none: scheme 3) and `region::write_region_compressed`: lays out one
whole region in memory from already-compressed chunks, one write + rename, one provenance
sidecar write, fsync optional. The old path (`update_chunk_in_place` per chunk) re-reads the
region and rewrites the sidecar with an fsync per chunk. Tests: round-trip through
`fastanvil` for zlib and none, multi-sector chunks, negative regions, wrong-region refusal.
`cargo test -p oxide-anvil` passes. No bench number yet: the pregen BENCH line that measures
it is the next commit.

**In flight:** `oxide-pregen` real generation (terrain + decoration + heightmaps + NBT +
compress + write, each stage timed) with a `BENCH` line and `scripts/bench-pregen-vps.sh`.
Output digest is blake3 over every chunk's uncompressed NBT, so any later serialiser or
pipeline change has a byte-exact gate of its own.
