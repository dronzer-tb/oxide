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

**Reverted: the gate and `--bench-lock`** (auditor, urgent, from the user). The box is mixed:
noise and datapath take `.lock` directly and never pass the gate. `--bench-lock` loaded the
datapack before taking `.lock` and ran the 8-thread digest after dropping it, so those
phases held no lock and could land inside another track's timed run. Now:
- All three scripts use bench-vps.sh's locking exactly: build, test and profile under
  `flock -s ~/oxide-perf/.lock nice -n 10`, the whole timed process under `flock -x`. That
  covers the sub-agents too; both were told, and both now use bare flock.
- `--bench-lock` is gone from the harness. The parallel digest stays: under a whole-run
  `flock -x` it only shortens our exclusive hold. The digest is unchanged in every run.
- `scripts/bench-lock.sh` stays in the tree, marked PROPOSAL ONLY and run by nothing.
  **Proposal for the user (feat/native-decoration):** flock(1) is not fair, and a steady
  stream of shared builds starves exclusive waiters. A second lock, `.gate`, fixes that: a
  timed run holds it while it waits and runs, and a build must pass it before taking
  `.lock` shared. It works only if every script on the box adopts it at once. A fair harness
  lock would also need to hold `.lock` shared through load and digest, upgrading only for
  generation.
- `build_s` now comes from cargo's own `Finished … in` line. Before, it counted time waiting
  on the lock (one cached build showed 173 s).

**fatbench (fat LTO) A/B, partial.** Same code in every run (the commits differ only in
docs); every digest is a977961a58122047:

| profile | cps |
|---|---|
| release | 125.0, 135.4, 140.0 |
| fatbench | 145.3 |

The release runs from the same session give 135.4 and 140.0; the 125.0 run ran on a loaded
box. Against those, fatbench is +5%, inside the noise. A second fatbench run is owed: the
loop was stopped by the urgent lock change. Build time is not recorded yet; these rows
predate the fix.

**Rejected: fat LTO (`fatbench` profile), clean A/B.** Same commit 8ea2c1c, plain locking,
every digest a977961a58122047. Build times are cargo's own, from a cold build of each profile.

| profile | cps | mean | build |
|---|---|---|---|
| release (thin LTO, cgu 1) | 137.8, 136.0 | 136.9 | 2m04s |
| fatbench (fat LTO, cgu 1) | 126.5, 145.5 | 136.0 | 3m55s |

No measurable gain. Release already has codegen-units = 1 and thin LTO, so fat LTO has little
left to inline; its spread (19 cps) is bigger than any difference. It costs +1m51s per cold
build. The profile stays in Cargo.toml as opt-in and costs nothing unused, but benches stay
on release. PGO is next, with the pipeline-1 sub-agent.

**Kept: finish pass without clones** (99a1618, by the pipeline-2 sub-agent, reviewed by me).
The decorate stage split, from temporary timers (`perf` is blocked on the box:
`perf_event_paranoid=4`):

| | writes (wall ms) | finish (wall ms) | clone (CPU s) | apply (CPU s) | heightmaps (CPU s) |
|---|---|---|---|---|---|
| before, 3 runs | 5791 / 5360 / 5563 | 804 / 741 / 927 | 1.5-2.3 | 3.6-4.1 | 0.6-1.0 |
| after, 2 runs | 5489 / 5487 | 602 / 452 | 0 | 2.9-3.9 | 0.7-0.9 |

Decoration writes are **87% of the stage**: 9.23M writes, about 7.5k per source chunk. The
change moves each inner terrain chunk into its finished chunk instead of cloning it, and
stores a palette index without cloning or hashing the state when the state is already in the
section's palette. A new state is cloned in exactly where `set` would insert it, so palette
order is unchanged.

    before: decorate_ms=6687 / 6278 / 6586  (cps 61.1 / 68.0 / 63.6)
    after:  decorate_ms=6114 / 5957 / 6023  (cps 64.3 / 66.6 / 61.0)
    digest df701ee858e2bdd3 on every run

That is about -0.5 s of decorate (-8%). Whole-run cps is too noisy to show it, since generate
alone swings ±1 s. A version that moved states into the palette was slower (apply 10 s CPU,
probably cross-thread frees of 9.2M heap states) and was dropped.

**Next, in decoration (now ours):** every query and write carries a string-keyed `BlockState`.
`DecorationRegion::is` hashes a full state for each `BlockFacts::is` call, `height()`
rescans written columns, and the overlay is a SipHash map of owned states. Plan:
1. Split the writes phase by feature to find where the 5.5 s goes (pipeline-2 sub-agent, timers
   only).
2. Then intern states as u32 ids, with flat fact bits by id and a dense overlay, if the split
   says lookups dominate.

**Kept: PGO for the terrain bench** (pipeline-1 sub-agent; `scripts/bench-pgo-vps.sh`, brought
in by me with small changes). The flow: instrumented build (`-Cprofile-generate`), a training
run under the shared lock, `llvm-profdata merge`, then a `-Cprofile-use` build. Build
directories go under `target/pgo-{gen,use}`, so the release cache is untouched, and the
script repeats `target-cpu=native` because RUSTFLAGS overrides `.cargo/config.toml`. llvm-tools
were added on the box with `rustup component add llvm-tools` (no sudo). Commit baa536c, r16,
8 threads, every digest a977961a58122047:

| build | cps |
|---|---|
| release | 143.2, 136.0 (median 139.6) |
| PGO, trained on the bench workload | 163.8, 151.5 (median 157.7, **+12.9%**) |
| PGO, trained on seed 999 r8, timed on 12345 r16 | 164.2 |
| PGO + fat LTO | 150.0, 168.1 (median 159.1: nothing over plain PGO) |

PGO clears the 10% bar on two runs, and the cross-trained run says it is not fitted to the
benchmark, so the script defaults to that training (seed 999, r8). The cost is a cold build of
about 11-13 min against about 7 min for release (instrumented about 6 min, training about 1 min,
profile-use 3.5-6 min). This is the first configuration on this box past PLAN's 160.5 start
line. It changes only how the binary is built, so it adds on top of every code track; whether
the progress page counts `profile=pgo` rows as the headline is the user's call.
`-Cllvm-args=-pgo-warn-mismatch=false` is not accepted by this LLVM, and no mismatch
warnings appeared without it.

**PGO, committed script (136e3b0), trained on seed 999 r8:**

    BENCH chunks=1089 threads=8 secs=6.140 cps=177.4 ms_per_chunk_core=45.11 digest=a977961a58122047 profile=pgo build_s=3m39s+3m13s

The best terrain run on this box so far: +27% over this session's release median (139.6), but
it is one run on a quiet box, so it needs a repeat before it counts on its own. The script works
from a clean checkout; the build is 6m52s of cargo time.
