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

**Kept: real pregeneration** (oxide-pregen with `--datapack`, 41f0857). Terrain at radius+2,
decoration writes at radius+1, finished chunks at radius in `decorated_chunk`'s order,
heightmaps from `oxide_chunkgen::decorate::heightmaps` (track item 4's decorated
heightmaps: done), one region write each. `scripts/bench-pregen-vps.sh [name] [radius]
[threads] [pregen args]` gives the track's number; its rows end `pregen=1` in
bench-pipeline.tsv. Digest is blake3 over every chunk's uncompressed NBT.

    BENCH chunks=1089 threads=8 secs=15.622 cps=69.7 ms_per_chunk_core=114.76 digest=df701ee858e2bdd3 generate_ms=9474 decorate_ms=5813 serialise_ms=110 compress_ms=198 write_ms=27 compression=zlib6 mb_disk=9.9
    BENCH chunks=1089 threads=8 secs=20.701 cps=52.6 ms_per_chunk_core=152.07 digest=df701ee858e2bdd3 generate_ms=12515 decorate_ms=7593 serialise_ms=299 compress_ms=266 write_ms=29 compression=zlib6 mb_disk=9.9

(The second ran while the box was still busy, before the Minecraft servers were stopped.)
Reading: at radius 16 generate is 1369 terrain chunks for 1089 finished (26% edge cost,
which shrinks with area), decoration is 37% of the path, and serialise + compress + write
together are about 2%. zlib is ~1.5 ms core per chunk, not the ~3 ms the track file
guessed. So the non-terrain win in this track is decoration, which lives in
`oxide-chunkgen/src/decorator/` (outside datapath's ownership per PLAN, and not ours).

**Kept: lock discipline** (d5f0505, 813333c). New bench-vps.sh from feat/native-decoration;
vps-cargo.sh holds the lock shared with nice 10; bench-pregen-vps.sh builds shared and
times exclusive.

**Track item 4, light: scoped, not started (report before starting, as the track file asks).**
Chunks go out with `isLightOn: 0`, Status `features`, no `SkyLight`/`BlockLight`, so the
server runs initialize_light and light on load. That keeps it correct, but it means the server
pays the light cost on first load. Doing it natively needs:
1. New data: per-state light opacity (`getLightBlock`), emission, and the occlusion faces
   for `useShapeForLightOcclusion` states (slabs, stairs and so on). `block_facts.tsv` (32,366
   states) has none of these, so `BlockFactsExporter.java` needs three more columns and a
   re-export on the server. That is user work on the server, not ours.
2. Sky light: per column, full light down to the first opaque block, then a BFS through
   transparent blocks with opacity cost. It crosses chunk borders, so a finished chunk needs
   the decorated 3x3 around it. The pregen pipeline already has that window, one stage later
   (decoration writes for radius+1, so terrain at radius+3).
3. Block light: BFS from emitters (lava, glow lichen, magma and so on), 15 blocks of reach, so
   at most one neighbour ring.
4. Output: 2048-byte nibble arrays for every section plus the one below and the one above the
   world, `isLightOn: 1`, Status `light` or later. The server must accept it without
   relighting. That needs the load test in item 5, plus a check that no lighting engine
   rejects it. The parity gate would be a vanilla light dump compared nibble for nibble; no
   such oracle exists yet.
Estimate: 2 to 3 sessions plus an exporter change. Recommend we do it only after the load test
(item 5) passes with unlit chunks.

**Track item 5, load test: steps for the user** (nothing here touches the server; I have
not run any of it):
1. On the bench box: `cd ~/oxide-perf/pipeline && ./target/release/oxide-pregen --datapack
   reference --seed 12345 --world out/loadtest --center-x 0 --center-z 0 --radius 16
   --threads 8`. That writes `out/loadtest/region/r.{-1,0}.{-1,0}.mca`, each with a
   `.mca.oxide` provenance sidecar. DataVersion 4903 comes from `reference/version.json`.
2. Make a fresh 26.2 world with **seed 12345** on a stopped test server (not `oxide-bench`),
   start it once, stop it. Then delete its `world/region/r.{-1,0}.{-1,0}.mca` and copy the four
   region files from step 1 into its place (leave out the `.oxide` sidecars). Run it without
   the Oxide module, so the server cannot regenerate anything.
3. Start the server and check the log for `Chunk ... is in the wrong location`, `Failed to
   read chunk`, or a `DataFixer` upgrade warning; there should be none. Join and fly over 0,0:
   terrain and trees should appear, light should come on in a few seconds (the server
   relights), and the chunk edges at ±16 chunks should meet the server's own chunks.
4. Report back: any of those log lines, and a screenshot at 0,0. If Status `features` is
   refused (26.2 might want `minecraft:features`), the fix is one line in
   `oxide-anvil/src/nbt.rs`.

**Measured, not applied yet: zlib backends** (OmniRoute worker gemini-3-flash wrote the
micro-bench in a scratch worktree but hit its 40-turn cap before running it; I fixed its
timing, which had included decompression, and ran it). 400 real pregen chunks, 12.8 MB NBT,
one thread on the bench box:

| backend | level | ms/chunk | ratio |
|---|---|---|---|
| miniz_oxide (flate2 default, today) | 1 / 3 / 6 | 0.26 / 0.62 / 1.25 | 3.82 / 4.76 / 4.93 |
| zlib-rs (flate2 feature, pure Rust) | 1 / 3 / 6 | 0.15 / 0.38 / 0.53 | 3.47 / 4.56 / 4.78 |
| zlib-ng | – | needs cmake, not on the box | – |

zlib-rs at level 6 is 2.4x faster than miniz at 3% larger files. Compressed bytes differ;
the pregen digest covers the uncompressed NBT, so the world is unchanged. Compression is
about 2% of the path, so the whole-path gain is at most about 1.5%. Queued as a one-line
`oxide-anvil` feature change with a pregen A/B.

**Fix:** bench-vps.sh's `rsync --delete` did not exclude `/out` and wiped the pregen world
every terrain bench. It now excludes it.

**Process:** the user has ruled OmniRoute out entirely (too slow). There will be no more
omni-agent.sh or omni-ask.sh calls on this track; parallel work goes to Opus sub-agents only.

**Kept: fair bench locking** (68df22e, 8cc6fd3), done ahead of PGO at the auditor's lead.
At 03:30 there were 11 timed runs waiting on `flock -x`, the oldest 26 minutes. There were
two causes:
1. flock is not fair, so a steady stream of shared builds kept the exclusive waiters out.
   The new `scripts/bench-lock.sh build|timed|timed-inner <cmd>` adds a second lock,
   `~/oxide-perf/.gate`. A timed run holds it from when it starts waiting until it ends. A
   build must take it before taking `.lock` shared, and drops it as soon as the shared lock
   is held. New builds now queue behind a waiting timed run; builds already running finish
   first.
2. The exclusive hold covered the whole harness process: datapack load, a 3-7 s timed section,
   then a ~25 s single-threaded digest pass. `oxide-harness --bench-lock <file>` (additive; the
   harness belongs to no track) takes the exclusive flock around `generate_all` only. The
   digest is now computed in parallel: per-chunk hashes, folded in position order, so the
   digest is the same.
`bench-vps.sh` uses `timed-inner` when the harness it built has `--bench-lock`, and plain
`timed` otherwise, so the script also works on a branch without the harness change.
`bench-pregen-vps.sh` and `vps-cargo.sh` go through the same script.

**For the owner of `feat/native-decoration`, so every track can take it:** cherry-pick
68df22e and 8cc6fd3 from `perf/pipeline`, or copy `scripts/bench-lock.sh` and
`scripts/bench-vps.sh`. The gate only works if *everything* on the box goes through
`bench-lock.sh`. A bare `flock -s ~/oxide-perf/.lock` (a hand-run test, or a sub-agent's
build) skips the gate and can still starve timed runs. The rule for all tracks becomes:
`sh scripts/bench-lock.sh build <cmd>` for any build, test or profile on the box.

**Scope change (from the user):** this track now owns decoration:
`crates/oxide-chunkgen/src/decorator/`, `decorate.rs` and `region.rs`. Datapath keeps the rest
of oxide-chunkgen, and changes there go through the auditor. Gate: the pregen digest stays
`df701ee858e2bdd3` at radius 16 unless a change is shown to be a bug fix; no float
reordering. Order of work: finish the LTO/PGO job, then decoration.
