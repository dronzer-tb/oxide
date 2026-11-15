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
