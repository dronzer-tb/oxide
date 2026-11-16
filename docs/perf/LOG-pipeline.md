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

**Kept: zlib-rs backend** (ff3f031). One line in oxide-anvil's flate2 features. Pregen r16,
digest df701ee858e2bdd3 on both sides:

    before (99a1618): compress_ms=291 / 216
    after  (ff3f031): compress_ms=139 / 99     mb_disk 9.9 -> 10.1

About 2x on compression, about -0.13 s per run; the whole-run cps sits inside the noise.

**Decoration profile** (pipeline-2 sub-agent, temporary timers sampled 1 in 61; patch and raw
output in the session scratchpad, not committed). Decoration CPU is 42.5 s for r16 (1225
`decorate()` calls), 99.3% of it inside features:
- **sculk_patch_deep_dark: 48%** (505 runs, about 40 ms each). `state_at` clones a full
  `BlockState` on every read, and `in_tag` / `is_inhibitor` build a `ResourceLocation` (two
  Strings) on every call, 243 calls per `can_place_growth`.
- Then ore_iron_upper 5.2%, ore_clay 4.5%, ore_tuff 3.7%, lush caves 3.5%. Upper ores call
  `height` about once per block.
- Primitives: `BlockFacts::is` 72M calls, `region.block` 92M, `region.is` 37M,
  `set_block` 10M, `height` 21M. Every one hashes a string-keyed `BlockState` or a
  `BlockPos` with std SipHash. The sampled ns/call figures include `Instant` overhead and
  run high.

**Kept: ahash for the region overlay, `written_top` and `BlockFacts`** (bc0265c). Nothing
iterates any of these maps, so the hasher cannot change output. `BlockFacts` is in
oxide-datapack, which no track owns; the change is one private field type plus the ahash dep.
Tests: oxide-chunkgen 189 + oxide-datapack, all pass.

    pregen before (99a1618/ff3f031): decorate_ms=6023 7273 5988 6198   median ~6110
    pregen after  (bc0265c):         decorate_ms=5966 5203 4615 5806   median  5505  (-10%)
    terrain digest after: a977961a58122047 (cps 132.9)

The profile's sampled per-call costs implied 1.2-1.7 s; the measured saving is about 0.6 s.
Hashing was real but smaller than the timers made it look. In flight: sculk by reference
with tags resolved once (pipeline-2).

**Kept: sculk by reference, tags resolved once** (9accc32, by the pipeline-2 sub-agent,
reviewed by me). It covers `cave/sculk.rs` only. The four tag sets are resolved once per
`place()` into a `Spreader`. States are borrowed, not cloned; each borrowed read either comes
before the same check it fed before, or is re-read after a call that only touches the
unsupported tally. Snapshots that are read across writes (`update`, `attempt_spread_vein`,
`on_discharged`) keep their clone. Tests: oxide-chunkgen 189, all pass. With the timers on,
sculk CPU went 20.5 s -> 13.4 s with **identical set_block (524,495) and fact-query
(41,666,003) counts**.

    sub-agent, on ff3f031:  decorate_ms=7218 5433 6287 (baseline) -> 4781 4470 (change)
    mine, 9accc32 (sculk + ahash):  decorate_ms=3753 4307   cps 77.0 / 71.9
    digest df701ee858e2bdd3 on every run

**Pregen path now** (median of the last two runs): about 74 cps; decorate about 4.0 s, down
from about 6.5 s this morning (-38%). Generate (terrain, about 10.3 s for 1369 chunks) is now
71% of the path, and it belongs to the noise and datapath tracks.

**PR for the user** (PLAN rule 4; there is no `gh` on the agent host, so I can't open it):
https://github.com/dronzer-tb/oxide/compare/feat/native-decoration...perf/pipeline. The branch
has feat/native-decoration merged in (29bec3c) and merges cleanly. Kept wins in it:
- real pregen, with a bulk region writer
- zlib-rs
- a clone-free finish pass
- ahash for the decoration maps
- sculk by reference
- PGO bench script
- parallel harness digest
Target from the progress page: PumpkinMC terrain at about 404 cps on this box (17.42 ms/chunk
single-thread, x7.04 for 8 threads, not byte exact). Our best terrain run is PGO at 177.4.

**Kept: per-state fact cache in the decoration region** (c585498, by the pipeline-2 sub-agent,
reviewed and test-run by me). The re-profile at 9accc32 showed `BlockFacts::is` still 54% of
decoration CPU even with ahash: 72.2M lookups at about 190 ns, each hashing a whole state.
- Each terrain section palette entry, and each written block, gets a lazily filled
  `Cell<u64>` (bit f = asked, bit f+32 = answer); `fact_in` / `fact_at` ask through it. Real
  table lookups dropped 72.2M -> 5.7M.
- The overlay maps position -> index into a write list kept in first-write order, so
  `into_writes` returns that list instead of draining 9.2M map entries (2.0 s CPU -> 0).
- Assumption: the region's facts are the decorator's own. All three production callers pass
  `decorator.facts()` (`&env.facts`). Tests: oxide-chunkgen 189, all pass.

    sub-agent:  9accc32 decorate_ms=4058 5051 -> change 2985 3008 (cps 81.4-82.4)
    mine:       c585498 decorate_ms=3179 3742  cps 83.8 / 76.2
    digest df701ee858e2bdd3 on every run

Decorate is now about 3.4 s, against about 6.5 s at the start of the day (-48%). The whole
pregen path runs at about 80 cps, against 65-70. Next: `set_block` (now the top primitive,
about 570 ns/call) and `height()`'s per-call `HashMap` lookup of the chunk heightmap.

**Kept: dense `written_top` and a per-region heightmap table** (4aca951, pipeline-2 sub-agent;
region.rs only). `written_top` is now a 48x48 array (i32::MIN = never written); the write
buffers reserve 8192 entries; `height()` indexes the nine chunks' heightmaps, resolved once in
`new()`, with no HashMap lookup per call. Same heights and writes; tests 189 pass on this
exact patch.

    sub-agent: c585498 decorate_ms=3116 3610 -> change 2815 3064 (both runs below both baselines)
    mine:      4aca951 decorate_ms=3250 (second run queued)
    digest df701ee858e2bdd3

A small gain, about 0.2-0.4 s, close to the noise; kept because it is exact and simpler.

**Out-of-scope edit, stated plainly (auditor):** bc0265c touched `crates/oxide-datapack`,
which this track does not own: `Cargo.toml` (+`ahash.workspace = true`) and
`src/block_facts.rs` (the private `flags` / `defaults` maps go from std `HashMap` to
`AHashMap`, plus a comment). The change is additive: no public item is renamed or reshaped,
the API is the same, and nothing iterates those maps, so no output can change. Terrain digest
after it: a977961a58122047.

**In flight (auditor's three asks):** scratch merge of this branch with perf/datapath
0e3cf61 (bf28e89 in /tmp/pipeline-merge; one conflict, bench-vps.sh, resolved with ours;
chunkgen 190 + pregen tests pass). The queue on it: pregen x2 (gate df701ee858e2bdd3),
terrain release x2, PGO x2. On this branch's HEAD: release x2 and PGO x2, same commit.

4aca951's second run: decorate_ms=3199, cps 87.0, digest df701ee858e2bdd3. That is the best
pregen run so far.

**Rejected: skip the overlay for columns never written, plus sculk fact checks through
`fact_in`** (pipeline-2 sub-agent, local WIP cfcfd97, not on the branch). It is exact: every
write raises its column's top. But it gives no measurable gain:

    change  cfcfd97: decorate_ms=3081 3212   (mean 3147)
    HEAD    4aca951: decorate_ms=3288 3205   (mean 3247)   digest df701ee858e2bdd3 on all

With ahash an overlay miss was already cheap; `block()`'s remaining ~83 ns is the terrain read
(chunk -> section -> palette). Profile at this point (timers on, decoration CPU 23.7 s):
- `set_block` is the top primitive (10.3M calls, ~565 ns, 24.5%). The stone-type ores (clay,
  tuff, granite, diorite, andesite: 6-8% each) are almost all `set_block`.
- Sculk is down to 24% (from 48%), mostly reads and face/tag logic now.
- Real `BlockFacts` lookups are down to 5.5M (5%), and `height()` to 98 ns per call.
The next real lever is probably the cost of owned `BlockState`s, allocated by the caller and
dropped on overwrite: interning states as ids. That is a structural change and comes after the
merge numbers show decoration's true share.

## Merge with datapath terrain (auditor's asks 1 and 2)

Scratch merge bf28e89 = this branch 4aca951 + perf/datapath 0e3cf61 (not pushed; rows in
`bench-pipeline-merge.tsv`, kept out of `bench-pipeline.tsv` so the page does not read the
merge's terrain as this branch's). All digests held: pregen df701ee858e2bdd3, terrain
a977961a58122047.

    merge pregen   cps 184.3 171.5   generate_ms 2915 2787   decorate_ms 2797 3341
    merge release  cps 632.2 657.6
    merge pgo      cps 593.3 684.9
    HEAD  release  cps 135.9 130.5   (7252db9)
    HEAD  pgo      cps 167.7 159.4   (second row tagged a91e803: docs-only commit, same code)

- **Decoration share:** with ~4.5x faster terrain, pregen goes from ~85 to ~178 cps, and decoration is now
  about half the pregen wall time (2.8-3.3 s of ~6 s). It is the main pregen lever now.
- **PGO depends on the code:** on HEAD (old terrain), PGO gains +23% over the same-commit release (163.6 vs
  133.2 mean). On the merge it is a wash, 639 vs 645 mean, with one run either side of release. The
  datapath work seems to have taken the wins PGO was finding (branch layout and inlining in
  the noise and density paths). Recommendation: do not make `profile=pgo` the headline. Re-test PGO
  on the next merge (origin/perf/noise e6ecc65, which has both terrain tracks).

Next: a fresh sub-agent (pipeline-2) is splitting `set_block`'s cost and cutting owned
`BlockState` traffic. The next scratch merge will be based on origin/perf/noise.

## Merge with origin/perf/noise (both terrain tracks)

Scratch merge a1f3a32 = 27e2f9b + origin/perf/noise e6ecc65 (bench-vps.sh conflict resolved
with ours; not pushed; rows in `bench-pipeline-merge2.tsv`). Tests pass (chunkgen + pregen,
release). Digests held.

    pregen   cps 194.2 183.4   generate_ms 2371 2664   decorate_ms 3052 3081
    release  cps 605.6 695.1

Decoration (about 3.05 s) now takes more time than terrain generation (2.4-2.7 s) in pregen.

**Disk incident:** the first two PGO builds failed with `No space left on device` (box at
99%, 2.8 GB free). I deleted only this track's finished outputs (~9 GB): `pipeline-merge/`,
plus the fatbench, pgo-gen, pgo-use and debug dirs under `pipeline/target`. The box is back to 12 GB free. It
is still 94% full. ~/oxide-perf holds 73 GB, mostly the other tracks' numbered scratch dirs at
about 2 GB each, and I left those alone. PGO scratch dirs are now deleted after each run.

## PGO: one alternating hold at 1 thread (auditor's test)

Noise merge a1f3a32, r16, 1 thread, release and PGO alternating in one exclusive hold
(`OXIDE_PGO_AB=5 scripts/bench-pgo-vps.sh pipeline-merge2 16 1`; PGO trained on seed 999 r8,
1 thread). Digest a977961a58122047 on all ten.

    ms/chunk  release 11.37 10.46  9.95 10.12 10.59   mean 10.50
              pgo      9.57  8.90  9.41  8.97  9.28   mean  9.23   (-12.1%, PGO ahead in 5/5)

So PGO does show a gain on the code that would ship: about 12% of core time per chunk on one
thread. The earlier 8-thread rows (593/685 against 632/658) were separate holds and too noisy to
show it. An 8-thread alternating hold (5 rounds) is queued to decide whether the gain survives
8 threads. If it does, PGO stays as an opt-in release step (about 7 min of build), still not the
headline.

8-thread alternating hold, same commit and setup (`OXIDE_PGO_AB=5 ... 16 8`), digest
a977961a58122047 on all ten:

    cps  release 635.4 626.5 702.5 708.9 738.5   mean 682.4
         pgo     679.6 610.6 778.7 702.6 847.0   mean 723.7   (+6.1%, PGO ahead in 3/5)

**PGO verdict:** a real gain on one thread (-12% core time, 5/5). At 8 threads it is smaller and
inside the noise (+6%, 3/5), so most of the per-core saving does not show up as wall time at full load.
It stays an opt-in script (`bench-pgo-vps.sh`, about 7 min of build) for a release build. It is
not the progress-page headline, and nothing else depends on it. Closed as an experiment.
The `pipeline-merge2` box dir was deleted once these rows were logged.

## Decoration stage profile, 1 thread (auditor's ask)

Commit 0769ca1+timers (`docs/perf/decorate-prof.patch`: per-feature wall time, and region
primitives timed one call in 61. Apply it with `git apply`, do not commit it to a branch).
`oxide-pregen --radius 16 --threads 1 --no-write`, under the shared lock at nice 10, digest
df701ee858e2bdd3. Terrain here is this branch's old code (generate_ms 82126); decoration is
the same code that would ship.

    decorate_ms=24387 on 1 thread: 1225 decorate() calls, 21.5 s CPU in them, 98% inside features
    (19.9 ms per call; about 17.5 ms per finished chunk)

Region primitives (est. CPU, share of decorate CPU):

| call | calls | ns/call | ms | share |
|---|---|---|---|---|
| `block` | 44.6M | 87 | 3889 | 18.1% |
| `set_block` | 10.3M | 338 | 3471 | 16.2% |
| `is` | 37.2M | 84 | 3137 | 14.6% |
| `height` | 21.1M | 114 | 2407 | 11.2% |
| `biomes_present` | 1225 | 268k | 328 | 1.5% |
| `BlockFacts::is` direct | 1.2M | 143 | 165 | 0.8% |

Top features (share of decorate CPU):

| feature | share | the calls it is made of |
|---|---|---|
| sculk_patch_deep_dark | **24.4%** (5239 ms, 505 runs, 10.4 ms each) | 21.8M `block`, 14.3M `is`, 0.5M `set_block` |
| the five stone-type ores (clay, tuff, granite, diorite, andesite) | **28.1%** together | ~1.3M `set_block` + ~1.6M `block` each |
| lush_caves_ceiling_vegetation | 4.7% | 1.6M `is`, 1.5M `block` |
| dark_forest_vegetation | 4.2% | 1.9M `block`, 0.8M direct `BlockFacts::is` |
| ore_gravel | 4.1% | 3.1M `height` |
| lush_caves_vegetation, lush_caves_clay | 3.4%, 3.1% | |
| ore_iron_upper, ore_coal_upper | 2.9%, 1.7% | 8.7M and 4.4M `height` for 12k and 8k writes |

Reading:
1. **Stone ores (28%)** are `set_block`-bound: the interning work (pipeline-2) targets these.
2. **Sculk (24%)** is the largest single feature, and its cost is reads: 43k `block` and 28k `is`
   per run. The next lever is fewer or cheaper reads in sculk's spread and charge logic. A second
   sub-agent (pipeline-3) is starting on it, limited to `decorator/cave/sculk.rs` so it does not
   collide with the interning work in region.rs.
3. Upper ores spend their time in `height` (about 700 calls per write; vanilla's
   discard-on-air-exposure check probably reads heights per block). This is worth a look after sculk.

## Interned block states in DecorationRegion (pipeline-2 sub-agent)

Commits 535525c + 704fea5 (sub-agent worktree; not on the branch yet, see below):
- **Region:** writes are `(BlockPos, u32)` plus a per-region state table, and the fact cache is per distinct
  state. New `set_block_ref(pos, &BlockState)` clones only on a state's first sight in the region,
  and `into_interned()` returns `InternedWrites { writes, states }`. `into_writes` is built on top
  and its output is the same.
- **Call sites:** 69 that passed `x.clone()` now pass `&x`, including both ore paths.

Where `set_block` went (temporary rdtsc counters, r8, `perf` is blocked on the box). The `writes` Vec push of a
fresh owned 88-byte `BlockState` (plus two heap strings) was 70% of `set_block`, and the
ore-target clone at the call site was 14% of region CPU. Overwrites, hashing and written_top
were small.

Alternating hold, r16 x 8, digest df701ee858e2bdd3 on all:

    decorate_ms  before 7252db9: 3497 3510   after 535525c: 3884 2994   (means 3504 / 3439, -2%)

Region-internal cycles dropped about 24%. But the cost moved into `into_writes`, which still
clones one owned state per write for the `Vec<BlockWrite>` contract (24-26% of region cycles).
**Neutral on its own.** The win needs oxide-pregen (this track) to take `into_interned()`
directly. Since `apply_writes` compares by reference, palette order and the digest are unchanged.
That change is on scratch tree /tmp/pipeline-4 (535525c + 704fea5 + the pregen switch). A
3-round alternating hold against this branch's HEAD is running.
Terrain digest after the change: a977961a58122047. Tests: chunkgen 189 and pregen pass.

## Sculk read memo (pipeline-3 sub-agent)

Commit 1a19e6b (sub-agent worktree, only `decorator/cave/sculk.rs`, +325/-66). Each
`place()` memoises what it read per position: sculk/vein, the blockers, water, the 6 vein faces, the
fire / sculk_replaceable / sculk_replaceable_world_gen / inhibitor tags, and facts. The memo is a
reused thread-local box (+-16 x/z, +-32 y, about 1.1 MB per thread) cleared by a generation counter, and
every spreader write invalidates its own entry first. RNG calls, write order and the states written are
unchanged. The digest is df701ee858e2bdd3 on every run.

1-thread timer profile (same patch on both, `--threads 1 --no-write`):

| | before 28286bb | after 1a19e6b |
|---|---|---|
| decorate_ms | 25138 | 24079 |
| sculk cpu_ms (505 runs) | 5173 (23.8%), 10.2 ms/run | 3678 (17.5%), 7.3 ms/run |
| sculk `block` / `is` calls | 21.8M / 14.3M | 3.0M / 52k |
| all `block` / `is` calls | 44.6M / 37.2M | 25.8M / 23.0M |

The 8-thread decorate_ms runs are inconclusive: before 3171 / 5485, after 3732 / 3753. The two
before runs differ by 73% because the box was heavily shared. The read counts are exact. The CPU
saving (-1.5 s, -29% for sculk, about -4% of decoration) comes from one profile each, so a 1-thread
alternating hold is next. Misses still go through `fact_at`, so a region call that returns all fact
bits at once would cut more; that is region.rs work, after interning lands.

## 1-thread alternating hold: interning + pregen switch, sculk memo (auditor's decider)

`scripts/bench-ab-pregen-vps.sh 16 1 3 pipeline pipeline-4 pipeline-3`: one exclusive hold,
r16, 1 thread, `--no-write`, three rounds rotating base (87927ea) / interning + pregen switch /
sculk memo. Digest df701ee858e2bdd3 on all nine. Rows in `bench-pipeline-ab.tsv`.

    decorate_ms  base       20396 22645 22066   mean 21702
                 interning  16951 17010 16106   mean 16689   -23.1%, ahead 3/3
                 sculk memo 24052 21970 22188   mean 22737   +4.8%, behind 2/3

**Kept: interning + pregen switch** (10d7220, 78de067, 639be4e on the branch; `crates/` is identical
to the tested tree, where tests pass: chunkgen 189 and pregen). The first decoration win on
perf/pipeline since the 4aca951 tables: about 5 s of 1-thread decoration CPU at r16.

**Rejected: sculk read memo** (1a19e6b, not landed). The profile showed 29% less sculk CPU, but
the whole stage is slower. The profile's per-call timers do not see the memo's own cost: a 1.1 MB
thread-local box per thread, generation checks, and cache pressure on top of every read. Keep the
idea of fewer reads, but it needs a small per-step neighbourhood rather than a whole-feature box.

**Hold length (auditor):** the 9-run hold above kept the exclusive lock about 15 minutes, and the
noise and datapath builds waited 20 minutes behind it. On this branch's old terrain a 1-thread r16 pregen
run is about 95 s, 70 s of it terrain. From here on, decoration holds run from a scratch tree
merged with origin/perf/datapath (terrain about 10 s per 1-thread run) or at radius 8, and every
exclusive hold stays under about 5 minutes. The HEAD-vs-ore hold queued on the old terrain was
cancelled before it started.

## Ore floor scan skip (in test)

`OreFeature.place` scans the floor height of every column in its box and returns at the first
column whose floor reaches `y_start`. The scan draws no random numbers, and `place_blob` does not
depend on which column hit. Upper ores start far above ground, so nearly every scan reads all
columns and fails: ore_iron_upper made 8.7M `height` calls for 12k writes.

New `DecorationRegion::height_bound(kind)`: the maximum of the nine stored heightmaps (computed
once per kind per region, `max_y` if a map is missing) and the highest write + 1. `height` answers an
unwritten column from the stored map and rescans a written one from at most
`max(stored, written_top + 1)`, so no column of the window exceeds the bound. The ore returns `false`
before the scan when `y_start` is above it. A new unit test checks the bound against every column
before and after writes. Tests: chunkgen 190, pregen pass.

Hold: pipeline-mb (a1d75b1 + origin/perf/datapath 2127bd7) against pipeline-mo (the same + the skip),
r16, 1 thread, 3 rounds.

Ore floor scan skip, holds (`bench-ab-pregen-vps.sh 16 1 3`, merged terrain, about 3.3 min each),
digest df701ee858e2bdd3 on all twelve:

    hold 1 (mb first)  mb 15248 15400 15912   mo 14584 15655 14252
    hold 2 (mo first)  mo 15407 13619 14183   mb 14808 14674 16249
    means over six     mb 15382   mo 14617   -5.0%, mo ahead in 4 of 6 pairs

That is the size the profile predicted (upper iron + coal were 4.6% of decoration CPU). **Kept**:
cherry-picked onto the branch from the tested tree, where chunkgen 193 and pregen tests pass.

## Sculk: read a spread's 3x3x3 neighbourhood once (pipeline-6 sub-agent)

311e70c (only `decorator/cave/sculk.rs`, +245/-71), landed here as the next commit.
- **Main change:** each `spread_all` step touches only the 27 positions around its source. A 27-slot
  stack array is filled on first ask (blocks-spread, vein and its faces, water, fire, the facts
  asked so far) and a slot is cleared when the spread writes its position. The source check is
  hoisted out of the face loop.
- **Smaller changes:** the cursor's facings are a bitmask, not a Vec; the cursor's block is
  borrowed, not cloned; and its fact checks go through the region's fact cache.
- **Unchanged:** RNG order, writes and floats.

Unlike the rejected whole-feature memo, nothing outlives a step and there is no thread-local box.
Hold (`bench-ab-pregen-vps.sh 16 1 3 pipeline-mb pipeline-6`, merged terrain, digest
df701ee858e2bdd3 on all six):

    decorate_ms  mb  17845 16777 19292   mean 17971
                 6   16700 15617 16794   mean 16370   -8.9%, ahead 3/3

**Kept.** Tests passed on the tested tree (chunkgen 192 there, pregen). The branch-head test run
waits until the box is free again: it is reserved for a live server test until about 17:00 CET.

## Plan while the box is reserved (until about 17:00 CET)

Decoration on this branch now has three kept changes on top of 4aca951: interning + pregen switch
(-23%), ore floor-scan skip (-5%) and the sculk neighbourhood (-8.9%). Each was measured on its own
in 1-thread holds. Once the box is free again:

1. Run the tests on the branch head (d6f2442); delete the box dirs pipeline-6 and pipeline-mb.
2. Make a fresh scratch merge of the head with origin/perf/datapath (the hold base) and re-profile there with
   `decorate-prof.patch` rebased onto interning (ready locally: the wrappers now also cover
   `set_block_ref` and the interned output), at r8, 1 thread, shared lock. That run is short.
3. **Stone ores, the per-cell path** (about 28% of decoration before interning). Per cell the path is:
   `ensure_can_write`, `region.block` (about 87 ns), `RuleTest::test`, `set_block_ref`. The design, all
   exact:
   - `TagMatch` does two std-SipHash lookups per cell (`tags.get(tag)`, then the members set by block
     name). Resolve the member set once per `place()`, which drops the first lookup outright. Then cache
     the rule's answer for a state: add `DecorationRegion::block_keyed(pos) -> (StateKey, &BlockState)`
     (StateKey = written id, or chunk/section/palette entry, the same keys the fact caches use) and
     keep a direct-mapped 8-entry (key -> matched) cache per `place()`. Stone ores replace mostly
     stone and deepslate, so almost every cell hits. The random variants still draw exactly when the
     name matches; only the name/tag part is cached.
   - `set_block_ref` hashes the full `BlockState` (name + property map) per write to find its id.
     Intern each target's state once per `place()` (`region.intern(&state) -> u32`) and write with
     `set_block_id(pos, id)`.
   - Expected: most of the ~0.5 us per ore cell that is not the terrain read.
4. Then re-rank from the new profile: lush caves (vegetation, ceiling vegetation, clay: about 11%
   together) and dark_forest_vegetation (direct `BlockFacts::is` calls, 0.8M) are next in line.

Holds stay on the datapath merge, under about 5 minutes each.

## Live-server finding: native-decoration=true, Chunky 115 -> about 10 cps (analysis only)

Reported by the user (laptop session): oxide-bench on perf/datapath bf577ba via CI, Vertex
Engine. With `native-decoration=true`, Chunky dropped from 115 to about 10 chunks/s. jcmd shows worker
time almost entirely in `OxideNative.generateChunk` under the generateBiomes stage, and Java
`Feature.place` / `OreFeature` still run afterwards. Read from the code at bf577ba; no server or
box action taken.

**1. Features run twice: the Vertex path never turns vanilla decoration off.**
- **Bukkit path:** `native-decoration` is honoured only by the Bukkit generator,
  `OxideChunkGenerator.shouldGenerateDecorations()` (returns `!service.nativeDecoration()`).
- **Vertex path:** `OxideVertexModule` registers `OxideChunkHook` for
  `EnumSet.of(ChunkStage.BIOMES, ChunkStage.SURFACE, ChunkStage.CARVERS)` and nothing reads the flag
  there. `GeneratorService.openHandle` does call `handle.setDecoration(true)`, so the native call
  returns decorated chunks, but the engine still owns the FEATURES stage and runs vanilla's features on
  top. The result is double ores and trees, a wrong world, and all of vanilla's feature cost on top of
  Oxide's.
- **Why it shows under generateBiomes:** the hook is registered at BIOMES (its lowest claimed
  stage), so the whole native call, decoration included, lands there. That part is expected.
- **Fix (plugin owner):** when native decoration is on, the module must also take the FEATURES stage
  (or whatever the Vertex API offers to skip vanilla features). If the API cannot do that, refuse
  `native-decoration` under Vertex at startup instead of running both.

**2. Neighbour terrain is regenerated per chunk: the decoration cache is far smaller than Chunky's
working set, and FIFO rather than LRU.**
- **What one finished chunk costs:** `Handle::generate_chunk` with decoration on calls
  `decorated_chunk`. One finished chunk needs the 9 write lists of its 3x3 neighbours, and each write
  list needs the 3x3 undecorated terrain around its source: 5x5 = 25 terrain chunks and 9 feature runs
  per finished chunk, unless cached.
- **The cache:** `DecorationCache` holds 256 terrain chunks and 1024 write lists. `Bounded::get` does not
  refresh an entry's age, so it is FIFO: a chunk every neighbour keeps asking for is still evicted
  first.
- **Chunky:** it walks a spiral. At ring r the 5x5 needs rings r-2..r+2, and the inner rings were last
  touched a whole ring earlier, about 8r chunks ago. Past r of about 8-10 that exceeds 256, so those are gone.
- **Estimated cost:** in steady state each finished chunk regenerates about 5 terrain chunks (a new
  5-wide column) and runs about 3 sources' features (a new 3-wide column), against 1 terrain and no
  native features with decoration off. That is in line with the 11x drop. The single global `Mutex` per
  cache adds contention across the 8 workers, but it is secondary.
- **bf577ba also predates this branch's decoration work:** writes are `Vec<BlockWrite>` with a
  `BlockState` clone per write, and applying them clones again. Interning (-23%), the ore scan skip (-5%)
  and the sculk neighbourhood (-8.9%) are not in it.

**What would fix the cost.** (2b) and (2c) touch decorate.rs, which this track owns. (2a) and the
handle constants are oxide-ffi.
- **(2a) Cheapest:** make `Bounded` LRU (refresh on get) and size the caches from the generation order
  instead of a constant. A spiral needs about 3 rings of terrain and 3 rings of write lists live:
  `terrain >= ~24 * r_max`, `writes >= ~24 * r_max`. Memory is the limit: an undecorated `ChunkData` is a
  few hundred KB.
- **(2b) Shrink what is cached:**
  - Keep write lists as `InternedWrites` (this branch). With those cached, terrain is needed only to
    make a new write list, plus the finished chunk's own.
  - Keep cached terrain as palette indices rather than `ChunkData` with owned states, so a few
    thousand chunks fit.
- **(2c) Structural:** decorate in batches the way `oxide-pregen` does. It generates each terrain chunk
  and each write list exactly once over a square (terrain at r+2, writes at r+1). For a live
  pregeneration the fastest route is to run `oxide-pregen` offline and let Chunky load, not generate.

**Next, when the box is free:** a counting test, with no server. Drive `decorated_chunk` in Chunky's
spiral order over r=32 with the bf577ba cache sizes and count terrain generations and feature runs per
finished chunk. Then repeat with LRU and with larger caches. That turns the estimate above into a
number before anyone changes the live config.

## Merged-tree provenance (auditor: the pregen digest is the only check of palette order, biomes, heightmaps)

The terrain digest a977961a58122047 hashes block states only. The pregen digest df701ee858e2bdd3
hashes every chunk's whole NBT, so on merged trees it is the only check of palette order, biomes and
heightmaps. Every merged-tree row names its merge:

| scratch merge | this branch | merged with | pregen digest |
|---|---|---|---|
| bf28e89 (`bench-pipeline-merge.tsv`) | 4aca951 | origin/perf/datapath 0e3cf61 | df701ee858e2bdd3 |
| a1f3a32 (`bench-pipeline-merge2.tsv`) | 27e2f9b | origin/perf/noise e6ecc65 (holds datapath's kept work) | df701ee858e2bdd3 |
| e327327 = `pipeline-mb` in `bench-pipeline-ab.tsv`, plus the -mo and -6 rows | a1d75b1 | origin/perf/datapath 2127bd7 | df701ee858e2bdd3 |

df701ee858e2bdd3 has held on every pregen row so far, merged or not. Any change is reported to the
auditor at once.

## Stone-ore per-state match cache kept, -14.9% decorate (merged with datapath d137014)

**Change** (9480668, cherry-picked from the tested scratch commit):
- region.rs: `StateKey(u64)` with `slot()`, plus `block_keyed`, `intern` and `set_block_id`, so a
  feature can read a cell's interned state key and write a state it interned once. New region test.
- ore.rs: `RuleTest` split into `matches` (pure in the state) and `roll` (random draw), a direct-mapped
  `MatchCache` of 8 slots keyed by `StateKey`, and `place_at_cached`. The random draws happen in the
  same order and count as before; only the repeated state comparison is cached.

**Hold** (`bench-ab-pregen-vps.sh 16 1 3 pipeline-mb pipeline-mo`, rows 1794762319 in
`bench-pipeline-ab.tsv`). Both trees are this branch at 36688fe merged with origin/perf/datapath
**d137014** (scratch merge 30fddc1); pipeline-mo adds the change (abfd462).

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 15583 | 15899 | 16340 | 15941 |
| stone-ore decorate_ms | 13010 | 13939 | 13739 | 13563 |

-14.9%, ahead in 3/3 pairs. Every row kept df701ee858e2bdd3. Tests on the mo tree: chunkgen 194 pass,
pregen pass.

## Spiral counting test: neighbour regeneration is smaller than estimated

`spiral_cost` (81e6458, ignored test in decorate.rs) drives `decorated_chunk` in Chunky's square spiral
out to r=32 (4225 chunks), one request at a time, with this branch's `DecorationCache` (FIFO, as at
bf577ba). It counts terrain generations and feature runs per finished chunk.

| terrain cache | write cache | terrain per chunk | feature runs per chunk |
|---|---|---|---|
| 256 (live server) | 1024 (live server) | 2.77 | 1.06 |
| 1024 | 1024 | 1.13 | 1.06 |
| 2048 | 2048 | 1.13 | 1.06 |
| 4096 | 4096 | 1.13 | 1.06 |

- 1.13 and 1.06 are the floors for a square: terrain is needed out to r+2 and writes to r+1, and
  (69/65)^2 = 1.127, (67/65)^2 = 1.062. A terrain cache of 1024 already reaches the floor; LRU would
  not lower it further for a pure spiral.
- **Correction to the live-server entry above:** it estimated about 5 terrain generations and 3 feature
  runs per chunk. The count is 2.77 and 1.06. Features are not rerun; terrain is generated about 2.5x
  more than needed at the server's 256.
- So regeneration explains at most a ~2-3x slowdown, not the 11x drop. The other named cause, vanilla
  FEATURES running on top of native decoration (item 1), and contention on the per-cache `Mutex` across
  8 workers, carry the rest. Not measured here: the test is single-threaded in exact spiral order with
  flat terrain; Chunky's 8 workers interleave neighbouring requests, which may cost more.
- (2a) reduces to raising `TERRAIN_CACHE_CHUNKS` from 256 to about 1024 (oxide-ffi, not this track).
  (2b) matters only for memory at that size.

## Decoration profile on the branch head, 1 thread (merged with datapath 00b456d)

Tree: this branch at 92d353d merged with origin/perf/datapath **00b456d** (scratch merge de747f2,
bench-vps.sh conflict resolved with ours), plus the timer patch v3 (not committed).
`oxide-pregen --radius 16 --threads 1 --no-write`, shared lock at nice 10, digest **df701ee858e2bdd3**.

    BENCH secs=31.620 generate_ms=16788 decorate_ms=13439
    decorate() calls=1225 cpu_ms=11378 (in features 11143); first profile (28286bb) was 21.5 s CPU

Primitives now: `block` 19.9M calls (15.0%), `set_block` 2.27M at 449 ns (9.0%), `is` 9.7M (6.7%),
`height` 1.8M (2.2%). Ore writes and reads now go through `set_block_id`/`block_keyed`, which the
timers do not wrap, so ore rows show set_block=0 and their cost is in the feature total only.

| feature group | cpu share |
|---|---|
| sculk_patch_deep_dark (505 runs, 7.19M `block`, 1.15M `is`, 0.52M `set_block`) | 28.8% |
| lush caves: ceiling_vegetation 6.1, vegetation 4.5, clay 4.2, cave_vines 3.7 | 18.5% |
| stone ores: clay 4.2, granite 3.8, diorite 3.6, tuff 3.5, andesite 3.4 (was 28.1%) | 18.5% |
| dark_forest_vegetation | 5.9% |
| glow_lichen | 2.7% |
| amethyst_geode | 2.4% |

Next targets in order: sculk's 14k `block` reads per run, then the four lush-cave features (they
read about 1.3-1.6M cells each, mostly `is` + `block` on the same cave ceilings and floors).

## Sculk: each state described once per run, veins written by id; kept, -7.8% decorate

**Change** (cherry-picked from sub-agent commit 74178ec; mainly `decorator/cave/sculk.rs`):
- Every read goes through `block_keyed` into a 256-slot per-`place()` cache keyed by `StateKey`. A miss
  records everything sculk asks of the state at once: sculk/vein, blocker, water, fire, both
  replaceable tags, inhibitor, waterlogged, six faces, all fact bits.
- Face checks, the 243-cell inhibitor scan in `canPlaceGrowth`, movement and substrate checks,
  `onDischarged` and `attemptPlaceSculk` read each cell once.
- Veins are named by face mask + waterlogged and interned on first use; sculk, water, air,
  sensor/shrieker likewise. All writes go through `set_block_id`, so no clone or hash per step.
- Exactness guards: `compile` refuses a default vein that is not the 7 properties all `false`; a vein
  read from the world with any other properties takes the old state path. Draws, write order and
  written states are unchanged, including writes that leave a block as it was.
- **Outside this track's files:** 8 lines in `crates/oxide-datapack/src/block_facts.rs` add
  `BlockFacts::bits(state)` (every fact with one hash lookup). Three sculk-only helpers in
  `decorator/cave/support.rs` are removed; `VOID_AIR` in `context.rs` is `pub(crate)`.

**Hold** (`bench-ab-pregen-vps.sh 16 1 3 pipeline-mb2 pipeline-sk2`, rows 1794766877). Both trees are
this branch at 4e1b187 merged with origin/perf/datapath **c38344f** (scratch merge b36dcf1).

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 13519 | 13447 | 13603 | 13523 |
| sculk decorate_ms | 12330 | 13049 | 12014 | 12464 |
| java_cpu_pct (1 java process, 1 s sample at round start) | 110 | 132 | 206 | |

-7.8%, ahead 3/3, df701ee858e2bdd3 on every row. The Java server was using 1.1-2.1 cores during the
hold; with 1-thread alternating rounds both trees saw the same load. Tests on the merged tree:
chunkgen 194 pass (1 ignored), pregen pass.

## Lush caves: biome gate before the scan, one read per scan step, cached patch tests; rejected (-0.7%)

**Change** (sub-agent commit b7d68dd; patch at `docs/perf/rejected-lush-caves-scan.patch`):
- a placed chain whose `biome` filter follows only draw-free, column-keeping, non-tallying modifiers
  (`environment_scan`, y-only constant `random_offset`, ...) drops a center-chunk position early
  when no quart of its 4x4 column has a biome listing the feature;
- `environment_scan` with local predicates reads each cell once (`DecorationRegion::probe`) and
  caches (allowed, target) per `StateKey`;
- `face_full` answers from one probe; `vegetation_patch` reuses air-walk answers, writes a constant
  ground state by id, caches name/replaceable tests per key.

**Hold** (rows 1794767525; same trees as the sculk hold, merged with datapath **c38344f**):

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 13293 | 13209 | 12945 | 13149 |
| lush decorate_ms | 13268 | 13134 | 12760 | 13054 |
| java_cpu_pct | 331 | 62 | 54 | |

-0.7%: ahead 3/3 but by 25-185 ms, inside run-to-run spread. The profile gave the four lush features
18.5% of decorate CPU, so the change did not reach where that time goes. **Rejected** (454 lines for a
gain inside noise). Digest df701ee858e2bdd3 held on every row. Next: a timed profile of this tree to
see whether the lush time is before the gate (count/height_range draws) or in the features
themselves on the lush columns.

## Hold script: CPU seconds per run and java CPU per run (auditor, while the Java server is on s5)

`bench-ab-pregen-vps.sh` (0ad50d7) wraps each run in `/usr/bin/time -f '%U %S'` and reads the java
processes' utime+stime from `/proc/<pid>/stat` before and after each run (read only). Each row gains
`cpu_user_s= cpu_sys_s= java_n= java_cpu_s= java_cpu_s_per_10s=`. Rounds above about 2 java CPU-s per
10 s are dropped from decisions. A radius-2 smoke run just now read java at 8.6 CPU-s per 10 s, so the
server is busy; holds go on, and the rows say which rounds count.

## Feature-time-only profile (timer bias check), 1 thread, branch with sculk, datapath c38344f

The full timer patch counts every region call, which inflates read-heavy features: the lush tree's
timed profile showed its four features 0.72 s cheaper, the hold only 0.1 s. A sampling profiler is not
available (`perf_event_paranoid=4`, no sudo). So this profile keeps only the per-feature wall time
(`prof::start` returns `None`, no call counting). Tree: b36dcf1 + sculk 5dcb571 + lite timers,
shared lock, digest df701ee858e2bdd3, decorate CPU 9328 ms.

| feature group | ms | share |
|---|---|---|
| stone ores: clay 538, granite 478, tuff 456, diorite 443, andesite 438 | 2353 | 25.2% |
| lush: ceiling_vegetation 667, vegetation 492, clay 448, cave_vines 385 | 1992 | 21.4% |
| sculk_patch_deep_dark (was 28.8% under full timers, before the sculk change) | 1060 | 11.4% |
| dark_forest_vegetation | 658 | 7.1% |
| glow_lichen 326, ore_gravel 287, amethyst_geode 259, ore_dirt 210 | 1082 | 11.6% |

Lush features and ore_clay make almost no block reads per run yet cost 0.3-0.56 ms per run, so the
time is not in reads. Next profile splits each feature into placement-chain time and feature-body time.

## Split profile: placement chain vs feature body (same tree, lite timers + outermost-place timer)

Feature body = time inside the outermost `place_configured_feature` of each chain; chain = the rest
(modifiers: count, in_square, height_range, environment_scan, biome...). Decorate CPU 8761 ms,
digest df701ee858e2bdd3.

| feature | total ms | body ms | chain ms |
|---|---|---|---|
| sculk_patch_deep_dark | 1001 | 961 | 40 |
| lush_caves_ceiling_vegetation | 705 | 444 | 261 |
| dark_forest_vegetation | 618 | 610 | 8 |
| ore_clay | 472 | 449 | 23 |
| lush_caves_vegetation | 468 | 230 | 238 |
| ore_granite_lower | 428 | 425 | 3 |
| lush_caves_clay | 415 | 287 | 128 |
| ore_andesite/diorite/tuff | ~400 each | ~395 each | ~5 |
| cave_vines | 381 | 16 | **365** |
| classic_vines_cave_feature | 115 | 30 | 85 |

- The lush chains are `count, in_square, height_range, environment_scan, random_offset(y), biome`:
  ~1.1 s of chain time across five features, almost all in the scans (cave_vines draws 188
  positions per chunk, each scanning up to 12 cells).
- The rejected change's column gate barely fired: lush_caves sits somewhere under nearly every
  column near spawn (1188 of 1225 decorate calls run the lush features), so "no lush quart in the
  whole column" is rare. Next: gate on only the quarts the scan can reach (y + span of the quiet run).
- Stone ores are all body: the remaining cost is the ore shape loop itself.

## Lush caves again: biome gate on the quarts the scan can reach; kept, -10.1% decorate

The rejected lush change (b7d68dd) plus one refinement (d2b4b14): the gate now bounds the y the
biome filter will see. Each quiet modifier before `biome` reports a y-span (environment_scan up:
(0, max_steps), down: (-max_steps, 0); y-only constant random_offset: (dy, dy); quiet predicate:
(0, 0); heightmap-like: unbounded). A center-chunk position entering the run is dropped only when no
quart in `[y+lo, y+hi]` of its 4x4 column (clamped into the world, as `DecorationRegion::biome`
clamps) has a biome listing the feature. `DecorationRegion::center_biome_levels` builds the
per-quart-level column bits once per placed-feature run. The skipped positions draw nothing, write
nothing and tally nothing, so it is exact; the digest confirms it.

**Hold** (rows from this run in `bench-pipeline-ab.tsv`). Both trees: b36dcf1 (this branch merged
with origin/perf/datapath **c38344f**) + sculk 5dcb571; pipeline-lg adds 7ac51b8 + d2b4b14.

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 10122 | 10659 | 10992 | 10591 |
| lush + gate decorate_ms | 9769 | 9583 | 9208 | 9520 |
| base cpu_user_s | 27.10 | 27.68 | 28.04 | 27.61 |
| lush + gate cpu_user_s | 27.54 | 25.96 | 26.24 | 26.58 |
| java_cpu_s_per_10s (base / lush) | 0.19 / 0.25 | 0.26 / 0.25 | 0.23 / 0.19 | |

-10.1% decorate_ms, ahead 3/3; -3.7% user CPU per run (decoration is about a third of it). The Java
server was quiet (all rounds far under 2 CPU-s per 10 s), so every round counts. df701ee858e2bdd3 on
every row. Tests on the tree: chunkgen 194 pass (1 ignored), pregen pass. Landed on the branch as
the two cherry-picked commits; the column-only version alone was -0.7% (entry above), so the gain is
the y-window.

## Ore z-run + region written-cell bitmap: rejected (+2.9% decorate)

**Change** (sub-agent commit b6141b6; patch at `docs/perf/rejected-ore-zrun-bitmap.patch`):
- ore.rs: along a sphere row the cell test holds on one contiguous z run (every rounded step is
  monotone), so the run's ends are found with a few exact evaluations instead of one division per
  cell; `dx*dx + dy*dy` and the build-height test hoisted per row; tested bitset sized up front.
- region.rs: a per-section written-cell bitmap so reads of unwritten cells skip the overlay hash.
  This reroutes four existing reads (`block`, `block_keyed`, `fact_at`, `probe`) and adds a statement
  to `write_id`, in this track's own file.

**Hold** (pipeline-lg = b36dcf1 [datapath **c38344f**] + sculk + lush gate; pipeline-or2 adds the change):

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 9531 | 9482 | 10628 | 9880 |
| ore decorate_ms | 10524 | 9640 | 10339 | 10168 |
| base / ore cpu_user_s | 26.07 / 27.12 | 26.01 / 26.79 | 26.64 / 28.08 | 26.24 / 27.33 |
| java_cpu_s_per_10s | 0.22 / 0.25 | 0.23 / 0.21 | 0.24 / 0.27 | |

+2.9% decorate, +4.2% user CPU, behind 2/3 (user CPU behind 3/3). Java quiet, all rounds count.
Digest df701ee858e2bdd3 on every row; tests 195 pass. **Rejected.** The ore.rs part alone is being
held next to separate the two halves.

### Ore z-run alone (no region bitmap): rejected (-0.7%, inside noise)

pipeline-or3 = pipeline-lg + only the ore.rs half of b6141b6 (d072107).

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 9059 | 8702 | 9690 | 9150 |
| z-run decorate_ms | 9287 | 8689 | 9280 | 9085 |
| base / z-run cpu_user_s | 25.86 / 25.82 | 25.05 / 25.39 | 28.20 / 26.43 | 26.37 / 25.88 |
| java_cpu_s_per_10s | 0.22 / 0.24 | 0.30 / 0.20 | 0.23 / 0.24 | |

-0.7%, behind in 1 of 3; inside run-to-run spread. Digest held, tests 195 pass. **Rejected.** So the
sphere test is not where the stone-ore body time goes, and the bitmap half cost about +3.5%. The ore
body needs its own split (sphere setup vs per-cell test vs cell read/match vs write) before another try.

## Dense per-section write overlay in place of the HashMap: -9.1% decorate on base 1, second base pending

**Change** (sub-agent commit a035159; only `region.rs`, this track's file):
- `overlay` is a `Vec<Option<Box<[u32; 8192]>>>`, one slot per (chunk in the 3x3 window, section),
  allocated once per region; a section's block is allocated and zeroed on its first write. The first
  4096 entries hold written state id + 1 (0 = unwritten); the second 4096 the cell's index into
  `writes`, read only on a rewrite, which updates `writes[original].1` in place (order unchanged).
- `locate(pos)` does the window and height test once and returns chunk, section and cell;
  `block`, `block_keyed`, `fact_at`, `probe` and all three `set_block*` use it. No hashing on reads or
  writes; `contains` and the `entry` probe are gone from the write path.
- Counts (r16, temporary print): 7535 writes into 45.4 sections per region on average, at most
  14061 writes into 86 sections. Memory: 32 KB per written section, ~1.45 MB zeroed per region.

**Hold, base 1** (rows 1794779368). pipeline-lg = b36dcf1 (branch merged with origin/perf/datapath
**c38344f**) + sculk + lush gate; pipeline-ov2 adds the change (2c6b5f1).

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 10255 | 9467 | 9723 | 9815 |
| overlay decorate_ms | 9396 | 8197 | 9181 | 8925 |
| base / overlay cpu_user_s | 26.20 / 25.63 | 25.81 / 25.12 | 25.47 / 26.26 | 25.83 / 25.67 |
| java_cpu_s_per_10s | 0.21 / 0.22 | 0.28 / 0.20 | 0.20 / 0.33 | |

-9.1% decorate, ahead 3/3; user CPU -0.6% (behind in round 3). Java quiet, all rounds count.
df701ee858e2bdd3 on every row; tests chunkgen 194 pass (1 ignored), pregen pass. Under 10%, so a
second hold on a fresh base (branch head merged with the current origin/perf/datapath) decides it.
(The hold finished 21:47 UTC; this entry is late because I missed its completion.)

### Dense write overlay, base 2: -8.4%; kept

pipeline-b2 = this branch at b84621e merged with origin/perf/datapath **26f512f** (scratch merge
3709e62); pipeline-ov3 adds the change (7c7ca06).

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 10693 | 9554 | 9053 | 9767 |
| overlay decorate_ms | 8642 | 10012 | 8197 | 8950 |
| base / overlay cpu_user_s | 27.83 / 25.75 | 26.59 / 27.49 | 25.57 / 24.02 | 26.66 / 25.75 |
| java_cpu_s_per_10s | 0.32 / 0.24 | 0.24 / 0.34 | 0.21 / 0.20 | |

-8.4% decorate and -3.4% user CPU, ahead 2/3 on both. With base 1 (-9.1%, 3/3) it is ahead in 5 of
6 rounds on two bases. Java quiet, all rounds count; df701ee858e2bdd3 on every row; tests chunkgen 194
pass. **Kept**, cherry-picked onto the branch.

## Trees: per-state info cache and interned writes (dark_forest_vegetation target); base 1 -12.3%

**Change** (sub-agent commit 4d69493; `decorator/tree.rs`, `decorator/tree/trunk.rs`):
- a 64-slot per-tree cache keyed by `StateKey` holding what tree code asks of a state (air; leaves,
  logs, replaceable_by_trees, prevents_nearby_leaf_decay tags; vine; persistent; parsed distance);
  outside-window positions read as void_air with their own entry. `is_free`, the leaf-distance pass
  and the other read checks use it; `has_vine` no longer builds a `ResourceLocation` per call.
- `Run::fact` goes through the region's fact cache (waterlogged, roots, leaf litter solidity).
- fixed trunk/foliage states are interned on their first accepted write and written by id after;
  leaf-distance rewrites cached by (state, distance) and written by id. Out-of-window positions keep
  the old path. Draws, visiting order (Java-ordered position sets), write order and states unchanged.
- Shared tree code, so every tree feature benefits, not only dark forest.

**Hold, base 1** (pipeline-b2 = branch b84621e + datapath **26f512f**; pipeline-df2 adds 33c801c):

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 10583 | 9594 | 10241 | 10139 |
| trees decorate_ms | 8168 | 9246 | 9269 | 8894 |
| base / trees cpu_user_s | 27.28 / 24.74 | 26.35 / 26.06 | 26.67 / 26.71 | 26.77 / 25.84 |
| java_cpu_s_per_10s | 0.28 / 0.20 | 0.25 / 0.23 | 0.28 / 0.27 | |

-12.3% decorate ahead 3/3; -3.5% user CPU (round 3 even). Java quiet. df701ee858e2bdd3 on every row;
tests chunkgen 194 pass. Larger than dark forest's ~7% share; confirming on a second base (branch
head with the dense overlay, merged with datapath) before landing.

## Glow lichen search stop + geode interned layers; base 1 +6.8% (losing)

**Change** (sub-agent commit 963b4f9; `decorator/vegetation/multiface.rs`, `decorator/cave/geode.rs`):
- glow lichen: vanilla's search re-tests the same neighbour `search_range` (20) times per direction;
  when `place_growth` finds no supporting face it has only read (no write, draw or tally), so the
  remaining passes are identical and the loop stops; a refusal keeps looping. One `probe` per cell
  test; `can_be_placed_on` cached per `StateKey` (8 slots); fixed direction arrays.
- geode: fixed layer providers and crack air interned once per run and written by id;
  `cannot_replace` answered from one keyed read cached per `StateKey`. Noise and float order untouched.

**Hold, base 1** (pipeline-b2 = branch b84621e + datapath **26f512f**; pipeline-gl2 adds 05548b7):

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 9073 | 9102 | 9111 | 9095 |
| lichen/geode decorate_ms | 9983 | 9089 | 10054 | 9709 |
| base / change cpu_user_s | 26.03 / 26.56 | 25.77 / 26.74 | 25.88 / 26.21 | 25.89 / 26.50 |
| java_cpu_s_per_10s | 0.35 / 0.24 | 0.22 / 0.27 | 0.21 / 0.30 | |

+6.8% decorate (behind 2/3, one even), +2.4% user CPU (behind 3/3). Java quiet; digest held; tests 194
pass. A second hold on base 2 (branch head with the overlay + datapath 80add17) is queued; unless it
reverses clearly, this is rejected.

### Trees, base 2: -3.8%; kept

pipeline-b3 = this branch at 69984c6 (with the dense overlay) merged with origin/perf/datapath
**80add17** (scratch merge 106d867); pipeline-df3 adds the tree change.

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 10196 | 8500 | 9114 | 9270 |
| trees decorate_ms | 9358 | 8399 | 9004 | 8920 |
| base / trees cpu_user_s | 27.68 / 26.28 | 25.14 / 25.04 | 25.80 / 25.77 | 26.21 / 25.70 |
| java_cpu_s_per_10s | 0.38 / 0.29 | 0.22 / 0.24 | 0.24 / 0.26 | |

-3.8% decorate, -1.9% user CPU, ahead 3/3 on both. With base 1 (-12.3%) it is ahead 6/6 on two
bases; the gain is smaller on top of the overlay, which already made the tree code's reads cheap.
Java quiet; df701ee858e2bdd3 on every row; tests 194 pass. **Kept**, cherry-picked onto the branch.

### Glow lichen + geode, base 2: +1.2%; rejected

Same base (pipeline-b3, datapath 80add17); pipeline-gl3 adds the change.

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 10122 | 9064 | 9213 | 9466 |
| change decorate_ms | 10248 | 8758 | 9746 | 9584 |
| base / change cpu_user_s | 28.09 / 29.20 | 26.44 / 26.83 | 26.47 / 27.40 | 27.00 / 27.81 |
| java_cpu_s_per_10s | 0.42 / 0.30 | 0.29 / 0.26 | 0.26 / 0.27 | |

+1.2% decorate, +3.0% user CPU, behind 3/3 on CPU. Losing on both bases (+6.8% on base 1).
**Rejected**; patch at `docs/perf/rejected-lichen-geode.patch`. Digest held throughout. A likely
reason: the per-call caches and extra branches cost more than the repeated reads they save now that
reads are cheap; glow lichen's early stop alone may still be worth a separate small try.

## Lush cave patch bodies: fewer reads, no state copies; base 1 -8.4%

**Change** (sub-agent commit eb548dc; under `decorator/`):
- `vegetation/patch.rs`: each column cell read once (the step back after an air walk reuses the
  cell already seen; the block under the ground reuses the last solid read for the sturdy-face check
  and the first ground step); a ground step that finds the ground block already there stops at once
  instead of asking the same question `depth` times with nothing changed in between.
- `vegetation/geometry.rs`: `PosSet` stops hashing (patch positions come from distinct columns).
- `state_provider.rs`: `state_ref` (same draws, returns the configured state by reference);
  `randomized_int_state_provider` over a fixed/weighted source precomputes every rolled state and its
  table membership at compile time.
- `feature.rs`, `ground/block_column.rs`: simple_block and block_column write by reference; block
  column's allowed-placement test answered once per `StateKey` per call when it is local.
- `survival.rs`: survival-rule tag sets are `AHashSet` (lookup only, never iterated).
- Draws, write order, written states and state-table order unchanged; `region.rs` untouched.

**Hold, base 1** (pipeline-b3 = branch 69984c6 + datapath **80add17**; pipeline-lb3 adds 5c61f04):

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 10569 | 10232 | 10305 | 10369 |
| lush decorate_ms | 9423 | 9689 | 9387 | 9500 |
| base / lush cpu_user_s | 28.04 / 26.54 | 27.43 / 26.75 | 27.52 / 26.35 | 27.66 / 26.55 |
| java_cpu_s_per_10s | 0.39 / 0.45 | 0.35 / 0.29 | 0.30 / 0.29 | |

-8.4% decorate, -4.0% user CPU, ahead 3/3 on both. Java quiet; df701ee858e2bdd3 on every row; tests
194 pass. Second base (branch head with trees, merged with current datapath) next.

### Lush patch bodies, base 2: +1.4% (inconclusive with base 1)

pipeline-b4 = this branch at b793819 (overlay + trees) merged with origin/perf/datapath **f52ccea**
(scratch merge c8eb303); pipeline-lb4 adds eb548dc.

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 9152 | 8303 | 8255 | 8570 |
| lush decorate_ms | 8401 | 9301 | 8370 | 8691 |
| base / lush cpu_user_s | 25.31 / 24.98 | 25.27 / 25.67 | 24.74 / 25.19 | 25.11 / 25.28 |
| java_cpu_s_per_10s | 0.25 / 0.24 | 0.26 / 0.41 | 0.29 / 0.27 | |

+1.4% decorate, +0.7% user CPU, behind 2/3; base 1 was -8.4% ahead 3/3. The bases disagree, so a
repeat hold on base 2 decides. Digest held; tests 194 pass.

## Profile of the branch head (overlay + trees; datapath f52ccea), split timers + ore cell counts

Lite split profile (feature wall time; no call counting), 1 thread r16, digest df701ee858e2bdd3:
decorate CPU **7344 ms** (was 9328 at the first lite profile). Top: sculk 906 (12.3%), ore_clay 530,
ore_tuff 380, ore_diorite_lower 373, dark_forest_vegetation 372, ore_granite_lower 370,
ore_andesite_lower 349, glow_lichen 347, lush ceiling 346, ore_gravel 260, geode 239. The ores
together are about 2.6 s, roughly 35%, all in the feature body.

Ore cell counters (temporary, in `place_blob`), whole run:

    blobs=130258 box_cells=66365335 in_sphere=36832030 fresh=10144191 written=7988750

- Ores write 8.0M cells = ~6500 per region, most of the ~7500 decoration writes per region.
- The box loop visits 66M cells (one f64 division each); 36.8M fall inside a sphere; only 10.1M are
  new to the bitset. The z-run change (the ore.rs half of b6141b6) was held before the dense overlay,
  when hashing hid the loop cost; it is held again on the current base.

### Lush patch bodies, base 2 repeat (order reversed): -2.5%, 1/3; rejected

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| lush decorate_ms (run first) | 7971 | 8087 | 7840 | 7966 |
| base decorate_ms | 8908 | 7786 | 7806 | 8167 |
| lush / base cpu_user_s | 22.33 / 23.72 | 23.03 / 22.44 | 23.34 / 23.01 | 22.90 / 23.06 |
| java_cpu_s_per_10s | 0.21 / 0.22 | 0.22 / 0.20 | 0.21 / 0.25 | |

Ahead only in round 1. Over both base-2 holds (6 rounds) the change is -0.5% decorate (8368 vs 8329
mean), i.e. nothing, against -8.4% on base 1 (before the tree change landed). The tree change already
took the cheap state-reference and fact-cache savings the shared helpers offered. **Rejected**; patch
at `docs/perf/rejected-lush-patch-bodies.patch`. Digest held on every row.

### Ore z-run retried on the dense-overlay base: -2.1%, 2/3; rejected again

pipeline-zr4 = pipeline-b4 (branch b793819 + datapath **f52ccea**) + the ore.rs half of b6141b6.
Tests 195 pass. (The first attempt to start this hold failed: ssh to the box was reset for a few
minutes around 02:20 UTC; it accepted connections again at 02:25.)

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 9029 | 8283 | 7954 | 8422 |
| z-run decorate_ms | 8059 | 8225 | 8463 | 8249 |
| base / z-run cpu_user_s | 24.81 / 23.45 | 23.21 / 23.97 | 23.86 / 23.56 | 23.96 / 23.66 |
| java_cpu_s_per_10s | 0.26 / 0.24 | 0.21 / 0.24 | 0.29 / 0.23 | |

-2.1% decorate and -1.3% user CPU, ahead 2/3 on each: inside run-to-run spread, as on the first try
(-0.7%). **Rejected.** The ore cost is in the 10M fresh cells (read + match + write), not the box
arithmetic. Digest held.

## Ore: one lookup per fresh cell, per-row height test, pooled overlay sections; base 1 -9.1%

**Change** (sub-agent commit 1e10bc6; `decorator/ore.rs`, `region.rs`):
- `DecorationRegion::cell(pos)` resolves a cell once; `block_keyed_at(cell)` and
  `set_block_id_at(cell, id)` use it, so each fresh ore cell does one window/height test and one index
  computation. The ore path drops `ensure_can_write` (`cell` refuses the same positions before any
  draw, so the answer is the same `false`). `block_keyed` goes through the same code.
- `outside_build_height(y)` once per y row (it only ever skipped cells: no draw, no `tested` bit).
- overlay: a cell's state id and its `writes` index sit side by side (one cache line per write); a
  finished region zeroes only the cells it wrote and returns its sections to a per-thread pool (up to
  256), instead of each region zeroing 32 KB per written section. New test
  `a_later_region_sees_none_of_an_earlier_ones_writes` (reuse after `into_interned` and after drop).
- Draws, visit order, `writes` order, written states and interning order unchanged.

**Hold, base 1** (pipeline-b4 = branch b793819 + datapath **f52ccea**; pipeline-rc4 adds abfc677):

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 9032 | 8289 | 9190 | 8837 |
| ore cell decorate_ms | 8603 | 7784 | 7713 | 8033 |
| base / change cpu_user_s | 25.15 / 24.36 | 24.41 / 23.38 | 24.82 / 23.26 | 24.79 / 23.67 |
| java_cpu_s_per_10s | 0.23 / 0.28 | 0.25 / 0.22 | 0.26 / 0.22 | |

-9.1% decorate, -4.5% user CPU, ahead 3/3 on both. Java quiet; df701ee858e2bdd3 on every row; tests
195 pass (the chained test step printed nothing, so they were re-run separately and passed). Under
10%, second base next.

### Ore one-lookup cells, base 2: -8.3%; kept

pipeline-b5 = this branch at 80f76d3 merged with origin/perf/datapath **667bdbb** (scratch merge
1a87a00); pipeline-rc5 adds the change. Tests 195 pass.

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 8989 | 8903 | 9024 | 8972 |
| ore cell decorate_ms | 8018 | 8914 | 7738 | 8223 |
| base / change cpu_user_s | 24.72 / 22.99 | 25.55 / 25.12 | 24.92 / 23.90 | 25.06 / 24.00 |
| java_cpu_s_per_10s | 0.24 / 0.29 | 0.26 / 0.26 | 0.25 / 0.24 | |

-8.3% decorate, -4.2% user CPU, ahead 3/3 on both; with base 1 (-9.1%) ahead 6/6 on two bases.
Java quiet; df701ee858e2bdd3 on every row. **Kept**, cherry-picked onto the branch.

## Sculk round 2: indexed neighbourhood, run-wide read cache; base 1 -9.6%

**Change** (sub-agent commits 31e1016 + 553a015; `cave/sculk.rs`, unused `Dir::axis` removed from
`cave/support.rs`). Its sampling of the base put ~65% of sculk in `spread_all`, mostly region reads.
- cursors: in world generation vanilla never merges cursors (`!isWorldGeneration()` guards the
  merge), so its per-position map is never read; the list is filtered in place (`retain_mut`), same
  order.
- `spread_all`'s 3x3x3 neighbourhood indexed 0..26 with step/offset/distance tables and the 24
  face/direction pairs; a failed same-position test is reused until the next write (not when
  computing it tallied an unknown shape); `getStateForPlacement` no longer repeats two checks
  `canSpreadInto` just passed on unchanged blocks.
- a per-run read cache over a 32^3 cube around the origin, per thread, stamped per run (nothing
  carries over); every write in the run clears its cell.
- description-cache keys are mixed before picking a slot (section palettes collided); movement
  offsets are a const table; built vein states kept on the feature.
- Draws, write order and written states unchanged.

**Hold, base 1** (pipeline-b6 = 1a87a00 [branch 80f76d3 + datapath **667bdbb**] + the ore cell change,
i.e. the branch head; pipeline-sc6 adds the two sculk commits):

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 8568 | 8033 | 7958 | 8186 |
| sculk decorate_ms | 7354 | 7454 | 7399 | 7402 |
| base / sculk cpu_user_s | 23.48 / 22.69 | 23.76 / 22.38 | 23.38 / 22.37 | 23.54 / 22.48 |
| java_cpu_s_per_10s | 0.20 / 0.21 | 0.24 / 0.20 | 0.23 / 0.20 | |

-9.6% decorate, -4.5% user CPU, ahead 3/3 on both. Java quiet; df701ee858e2bdd3 on every row; tests
195 pass. Second base next.

### Sculk round 2, base 2: -3.9%; kept

pipeline-b7 = this branch at 2979ec0 merged with origin/perf/datapath **464c8d4** (scratch merge
2bb5364); pipeline-sc7 adds the two sculk commits. Tests 195 pass.

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 8262 | 7961 | 7770 | 7998 |
| sculk decorate_ms | 7474 | 8306 | 7272 | 7684 |
| base / sculk cpu_user_s | 22.78 / 22.99 | 24.39 / 23.69 | 23.08 / 22.12 | 23.42 / 22.93 |
| java_cpu_s_per_10s | 0.21 / 0.21 | 0.24 / 0.23 | 0.25 / 0.22 | |

-3.9% decorate, -2.1% user CPU, ahead 2/3 on each. With base 1 (-9.6%, 3/3) it is ahead in 5 of 6
rounds on both measures over two bases. Java quiet; df701ee858e2bdd3 on every row. **Kept**,
cherry-picked onto the branch.

## Profile after ore cells + sculk round 2 (branch fad1f9e + datapath 464c8d4), lite split timers

1 thread r16, digest df701ee858e2bdd3. Decorate CPU **6240 ms** (7344 at the previous profile, 9328 at
the first lite profile). Branch-head tests: chunkgen 192 pass (1 ignored), pregen pass.

| feature | ms | share |
|---|---|---|
| sculk_patch_deep_dark | 666 | 10.7% |
| ores: clay 416, tuff 328, granite 325, diorite 316, andesite 312, gravel 210, dirt 159, coal 148, copper 77, iron 71+ | ~2360 | ~38% |
| dark_forest_vegetation | 333 | 5.3% |
| lush: ceiling 301, vegetation 184, clay 183 | 668 | 10.7% |
| glow_lichen | 276 | 4.4% |
| amethyst_geode | 235 | 3.8% |
| disk_sand 116, classic_vines 103, freeze_top_layer 96, sculk_vein 94, cave_vines 84 | 493 | 7.9% |

Ores remain the largest group at ~230 ns per fresh cell (10.1M fresh cells); next round needs a
sampled view of the ore body rather than wall timers.

## Glow lichen: stop the repeated search pass after a quiet failure; base 1 -4.2% (mixed)

**Change** (ee0ce0d, 12 lines; `vegetation/multiface.rs`, plus an additive
`UnsupportedTally::records()` in `context.rs`): vanilla's search re-tests the same neighbour
`search_range` (20) times per direction. A failed `place_growth` writes and draws nothing; if it also
tallied nothing (`face_full` can tally an unknown shape, so tallies are compared before/after), the
next pass reads the same cell with the same directions and fails the same way, so the loop breaks.
A pass that tallied keeps looping as vanilla does. This is only the early stop from the rejected
lichen/geode patch, without its caches.

**Hold, base 1** (pipeline-b7 = branch 2979ec0 + datapath **464c8d4**; pipeline-gs adds ee0ce0d):

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 8648 | 7678 | 8557 | 8294 |
| stop decorate_ms | 8273 | 8068 | 7498 | 7946 |
| base / stop cpu_user_s | 24.05 / 24.15 | 23.22 / 23.34 | 24.66 / 22.28 | 23.98 / 23.26 |
| java_cpu_s_per_10s | 0.22 / 0.28 | 0.28 / 0.33 | 0.26 / 0.19 | |

-4.2% decorate ahead 2/3; user CPU -3.0% but ahead only 1/3. Glow lichen is ~4.4% of decorate CPU, so
the true effect is at most that; this hold cannot tell. Second base queued. Digest held; tests 195 pass.

### Glow lichen early stop, base 2 (order reversed): +0.4%; rejected

pipeline-b8 = pipeline-b7 + sculk round 2 (= branch head 8d1ba38 merged with datapath **464c8d4**);
pipeline-gs8 adds ee0ce0d.

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| stop decorate_ms (run first) | 8164 | 8470 | 7591 | 8075 |
| base decorate_ms | 7932 | 8492 | 7715 | 8046 |
| stop / base cpu_user_s | 24.66 / 23.06 | 25.12 / 24.94 | 23.27 / 23.85 | 24.35 / 23.95 |
| java_cpu_s_per_10s | 0.32 / 0.21 | 0.26 / 0.29 | 0.21 / 0.28 | |

+0.4% decorate, +1.7% user CPU. Over both bases the change is inside noise: the repeated pass is
rarely reached (most searches break on a non-air neighbour first). **Rejected**; patch at
`docs/perf/rejected-lichen-early-stop.patch`. Digest held.

## Ore round 3 (sampled): per-feature match memo, prefaulted write list; base 1 -3.8%

**How it was sampled** (perf is blocked: `perf_event_paranoid=4`): the sub-agent copied the noise
track's LD_PRELOAD sampler (`~/oxide-perf/noise-prof/prof2.c`, read only; SIGPROF every 2 ms of CPU via
`setitimer(ITIMER_PROF)`, frame-pointer stack walk, dumps return addresses and `/proc/self/maps` at
exit) into its own dir, built oxide-pregen with `-C force-frame-pointers=yes` in a separate target dir,
ran it as `flock -s ~/oxide-perf/.lock nice -n 10 env PROF_OUT=... LD_PRELOAD=.../prof2.so ./pregen-fp
<r16 1-thread args>`, and symbolised with `noise-prof/sym2.py` (`addr2line -f -C -i`). Page faults
were counted separately with `getrusage(RUSAGE_THREAD).ru_minflt` in a throwaway build.

**Where the ore samples were** (base 8d1ba38, 1223 samples ~2.45 s): `writes.push` in `write_id`
~17% (94% of the write list's new pages fault: ~33k faults, ~30 per region, ~13 us each on this VM);
rule matching 12.8% (`MatchCache` rebuilt per blob, slot = palette entry so sections thrashed);
block reads ~17% (cache misses: a z step strides 16 cells); sphere test 10-15%; target interning ~3%.

**Change** (sub-agent commits ee17bcc, 7b28512; `ore.rs`, `region.rs`):
- each ore feature keeps its target ids and a 32-slot match cache with a mixed slot index in a
  per-thread memo tagged by a new `DecorationRegion::serial()`; a region's first blob still interns
  the targets in order (later blobs' interning was already a no-op), so interning order is unchanged.
- a new region's reserved write list (8192 entries, 128 KiB) is populated with one
  `madvise(MADV_POPULATE_WRITE)` instead of ~30 page faults; a kernel without it refuses the call and
  pages fault in as before.
- Draws, visit order (incl. the colliding `tested` index), `writes` order, states, interning order
  unchanged; no float changes.
- Side finding: the whole run takes ~265k minor faults and ~3.5 s system time; only ~33k are the
  write lists, the rest is probably generation-side allocation (not this track's; passed on).

**Hold, base 1** (pipeline-b8 = branch head merged with datapath **464c8d4**; pipeline-o3m adds both):

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 8612 | 7134 | 8283 | 8010 |
| ore r3 decorate_ms | 7300 | 7751 | 8067 | 7706 |
| base / ore cpu_user_s | 24.17 / 22.63 | 22.44 / 23.55 | 23.62 / 23.26 | 23.41 / 23.15 |
| base / ore cpu_sys_s | 2.32 / 1.51 | 1.40 / 1.56 | 2.11 / 1.44 | 1.94 / 1.50 |
| java_cpu_s_per_10s | 0.26 / 0.20 | 0.22 / 0.26 | 0.30 / 0.24 | |

-3.8% decorate ahead 2/3; user CPU -1.1%, sys -0.44 s, user+sys -2.8% (ahead 2/3). Tests 195 pass;
digest held. The base ranged 7134-8612, so a reversed repeat decides.

### Ore round 3, reversed repeat on the same base: -4.4%; kept

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| ore r3 decorate_ms (run first) | 6968 | 8109 | 7550 | 7542 |
| base decorate_ms | 7925 | 8344 | 7406 | 7892 |
| ore / base cpu_user_s | 21.94 / 24.20 | 23.63 / 23.75 | 23.07 / 22.94 | 22.88 / 23.63 |
| ore / base cpu_sys_s | 1.27 / 1.38 | 1.83 / 1.80 | 1.37 / 1.31 | 1.49 / 1.50 |
| java_cpu_s_per_10s | 0.20 / 0.21 | 0.27 / 0.21 | 0.21 / 0.23 | |

-4.4% decorate, user+sys -1.7%, ahead 2/3 on each. Over both holds (6 rounds, either order):
decorate 7624 vs 7951 mean (-4.1%), user+sys -2.2%, ahead 4/6 on each; the direction holds in both
orders. Java quiet; df701ee858e2bdd3 on every row. **Kept**, cherry-picked onto the branch.

## Headline: decorated pregen at 8 threads on the branch head (6142cff + datapath 323b9d4)

`bench-ab-pregen-vps.sh 16 8 3 pipeline-b9` (single tree, 3 rounds, `--no-write`, exclusive lock;
scratch merge 5238497). Tests on the merged tree: chunkgen 196 pass (1 ignored), pregen pass.

| run | cps | generate_ms | decorate_ms | cpu_user_s | cpu_sys_s | java_cpu_s_per_10s |
|---|---|---|---|---|---|---|
| 1 | 319.9 | 1798 | 1437 | 23.31 | 3.06 | 0.18 |
| 2 | 341.4 | 1810 | 1187 | 23.12 | 1.98 | 0.19 |
| 3 | 344.0 | 1788 | 1196 | 23.02 | 2.13 | 0.28 |

Mean **335 cps** with decoration, digest df701ee858e2bdd3 on every row, Java quiet. The merged-tree
pregen rows at the start of this log were 184.3/171.5 (datapath 0e3cf61) and 194.2/183.4 (noise
e6ecc65), so the decorated pipeline is about +75% since then (both tracks' work). PumpkinMC terrain is
about 404 cps on this box (not byte exact). At 8 threads decoration is still ~40% of the two stage
times (generate ~1.8 s, decorate ~1.2 s), so decoration stays this track's lever.

## Decoration driver (sampled across all features): biome gating once per chunk, pooled chain Vecs; base 1 +0.2%

**Sampled** (same LD_PRELOAD sampler, copied into the sub-agent's own dir) at b2bdac5: 3024 of 43450
samples (~6.0 s) under `BiomeDecorator::decorate`. Top inclusive frames: `place_placed` 95.5%,
`placement::run` 90.8%, feature `place`s 76.7%, `OreFeature::place_blob` 33.2%, libc `[ext]` (malloc/
realloc/free) 17.2%, `place_at_cached` 12.9%, hash `get` 11.1%, sculk `update_cursors` 10.9%,
`place_configured_feature` 10.0%, `Modifier::positions` 9.0%, `make_hash` 8.9%.
Shared costs found: `center_biome_levels` rescanning 24 sections per gated feature (125),
`Environment::biome_lists` two SipHash lookups per call (128), `biomes_present` (105), and a fresh
`emitted` Vec at every chain level of every feature run (~300 in `[ext]`).

**Change** (sub-agent be05004; 14 files, 9 of them one-line test updates): `center_biomes()` once per
chunk, `CenterBiomes::levels(wanted)` ORs the accepted biomes' bits; `biome_features` is an AHash map
to a feature-key bit set; `biomes_present` with a u64 mask and a one-entry-palette shortcut; region
state tables reserve 64; `placement::run` borrows position Vecs from a per-thread pool. Its profile:
decoration 3024 -> 2681 samples.

**Hold, base 1** (pipeline-b9 = branch 6142cff + datapath **323b9d4**; pipeline-d9 adds be05004):

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 7254 | 8162 | 7712 | 7709 |
| driver decorate_ms | 7788 | 7250 | 8133 | 7724 |
| base / driver user+sys s | 23.06 / 23.75 | 24.78 / 23.16 | 24.19 / 24.38 | 24.01 / 23.76 |
| java_cpu_s_per_10s | 0.26 / 0.26 | 0.30 / 0.25 | 0.21 / 0.25 | |

+0.2% decorate, user+sys -1.0% (ahead 1/3): no gain visible, against ~11% of decoration predicted by
the profile. Tests 196 pass; digest held. A reversed repeat decides.

### Decoration driver, reversed repeat: -7.9% decorate, CPU -1.8% (ahead 1/3)

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| driver decorate_ms (run first) | 6917 | 7064 | 6760 | 6914 |
| base decorate_ms | 7222 | 8302 | 6989 | 7504 |
| driver / base user+sys s | 22.58 / 22.13 | 22.32 / 23.95 | 21.98 / 22.00 | 22.29 / 22.69 |
| java_cpu_s_per_10s | 0.30 / 0.22 | 0.25 / 0.23 | 0.22 / 0.25 | |

Over both holds (6 rounds): decorate 7319 vs 7607 (-3.8%, ahead 4/6), user+sys -1.4% (ahead 3/6).
Still ambiguous; a third hold (base first) decides. Digest held.

### Decoration driver, third hold (base first): +0.5%; rejected. Hold order now ABBA.

| | run 1 | run 2 | run 3 | mean |
|---|---|---|---|---|
| base decorate_ms | 7507 | 7900 | 7428 | 7612 |
| driver decorate_ms | 7828 | 7223 | 7889 | 7647 |
| base / driver user+sys s | 22.35 / 23.59 | 24.24 / 22.71 | 22.97 / 23.39 | 23.19 / 23.23 |
| java_cpu_s_per_10s | 0.19 / 0.22 | 0.21 / 0.20 | 0.20 / 0.24 | |

Over three holds (9 rounds): decorate 7428 vs 7608 (-2.4%), user+sys -0.9%. The change won only in
the hold where it ran first in each round (-7.9%) and was flat in both holds where it ran second
(+0.2%, +0.5%). That is a position effect, not the change. **Rejected**; patch at
`docs/perf/rejected-decoration-driver.patch`. Digest held on every row.

**Hold script fix:** `bench-ab-pregen-vps.sh` now reverses the tree order on even rounds (ABBA...), so
a slot advantage cancels. Use an even number of rounds (4 rounds at r16, 1 thread is about 4.5 min).
Earlier 3-round holds gave the first-listed tree (usually the base) two first slots; the kept changes
won as the second tree, i.e. against that bias, and the ore round 3 and lush rejections already used
both orders.

### Sculk region memo (sub-agent 6122eaf): -2.4% decorate, -0.5% secs; rejected on ceiling

Sculk's per-state description cache and its interned write ids moved from per-run to a per-feature
memo keyed by `DecorationRegion::serial()` (614 sculk runs over ~204 regions, ~3 per region; 53k
descriptions and 17.6k interns were rebuilt per run). RNG, floats, write order unchanged.

Sampling (frame pointers, r16 1 thread): `update_cursors` 310 -> 295 / 299 samples (2.61% -> 2.43% of
all samples), within about the +/-5% run spread.

| | round 1 | round 2 | round 3 | round 4 | mean |
|---|---|---|---|---|---|
| base decorate_ms | 7673 | 8392 | 7657 | 8411 | 8033 |
| memo decorate_ms | 7786 | 7940 | 7571 | 8063 | 7840 |
| base / memo secs | 22.17 / 22.65 | 23.83 / 23.02 | 22.37 / 22.41 | 23.60 / 23.44 | 22.99 / 22.88 |
| java_cpu_s_per_10s | 0.26 / 0.23 | 0.23 / 0.33 | 0.22 / 0.26 | 0.21 / 0.24 | |

The hold ran ABAB, not ABBA: the sub-agent's tree (5238497) predates the script fix, so the base had
the first slot every round (the bias the kept changes have been winning against). Decorate -2.4%
(ahead 3/4), secs -0.5%. Sculk `update_cursors` is now ~2.5% of the run, so a 7% cut is worth at most
~0.2% overall, below what any hold here can resolve. **Rejected** (no measurable gain, more state);
patch at `docs/perf/rejected-sculk-region-memo.patch`. Tests 196 pass; digest held on every row. TSV
rows carry only the fields the sub-agent reported (it reverted its appended rows).

Also tried and dropped by the sub-agent, all within noise or worse: skipping repeated no-op spreads
(241k spreads made 217k writes, so it almost never applies), 4-byte remembered-block cells, reading
the whole 3x3x3 up front (310 -> 360 samples). What's left in `update_cursors` is the first
region read of each block and the region writes. Sculk is done for now.

### Decoration allocator cut (sub-agent, now e4b7c5c + 79121e4 + 86f691b): -11.2% decorate, kept

Three changes, all inside our decoration files; RNG draws, ids, intern order and write order unchanged:
- `region.rs`: a new state is cloned once on intern (the second copy was a map key); 64 states reserved.
  `placement.rs`: placement chains reuse emptied position lists per thread.
- `region.rs`: the write list reserves 16k entries and prefaults the first 8k, so regions past 8k
  writes no longer realloc-copy (max RSS one run 724 MB; base RSS not measured alike).
- `ore.rs`: each blob reuses the thread's visited-cell bit set.

Sampling (r16 1 thread, two runs each): allocator (malloc/realloc/free) 266/228 -> 158/171 samples
(-33%); decoration 3053/2806 -> 2697/2687 (-8%). The earlier "17% malloc/free" was all of libc; the
allocator itself was 8-9% of decoration. Gone: add_state clone (42 -> 4), placement lists (54 -> 0),
write_id realloc (21 -> 0), ore bit set (22 -> 0). Left: the one intern clone (~23), `pos_set` grow
and vegetation geometry insert (~25 each), state-provider clones (~12), ore sphere lists (~8),
`biomes_present` (~8).

ABBA hold, 4 rounds, r16 1 thread (b9 = branch + datapath 323b9d4):

| | round 1 | round 2 | round 3 | round 4 | mean |
|---|---|---|---|---|---|
| base decorate_ms | 8367 | 8097 | 7927 | 8175 | 8142 |
| alloc decorate_ms | 7238 | 6890 | 7338 | 7468 | 7234 |
| base / alloc secs | 23.25 / 22.28 | 22.90 / 21.36 | 23.18 / 22.45 | 23.31 / 23.57 | 23.16 / 22.41 |
| base / alloc cpu_sys_s | 2.08 / 1.35 | 1.68 / 1.26 | 1.82 / 1.41 | 1.65 / 1.46 | 1.81 / 1.37 |
| java_cpu_s_per_10s | 0.22 / 0.24 | 0.23 / 0.22 | 0.24 / 0.26 | 0.24 / 0.28 | |

Decorate -11.2%, ahead 4/4 in both slot orders; secs -3.2%; sys -24%. Over 10%, so one base. **Kept.**
Cherry-picked onto the branch; branch head tests pass (oxide-chunkgen 192 + others, 0 failed) and a
2-thread r16 run of the branch head gives digest df701ee858e2bdd3. Digest held on every hold row.
New base for holds: `pipeline-m1` (ee3b211 = branch head + datapath 323b9d4); `pipeline-b9` deleted.

### Decoration hash cut (sub-agent f3dfae0, now a3fb9fb): -4.7% decorate over two ABBA holds, kept

Hash lookups were ~15% of decoration on base m1 (409/421 of 2729/2661 samples). Changes, all in
decorator/ and region.rs; RNG draws, write order, written states, intern order unchanged:
- Per-feature biome gate: center-chunk biomes and their levels gathered once per chunk; gated
  features combine those instead of rescanning with a hash lookup per palette entry (the gating part
  of the rejected be05004, which had only been held with the old fixed-order script).
- Each biome's placed features as a bitset behind one AHash lookup (gate + biome modifier), replacing
  two SipHash lookups.
- Region per-state fact cache fetches all of a state's `BlockFacts` bits in one lookup.
- Vegetation patch `PosSet` uses AHash (order still from its insertion list).

Samples: hash 415 -> 281 avg (-32%); biome checks 119/142 -> 28/20; decoration 2695 -> 2586 (-4%).

| hold | m1 decorate_ms | h1 decorate_ms | change | h1 ahead | m1 / h1 secs |
|---|---|---|---|---|---|
| 1 (m1 listed first) | 7806 7668 7453 7200 = 7532 | 7090 7635 7167 8134 = 7507 | -0.3% | 3/4 | 22.57 / 22.81 |
| 2 (h1 listed first) | 8316 7500 7597 7770 = 7796 | 6935 6805 7044 7630 = 7104 | -8.9% | 4/4 | 22.83 / 22.26 |
| both | 7664 | 7305 | **-4.7%** | 7/8 | 22.70 / 22.53 (-0.7%) |

java_cpu_s_per_10s 0.28-0.49 on every row. Hold 1's one loss was a slow h1 run (8134 ms, sys 2.22 s).
Both holds ABBA, so no slot bias; the hold result matches the sampled -4%. **Kept.** Cherry-picked;
branch head tests pass (0 failed) and an r16 2-thread run gives digest df701ee858e2bdd3; digest held
on every hold row. New base: pipeline-h1 (f3dfae0 = branch head + datapath 323b9d4).

Left in hashing: ore rule tag matching (~31), state interning (~35), remaining fact lookups (~60),
block predicates (~45).

### Ore visited-bit-first + 256-slot match cache (sub-agent 3eecd2d): +1.1% over three holds, rejected

`place_blob` split on base h1 (two runs, ~35% of decoration): sphere math and loop control 53-57%
(the per-cell inside-sphere division ~148 samples), block reads 22-25% (memory stalls), rule match
and draws 9%, writes 6%, visited set 3-4%, air check 2%. Change (`ore.rs` only, no float expression
touched): test the visited bit before the sphere test (identical cells, draws, writes and order,
including vanilla's colliding visited index), and grow the region match cache from 32 to 256 slots.
Profile: `place_blob` 938/897 -> 928/849 (-3%), decoration about -2%; skipping the division mostly
moved its time onto the bit test, so the loop is branch-bound, not division-bound.

| hold | h1 decorate_ms | o4 decorate_ms | change | o4 ahead |
|---|---|---|---|---|
| 1 (h1 first, sub-agent) | 7598 7429 7726 7335 = 7522 | 6920 6782 7450 6928 = 7020 | -6.7% | 4/4 |
| 2 (o4 first) | 7382 7137 7948 6991 = 7365 | 8263 7478 7604 7198 = 7636 | +3.7% | 1/4 |
| 3 (h1 first) | 7118 7372 7108 7331 = 7232 | 7401 7986 7448 7963 = 7700 | +6.5% | 0/4 |
| all 12 rounds | 7373 | 7452 | **+1.1%** | 5/12 |

java_cpu_s_per_10s 0.21-0.38 on every row. Hold 1's 4/4 win did not repeat in either order: hold to
hold swings here reach ~7%, larger than a ~2% sampled gain. **Rejected**; patch at
`docs/perf/rejected-ore-visited-first.patch`. Digest df701ee858e2bdd3 on every row.

What remains in `place_blob` is branchy loop control (~half) and cache-missing block reads (~20%);
section-local reads need a view that survives interleaved writes (unsafe in region.rs) and a per-row
sphere bound is the rejected z-run, so ores are parked unless a different formulation appears.

### Decoration profile on h1 (f3dfae0): no feature clears 5%; apply_writes and heightmaps do

Sub-agent, r16 1 thread, two sampled runs plus two runs with throwaway per-feature wall timers
(reverted; digest df701ee858e2bdd3, decorate_ms 7266 / 7491).

decorate_ms split (~3500 samples): features + driver (`BiomeDecorator::decorate`) 73%,
**`apply_writes` (oxide-pregen) 14.6%**, **heightmaps (`decorate.rs`) 7.2%**, region build /
`into_interned` / drop 5%.

Per feature (ms, two runs): sculk_patch_deep_dark 670/602 (~8.7% of decorate_ms),
dark_forest_vegetation 315/303 (~4.2%), ore_clay 290/305, glow_lichen 232/247 (~3.3%),
lush_caves_ceiling_vegetation 227/234, amethyst_geode 210/256, ore_diorite/tuff/granite/andesite
208-227 each, lush_caves_vegetation 147/148, ore_gravel 143/149, lush_caves_clay 127/126,
ore_coal_lower 117/138, ore_dirt 114/153, disk_sand 87/86, freeze_top_layer 83/81, sculk_vein 80/72.
By code (share of decorator samples): ore 37-38%, driver + placement modifiers 17-18% (spread thin),
sculk 12%, vegetation patch 7.4%, tree 7-8%, multiface 5-6%, geode 4%, disk ~2%, rest ~1% each.

The largest non-ore, non-sculk feature (dark forest, ~4%) cannot yield a 5% cut, which is about the
smallest gain holds here resolve. Next targets instead:
- `apply_writes` searches the section palette by full `BlockState` equality per write; resolve each
  interned state to a palette index once per section.
- `heightmaps()` keeps scanning sections below the surface after all six maps are filled and does six
  fact lookups per palette entry; stop once every column is set.

### apply_writes palette index per section + heightmap early stop (now 59af47f + 1e047df): -18.2% decorate, kept

- A, `oxide-pregen/src/generate.rs` `apply_writes`: each interned state's section palette index is
  resolved once per section and reused. The first write resolves exactly as before (first equal
  entry, else inserted), and a palette only grows, so that stays the first equal entry.
- B, `decorate.rs` `heightmaps()`: stop once no column of any of the six maps is unset (a column keeps
  the first counted block from the top), and read each palette entry's facts in one lookup.

Samples (r16 1 thread, two runs): `apply_writes` 484/544 -> 392/355 (-27%), `heightmaps()` 243/259 ->
42/43 (-83%).

One ABBA hold, 4 rounds, three trees (h1 = base f3dfae0, fa = A, fb = A+B):

| | round 1 | round 2 | round 3 | round 4 | mean | change |
|---|---|---|---|---|---|---|
| h1 decorate_ms | 7904 | 7035 | 7351 | 7734 | 7506 | |
| fa decorate_ms | 7243 | 5954 | 7026 | 6256 | 6620 | -11.8% (4/4) |
| fb decorate_ms | 6813 | 6232 | 5809 | 5692 | 6137 | -18.2% (4/4) |
| h1 / fa / fb secs | 23.09 / 23.09 / 22.04 | 21.80 / 20.22 / 21.71 | 22.01 / 22.48 / 20.25 | 22.46 / 20.78 / 20.65 | 22.34 / 21.64 / 21.16 | -3.1% / -5.3% |

java_cpu_s_per_10s 0.18-0.86 on every row. A and A+B each clear 10% in one hold; B over A is -7.3%
(3/4), matching its -83% heightmap samples. **Both kept.** Cherry-picked; branch head tests pass
(oxide-chunkgen, oxide-pregen, oxide-anvil, 0 failed) and an r16 2-thread run gives digest
df701ee858e2bdd3; digest held on every hold row. New base: pipeline-fb (4d750ec = branch head +
datapath 323b9d4).

Next in apply_writes: each finished chunk walks all writes of its nine neighbour regions and keeps
the ones landing in it; bucketing each region's writes by target chunk once should remove most of it.

### apply_writes bucketed by target chunk (sub-agent 7968121): +9.3% decorate, rejected

Each region's writes stable-counting-sorted once by target chunk (in place through a per-thread
buffer), so a finished chunk reads only its own bucket from each of nine neighbour regions; per-chunk
write order unchanged. Premise wrong: skipping foreign writes was only ~35 samples (~1 ns each); the
cost of `apply_writes` is the writes themselves (palette lookups, section stores). Samples:
`apply_writes` + grouping ~374 (base) -> ~456 (fresh list per region) -> ~406 (in place). Max RSS
723 vs 724 MB.

1-thread hold (ABBA, 4 rounds): decorate fb 5808 5809 5933 6155 = 5926, w1 6627 6321 6866 6095 =
6477 (**+9.3%**, w1 behind 4/4); secs 20.72 vs 21.30 (+2.8%); cpu_sys_s 1.27 vs 1.83 (+44%,
untraced). **Rejected**; patch at `docs/perf/rejected-apply-writes-buckets.patch`. Digest
df701ee858e2bdd3 on every row; tests pass.

8-thread r16 hold (same command as the 335 cps headline, 4 rounds):

| round | fb cps / generate_ms / decorate_ms | w1 cps / generate_ms / decorate_ms |
|---|---|---|
| 1 | 323.9 / 1832 / 1322 | 293.3 / 1790 / 1692 |
| 2 | 356.7 / 1841 / 1052 | 336.3 / 1811 / 1207 |
| 3 | 314.6 / 2015 / 1254 | 343.8 / 1854 / 1154 |
| 4 | 206.1 / 1886 / 2834 (cpu_sys_s 11.68: box interference, dropped) | 314.3 / 1896 / 1399 |

Branch head (fb) at 8 threads: 323.9 / 356.7 / 314.6, mean 332 cps, about the 335 headline. The
1-thread decorate cut (-18% since h1, about -25% since the headline base) doesn't show at 8 threads:
8-thread spread is about +/-10% here, and generate (~1.85 s, not ours) is now the larger stage.

### Decoration 8-thread scaling: diagnosis, and 8-byte packed writes (sub-agent 19d5a20) rejected

Instrumented build (per-worker busy time per phase, `getrusage` faults and sys; reverted), base
4d750ec, r16:

| phase | threads | wall s | busy s (all workers) | idle s | minor faults | sys s |
|---|---|---|---|---|---|---|
| make region writes | 1 | 5.45 | 5.45 | 0.00 | 54k | 0.48 |
| make region writes | 8 | 1.18 / 1.53 | 9.4 / 12.2 | 0.06 / 0.07 | 60k | 1.7 / 3.0 |
| apply writes + heightmaps | 1 | 0.92 | 0.92 | 0.00 | 32k | 0.35 |
| apply writes + heightmaps | 8 | 0.61 / 0.30 | 4.8 / 2.4 | 0.04 / 0.03 | 32k | 1.6 / 0.9 |

- Not imbalance or a serial tail: idle under 1% of thread time; the step between phases is ~15 ms.
- Per-work inflation at 8 threads: 1.7-2.2x CPU in the region phase, 2.6-5x in the apply phase.
- Faults barely change with threads (~92k) but sys per fault goes ~9 us -> 28-50 us: faults on fresh
  memory contend in the kernel. Memory is always fresh because the stage keeps every terrain chunk,
  write list and finished chunk until it ends. Region phase: ~39k faults from the prefaulted write
  lists (128 KB per region), ~20k other region allocations. Apply phase: ~31k mostly from finished
  sections expanding to full 16 KB index arrays on first write (oxide-core, not ours).
- The rest is user-time inflation: 8 cores share one 8 MB L3; decoration is memory and cache bound.
- Locks and false sharing are unlikely (one relaxed atomic per region; pools and caches per
  thread) but were not measured separately.

Fix tried: each write in 8 bytes instead of 16 (position packed in 32 bits relative to the region's
48x48 window, beside the state id; same order, positions and states). Instrumented 8 threads: region
phase faults ~60k -> ~40k, wall 1.05/0.93/0.81 s.

Holds:
- 8 threads, 6 rounds: undecidable. Five of twelve rows had cpu_sys_s 15-37 s (earlier 8-thread holds
  2-3 s), on both trees; java quiet (0.15-0.54); box had 17 GB free afterwards. Cause outside the lock,
  untraced. Medians fb 235 cps / 2124 ms, t1 260 cps / 1892 ms.
- 1 thread, 4 rounds: decorate fb 6161 6077 6510 5947 = 6174, t1 6636 6305 6505 6153 = 6400
  (**+3.7%**, t1 behind 3/4); sys 1.45 -> 1.70 s. Possibly the write list now sits at exactly 128 KB,
  glibc's default mmap threshold (untested).

**Rejected**; patch at `docs/perf/rejected-packed-writes.patch`. Digest df701ee858e2bdd3 on every
row; tests pass.

Next: the stage holds every region's write list and every finished chunk until it ends, so memory is
never reused. Free each region's writes once its nine readers are finished, and reuse those buffers,
or overlap the phases so peak memory falls. Fresh 16 KB section arrays (oxide-core) are noted but not
ours.

### Write-list pool + overlapped decoration phases (sub-agent cbc2955, now df31e5c): -6.7% decorate at 1 thread, kept

- `region.rs`: `InternedWrites::recycle()` returns a region's write list to a shared pool (cap 256); a
  new region takes a pooled list (pages already mapped) and prefaults only when the pool is empty. Write
  lists are 256 KB, above glibc's 128 KB mmap threshold, so before this every free returned the pages
  to the kernel and every new list faulted fresh ones. No global malloc settings changed.
- `oxide-pregen/src/generate.rs`: the two phases interleave along z. Batch k decorates region row k and,
  in the same `rayon::join`, finishes chunk row k-3, whose nine regions are done; a finished chunk row
  recycles the region row only it and earlier rows read, so ~4 rows of write lists are alive instead
  of all 1225. Each chunk reads the same base terrain and the same nine complete write lists in the
  same order; base terrain is moved out only once no pending region reads it; finished chunks are
  collected in row order.

Instrumented (`getrusage(RUSAGE_THREAD)` per region and chunk; reverted), r16:

| phase | threads | faults before -> after | sys s before -> after |
|---|---|---|---|
| regions | 1 | 53.8k -> 9.3k | 0.61 -> 0.07 |
| regions | 8 | 61k -> 21k | 2.5 / 1.85 -> 1.32 / 1.21 |
| chunks | 1 | 31.7k -> 30.3k | 0.41 -> 0.26 |
| chunks | 8 | 31.6k -> 25.5k | 1.17 / 0.85 -> 0.48 |
| whole stage | 1 / 8 | 85.5k -> 39.6k / 92k -> 47k | 1.03 -> 0.34 / 3.2 -> 1.8 |

Remaining chunk-phase faults are oxide-core section index arrays (not ours).

| hold | fb decorate_ms | p1 decorate_ms | change | p1 ahead | sys fb / p1 |
|---|---|---|---|---|---|
| 1 thread #1 (fb first) | 6796 6209 6667 6251 = 6481 | 5862 6057 6369 6184 = 6118 | -5.6% | 4/4 | 1.76 / 1.36 |
| 1 thread #2 (p1 first) | 6105 7146 6042 7261 = 6639 | 5951 6255 6031 6269 = 6127 | -7.7% | 4/4 | 1.59 / 1.36 |
| 1 thread both | 6560 | 6122 | **-6.7%** | 8/8 | |
| 8 threads #1 (fb first) | mean 1713 (285 cps) | mean 1543 (306 cps) | -9.9% | 5/6 | 4.62 / 2.75 |
| 8 threads #2 (p1 first) | 1515 1243 1224 3019 1980 1488 = 1745 | 1621 1419 2389 2018 1995 1806 = 1875 | +7.4% | 2/6 | 5.82 / 4.34 |
| 8 threads both | 1729 | 1709 | -1.2% | 7/12 | |

8-thread hold 2 had spike rows on both trees (fb sys 14.29 and 6.15, p1 7.12) and disagrees with hold
1; 8 threads is recorded as neutral. **Kept** on the 1-thread result (8/8 rounds, both orders) and the
lower sys at both thread counts. Checks on the branch head: tests pass (oxide-chunkgen, oxide-pregen,
oxide-anvil, 0 failed); digest df701ee858e2bdd3 at 1, 3 and 8 threads; an off-centre r7 4-thread run
(centre 37,-20) gives the same digest as the base (dea822a7b8517773), and the written .mca files are
byte-identical to the base's except the region timestamp table. Digest held on every hold row.
New base: pipeline-p1 (cbc2955).

The row batching adds a sync per row (37 steps of ~68 items at 8 threads); finishing each chunk as soon
as its own nine regions are done (dependency-driven) may recover the 8-thread wall time.

### Dependency-driven chunk finishing (sub-agent ef5e48b, now f4b0792): -14% to -25% decorate at 8 threads, kept

`oxide-pregen/src/generate.rs` only. Regions run in row order on a FIFO rayon scope
(`scope_fifo`/`spawn_fifo`). Each chunk counts its outstanding regions; the region that completes a
chunk's ninth neighbour queues it. Each region counts the chunks still to read its write list; the
last one recycles it to the pool. Terrain chunks are shared through `Arc`; a chunk's base moves out
without a copy once its nine regions drop their handles. Same base, same complete write lists in the
same order per chunk; finished chunks collected in row order.

Instrumented 8 threads (reverted), two runs: idle per worker ~0.27 s -> ~0.04 s (all workers 2.06/2.27
-> 0.37/0.33 s), but busy time rose (regions 6.64/6.37 -> 7.91/7.28 s, chunks 1.66/1.41 -> 2.47/2.20
s), stage wall 1.30/1.26 -> 1.34/1.23 s. Likely cache locality from interleaving chunk and region work
on the same cores (not measured).

| 8-thread hold (6 rounds) | p1 decorate_ms mean / median | q1 decorate_ms mean / median | change (mean) | q1 ahead | cps p1 / q1 |
|---|---|---|---|---|---|
| 1 (p1 first, sub-agent) | 1740 / 1613 | 1442 / 1464 | -17% | 5/6 | 289 / 298 |
| 2 (q1 first) | 1929 / 1932 | 1662 / 1596 | -13.9% | 4/6 | 268 / 293 |
| 3 (p1 first, after a 10 min wait) | 2599 / 2758 | 1960 / 1831 | -24.6% | 5/6 | 230 / 259 |

Hold 2 had four rows with cpu_sys_s > 6 s (three p1), so it was rerun after 10 minutes as hold 3,
which was noisier still (sys up to 10.3 s on both trees; the box has been noisy at 8 threads all day,
cause outside the lock). All three holds favour q1 in both orders, 14/18 rounds. 1-thread hold (4
rounds): 6193 vs 5867 (-5.3%, 3/4), no regression; no barriers exist at 1 thread, so read as noise.
java_cpu_s_per_10s 0.16-1.29 (one 1.29 row; under the 2 CPU-s cutoff).

**Kept.** Branch head: tests pass (oxide-chunkgen, oxide-pregen, oxide-anvil, 0 failed); digest
df701ee858e2bdd3 at 1, 3 and 8 threads; off-centre r7 4-thread digest dea822a7b8517773 and .mca
files identical to p1's outside the timestamp table. Digest held on every hold row. New base:
pipeline-q1 (ef5e48b).

The 8-thread box noise (sys spikes 6-14 s on a growing share of rows) now limits how small an
8-thread gain can be resolved; absolute cps across days is not comparable until it is traced.

### 8-thread sys spikes traced to glibc arena growth; up-front arena reserve (sub-agent 0b56ce5, now 2359e11): +40% cps at 8 threads, kept

**Diagnosis** (20 timed r16 8-thread runs of q1, one at a time under `flock -x`, ~2-3 min exclusive in
total; /proc/vmstat, PSI and the session cgroup's memory.stat/events diffed per run; per-task
stat/wchan/syscall sampled every 20 ms). Per-run sys time ranged 1.7 s to 19.8 s with:
- no compaction, THP faults, reclaim (pgscan/pgsteal 0), allocstall or drop_pagecache; pgmajfault 0-2;
  memory PSI 0 in every run: **not memory pressure and not another tenant**;
- pgfault flat at 175-215k whatever the sys time: the same faults cost more;
- blocked threads almost all in `D:do_mprotect_pkey` (syscall 10, mprotect), `vma_merge`,
  `lock_mm_and_find_vma`; 100-160 such samples in spike runs vs 25-60 in calm ones.
- `strace -f -c` (one run): **53,233 mprotect** calls, 1,105 madvise, 558 brk, 58 mmap, 19 munmap.
  `strace -k`: all from glibc `malloc` -> `sysmalloc` growing each worker's 64 MB arena heap,
  mostly one 4 KB page at a time. Each mprotect takes mmap_lock for writing, so with 8 threads
  faulting under the same lock they queue; how badly depends on scheduling, hence the spread.

So the "box noise" at 8 threads in the last several holds was largely our own process.

**Fix** (`oxide-pregen/src/main.rs`, 28 lines): after the pool is built, `rayon::broadcast(reserve_arena)`
allocates two 64 MB heaps' worth of untouched 120 KB blocks per worker (below the 128 KB mmap
threshold, so the thread's arena serves them) and frees them, keeping the one block that opened the
second heap so it is not unmapped. glibc keeps a heap's write permission after trimming (it releases
pages with madvise), so the arena then grows with no mprotect during the run. Only malloc's chunk
headers are written (about one page per block, released on trim). No global malloc setting changed.
Relies on current glibc behaviour; on another allocator it is a burst of allocations and frees.
Work-time mprotect calls: ~53k -> 0 (8,616 remain, all at start-up, ~1,076 per worker).

8-thread hold (6 rounds, ABBA, q1 listed first):

| | q1 | k1 | change |
|---|---|---|---|
| cps per round | 306.1 230.0 264.7 289.1 237.2 252.8 | 393.3 358.3 325.0 373.5 378.9 377.4 | |
| cps mean | 263 | **368** | **+40%** (6/6) |
| decorate_ms mean | 1878 | 921 | -51% |
| secs mean | 4.18 | 2.97 | -29% |
| cpu_sys_s mean (range) | 5.19 (2.86-8.10) | 1.60 (1.38-1.88) | -69% |

1-thread hold (4 rounds, k1 listed first): decorate 6174 vs 6297 (-2%, 2/4), sys 1.42 vs 1.36:
neutral, as expected with one arena. java_cpu_s_per_10s 0.14-0.61.

**Kept.** Branch head: tests pass (0 failed); digest df701ee858e2bdd3 at 1 and 8 threads; r7
off-centre digest dea822a7b8517773 and .mca identical to q1's outside the timestamp table; digest held
on every hold row. New base: pipeline-k1 (0b56ce5).

**Against PumpkinMC:** decorated, byte-exact pregen at 8 threads is now 368 cps mean (325-393) on this
box, vs PumpkinMC terrain-only ~404 cps (not byte exact). Earlier 8-thread figures (335 cps headline,
332 on fb) were taken with this contention present and are not directly comparable.

### Overlap generation with decoration (sub-agent 8fac5b9): neutral, rejected

`generate.rs`: terrain chunks queued in row order on the same `scope_fifo` as regions and chunks; a
region starts once its nine terrain chunks exist (counter per region); terrain no finished chunk is
built on is dropped after its last region. Same inputs, write-list order and row-ordered output; r7
byte check vs k1 passes (dea822a7b8517773, .mca identical outside the timestamp table). Max RSS at 8
threads 675/661 MB -> 662/662 MB. (`generate_ms`/`decorate_ms` lose their meaning when overlapped.)

Instrumented busy time at 8 threads: terrain 16.1/17.7 s (vs ~14.6 s when run alone), regions
6.4/6.9 s, chunks 1.4/1.7 s, idle per worker 0.06-0.11 s, wall 3.08/3.37 s. Workers are now busy
almost all the time: the box is compute-bound, and terrain work costs ~10-20% more CPU when it shares
cores with decoration (likely cache), cancelling the idle gap the overlap removed.

| hold | k1 | g1 | change | g1 ahead |
|---|---|---|---|---|
| 8 threads, 6 rounds: cps mean | 310.8 | 308.0 | -0.9% | 3/6 |
| 8 threads: cpu_user_s mean | 26.5 | 26.9 | | |
| 1 thread, 4 rounds: secs mean | 21.64 | 21.71 | +0.3% | 2/4 |

Round 1 of the 8-thread hold was slow on both trees (user ~35 s, java 0.90-1.04 CPU-s per 10 s).
**Rejected** (no gain, more scheduling code); patch at
`docs/perf/rejected-overlap-generate-decorate.patch`. Digest df701ee858e2bdd3 on every row; tests pass.

Takeaway: at 8 threads the run is CPU-bound, so further gains must cut CPU, not reschedule. Per r16
run at 1 thread: generate ~13.9 s (other tracks), decorate ~6.0 s, serialise ~0.65 s, compress
~0.6 s. Reaching PumpkinMC's ~404 cps from ~368 needs roughly 10% less total CPU; within this track
that means decoration (ores still ~37% of it) or serialise/compress.

### Whole-process CPU profile on k1 (0b56ce5), 1 thread r16: no single cut in our code reaches 3%

Sampler on a frame-pointer build, two runs (~11,300 samples each, digest df701ee858e2bdd3), classified
by outermost stage:

| part | share of run CPU |
|---|---|
| generation (other tracks) | 62.7% / 64.2% |
| decoration features | 19.0% / 19.0% (ore 8.7/8.5, sculk 2.8/2.7, vegetation patch 1.9, tree 1.6/1.7, multiface 1.2/1.3, geode 1.0, rest ~1.3) |
| decoration driver + placement chains | 3.7 / 3.2 (largest frame `placement::run` 0.4-0.5%) |
| serialise (oxide-anvil NBT) | 2.9 / 3.2 (~40% `pack_bits` in oxide-core, ~37% libc copy/alloc) |
| compress | 2.9 / 3.2 (flate2 1.1.9, zlib-rs 0.6.8 backend, level 6 = `Compression::VANILLA`) |
| region build, `into_interned`, recycle | 2.5 / 1.7 |
| `apply_writes` | 2.3 / 2.2 (~42% libc: oxide-core palette and index arrays; ~16% per-write target check; ~8% oxide-core `set_index`) |
| start-up (datapack load, feature compile) | 1.8 / 1.7 |
| heightmaps | 0.5 / 0.3 |
| other / scheduler | ~1.5 |

Our code is ~36% of run CPU, generation ~64%. Ranked candidates (best realistic saving of total CPU):
ore placement (8.6%; needs a ~35% ore cut to reach 3%), serialise oxide-anvil side (0.5-1%), sculk
(2.8%, cut twice), driver (3.5%, spread thin), apply_writes per-write check (<=0.4%), region build
(<0.5% each), start-up (1.7% at r16, fixed per run). Compress left alone: zlib-rs is already fast,
and any other implementation risks the .mca bytes.

Out of our scope but worth noting for its owner: oxide-core `pack_bits` and the palette/section index
allocations are ~2% of run CPU across serialise and apply_writes.

Next: one more ore attempt with a formulation not yet tried: evaluate the inside-sphere test for a
whole x row at once (same per-element float operations, no reordering or FMA) into a bit mask, then
visit set bits in the original order, so the branchy per-cell loop control (~55% of `place_blob`)
shrinks.

### Ore row-mask sphere test: -4% place_blob in samples, stopped before holds (bar was -25%)

`ore.rs`: `dz*dz` once per z of each sphere's box (vanilla's expression), and for each (x, y) row that
passes vanilla's row test, every z cell's test `!((dx*dx + dy*dy) + dz*dz >= 1.0)` (same left-to-right
sum, NaN treated as inside as before) into a u64 mask in one branch-free loop; set bits walked in
ascending z (vanilla's inner axis) with visited bit, block read, rule match and draws unchanged.
`.cargo/config.toml` builds with `-C target-cpu=native`; `objdump` finds 0 `vfmadd` in base and new.

| place_blob samples (two runs each) | base 0b56ce5 | row mask |
|---|---|---|
| total | 887 / 951 | 887 / 878 |
| sphere math and loop | 512 / 513 | 411 / 392 |
| block reads | 188 / 251 | 209 / 237 |
| visited set | 32 / 25 | 87 / 77 |

-21% sphere math, but the visited test rose by ~50 samples: the time moves to the next memory access,
as with z-run bounds and visited-first. **Not held, not kept**; patch at
`docs/perf/rejected-ore-row-mask.patch`. Digest df701ee858e2bdd3 on all sampled runs.

**Ores are closed.** Three in-loop formulations end the same way: once the arithmetic goes, ore cost is
cache-missing reads of the visited set and block data, which the loop structure cannot remove.

## Headline: decorated pregen at 8 threads on the merged head (k1 = branch + datapath 323b9d4)

origin/perf/noise 4aee7cf differs from datapath 323b9d4 by one docs commit, so k1 (0b56ce5) holds every
track's kept code. `bench-ab-pregen-vps.sh 16 8 6 pipeline-k1` (single tree, exclusive lock, `--no-write`):

| run | cps | generate_ms | decorate_ms | serialise_ms | compress_ms | cpu_user_s | cpu_sys_s |
|---|---|---|---|---|---|---|---|
| 1 | 384.0 | 1760 | 895 | 92 | 89 | 21.11 | 2.10 |
| 2 | 373.4 | 1745 | 999 | 87 | 86 | 21.85 | 1.89 |
| 3 | 355.0 | 1805 | 919 | 121 | 223 | 22.63 | 1.57 |
| 4 | 377.3 | 1917 | 790 | 82 | 97 | 22.37 | 1.59 |
| 5 | 342.3 | 2162 | 842 | 96 | 82 | 24.25 | 1.60 |
| 6 | 362.7 | 1801 | 984 | 98 | 119 | 22.56 | 1.42 |

Mean **366 cps** (342-384), digest df701ee858e2bdd3 on every row, sys 1.4-2.1 s on every row (no
spikes since the arena fix), java_n=0 on every row: no Java process was running on the box during the hold. Previous
headline 335 cps (b2bdac5) was measured with the mprotect contention present.

Stage split: generate ~1.83 s (62%, other tracks), decorate ~0.90 s (31%), serialise + compress
~0.19 s. PumpkinMC terrain-only ~404 cps on this box (not byte exact): the byte-exact decorated
pipeline is ~9% short. Our track's remaining levers are each under 1% of run CPU (profile above), so
the rest of the gap sits mainly in generation and in oxide-core (`pack_bits`, palette/index arrays,
~2%), both outside this track's files.

### mimalloc as oxide-pregen's global allocator (sub-agent 9d96ad5): +2.0% at 8 threads, -4.5% at 1 thread, rejected

`#[global_allocator] mimalloc::MiMalloc` in `oxide-pregen/src/main.rs` (new dependency `mimalloc =0.1.43`,
libmimalloc-sys pinned 0.1.39; `reserve_arena` kept). Address dependence checked: the ore memo is keyed
by feature address but only looked up, never iterated; `ptr::eq` in the biome grid is a shortcut
before value comparison; content-keyed hash maps only. Digest df701ee858e2bdd3 on every row, r7 byte
check vs k1 passes, tests pass. Max RSS: 1 thread 651 -> 617/624 MB, 8 threads 673/680 -> 665/675 MB.

| hold | k1 | a1 | change | a1 ahead |
|---|---|---|---|---|
| 1 thread, 4 rounds (k1 first): secs | 20.87 | 21.80 | +4.5% | 0/4 |
| 8 threads #1 (k1 first): cps | 349.3 | 373.0 | +6.8% | 4/6 |
| 8 threads #2 (a1 first): cps | 350.1 | 340.2 | -2.8% | 2/6 |
| 8 threads both: cps | 349.7 | 356.6 | +2.0% | 6/12 |
| 8 threads both: decorate_ms | ~1030 | ~770 | ~-25% | 12/12 |

Decoration is faster under mimalloc in every 8-thread row, but generation and sys time rise (a1 sys up
to 4.92 s in hold 2), and the 1-thread run is slower in every round. **Rejected**; patch at
`docs/perf/rejected-mimalloc.patch`.

Lead: decoration still pays ~25% of its 8-thread time for glibc allocation. A decoration-local fix
(per-thread reuse of the remaining per-region and per-feature allocations, or an arena for region
build) could capture that without changing the allocator for generation.

### Decoration per-write allocations cut (sub-agent 845de28, now abfb474): -7.1% decorate at 1 thread, kept

Counted with an `LD_PRELOAD` malloc/calloc/realloc/free shim walking frame pointers (attributed to the
first non-container frame in our crates, decoration stacks only; counts identical at 1 and 8 threads).
Decoration allocations per r16 run: **5,867,271 -> 3,230,664 (-45%)**; frees 4,956,097 -> 2,352,925.

| call site | before alloc / free | after |
|---|---|---|
| `apply_writes` palette entry clones (oxide-core `index_of_or_insert`) | 1,180,893 / 189,483 | unchanged (not ours) |
| `set_block` dropping a passed-in owned state | 0 / 1,357,550 | 0 / 418,620 |
| multiface neighbour clone per search repeat | 1,024,218 / 1,024,218 | 56,092 / 78,925 |
| state provider `Simple` / `Weighted` clones | 844,752 / 527,667 | gone |
| write-list `recycle` drops | 0 / 443,678 | unchanged |
| `biomes_present` `Vec<bool>` per section | 264,600 / 264,600 | gone |
| region `intern` clone | 242,102 | 290,286 (states now interned from a reference) |
| freeze `should_snow` rebuild / block-below clone | 164,665 frees / 105,348 | gone |
| ore blob sphere list | 130,258 / 130,258 | gone (per-thread reuse) |

Changes (draws, writes, intern order, output unchanged): `StateProvider::state_cow` returns a borrowed
state where the provider names one (same draws; `state()` wraps it), used with `set_block_ref` by disk,
block column, simple block, vegetation patch ground and tree ground decorations; multiface reads the
searched neighbour once; freeze passes its compiled snow state and clones the block below only when it
gains `snowy=true`; `biomes_present` marks palette entries in a u64; ore reuses a per-thread sphere list.

| hold | k1 | d1 | change | d1 ahead |
|---|---|---|---|---|
| 1 thread #1 (k1 first): decorate_ms | 5991 | 5620 | -6.2% | 3/4 |
| 1 thread #2 (d1 first): decorate_ms | 6446 | 5934 | -7.9% | 3/4 |
| 1 thread both: decorate_ms / secs | 6218 / 21.30 | 5777 / 20.72 | **-7.1%** / -2.7% | 6/8 |
| 8 threads #1 (k1 first): cps | 386.8 | 360.3 | -6.9% | (two slow d1 rows, user 24-29 s) |
| 8 threads #2 (d1 first): cps | 355.0 | 371.3 | +4.6% | 4/6 |
| 8 threads both: cps | 370.9 | 365.8 | -1.4% (neutral) | |

**Kept** on the 1-thread result in both orders, with 8 threads neutral (as with the write-list pool).
Halving mallocs did not reproduce mimalloc's ~25% 8-thread decorate gain, so that gain is from
mimalloc's per-thread heaps and free-list locality, not call counts. Branch head: tests pass; digest
df701ee858e2bdd3 at 1 and 8 threads; r7 digest dea822a7b8517773 and .mca identical to k1's outside the
timestamp table; digest held on every hold row. New base: pipeline-d1 (845de28).
