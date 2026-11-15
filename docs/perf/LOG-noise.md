# Log: noise track

Bench: `scripts/bench-vps.sh noise 16 8`. Digest gate at radius 16: `a977961a58122047`.

## Tooling notes (read before profiling)

- `perf` is unusable on the bench VPS for the `bench` user (`perf_event_paranoid = 4`, no sudo).
- Substitute: an `LD_PRELOAD` SIGPROF sampler kept on the VPS in `~/oxide-perf/noise-prof/`
  (`prof.so` flat, `prof2.so` with frame-pointer stacks; `sym.py`, `sym2.py`, `ana.py` to
  symbolize and aggregate). Stacks need a build with
  `RUSTFLAGS="-C target-cpu=native -C force-frame-pointers=yes"` and
  `CARGO_TARGET_DIR=../noise-prof/target`. Run everything under `flock ~/oxide-perf/.lock`.
- The agent host is aarch64 with no C linker and no `reference/`: nothing compiles locally.
  Build, `cargo test` and the program dump all run on the VPS.
- A whole-process profile is ~43% digest work (`build_merkle`, blake3, `BlockState` Display).
  That is outside the timed section; exclude stacks containing `merkle`/`blake3`.

## Baseline profile, 2026-11-15 (commit 726d38c, radius 12, 8 threads, generation only)

The noise crate is 46.7% of generation time, inclusive. By entry point:

| Share | Entry | Caller |
|---|---|---|
| 15.4% | `sample_in_chunk(FinalDensity)` per block | `fill.rs:190` |
| 11.9% | corner-grid build (first sample; the Perlin leaves) | `fill.rs:96` |
| 9.6% | `cell_bounds` | `fill.rs:162` |
| 3.6% | `sample_in_chunk` | `Aquifer::preliminary_surface_level` |
| 3.0% | `sample_in_chunk` (vein toggle/ridged) | `ore_veins::vein_state` |
| <1% each | surface, biome climate, aquifer `sample` | |

Other inclusive figures: `ImprovedNoise::noise_with_scale` 10.7%, `ChunkCaches::interpolated`
21.9%, `ChunkCaches::once` 10.1%, `SurfaceSystem::apply` 27.5% (datapath's), allocator ~11%.

`final_density` compiles to 5 ops: `interpolated(0) -> squeeze`, `interpolated(1) ->
range_choice`, `min`. So the per-block cost is cache lookups and lerps, not dispatch.

## Experiments

| # | What | Before (cps) | After (cps) | Digest | Verdict |
|---|---|---|---|---|---|
| 0 | Baseline, 726d38c | - | 142.7 | ok | - |
| 1 | `cell_bounds`: registers on the stack, not three `Vec`s per call | 142.7 | 151.7, 152.0 | ok | **kept** |
| 2 | Cache slots keyed by structure, not node address: `sloped_cheese` (with its blended noise) was three `cache_once` slots, evaluated twice per cell corner | 152.0 | 157.4 (one run; strictly less work) | ok | **kept** |
| 3a | Whole-cell evaluation of cell-batchable programs (`Program::run_cell`) behind `sample_in_chunk`, one shared layer memo | 157.4 | 155.1 | ok | superseded by 3b: fill's per-block share fell 15.4% -> 4.3%, but the ore-vein pass alternates `vein_toggle`/`vein_ridged` per block and evicted the single memo on every call (ore veins 3% -> 11%) |
| 3b | Same, one layer memo per router slot | 157.4 | 163.8 | ok | **kept** |

## Handover, 2026-11-15 (session stopped at its usage limit)

Current: `BENCH chunks=1089 threads=8 secs=6.650 cps=163.8 ms_per_chunk_core=48.85
digest=a977961a58122047` at 4abb9a3 (baseline 142.7, +15%). Exactness is not negotiable: the
target is to beat PumpkinMC on speed while staying byte exact, so the digest gate stays as is.

Profile after 3a (radius 12, generation only): noise crate 36% inclusive, of which corner-grid
build 10.3%, `cell_bounds` 4.4%, aquifer `preliminary_surface_level` 3.4%, ore veins (should
now be ~3% with 3b; not re-profiled). Outside this track: `SurfaceSystem::apply` 34%,
`BlockState` clone/drop ~10%, allocator ~9%.

Nothing is in flight; the tree is clean and pushed. No PR has been opened yet.

New API for the datapath track: `NoiseRouterEvaluator::sample_cell` / `cell_len` fill one whole
cell of `final_density` (lane order `(ly * cw + lz) * cw + lx`), bit-identical to
`sample_in_chunk`. The per-slot memo behind `sample_in_chunk` assumes callers work one layer of
cells at a time (y outermost); a column-wise caller (y innermost) should use `sample_cell`.

Next three things to try:
1. Memoise `cell_bounds` per cell in the layer memo (24.5k calls per chunk, 768 distinct
   cells). Memoise only `Some` results: `None` means a corner grid is not built yet.
2. Corner-grid build, the remaining Perlin cost: evaluate the grid a column or the whole
   lattice at a time and vectorise `ImprovedNoise` with AVX2, each lane in scalar order (no
   FMA). The 2D subtrees under `sloped_cheese` are already cached per column.
3. `preliminary_surface_level` (14 ops with `FindTopSurface`, called from the aquifer with
   y = 0): dump the program (extend `dump_program` in oxide-ffi to other slots) and see whether
   its inner density scan can reuse corner values instead of re-running the tree per step.

## Session 2, 2026-11-15

Auditor notes taken on board: every VPS build, run *and* profile symbolization goes under
`~/oxide-perf/.lock` (session 1's `addr2line`/python passes and a ~1 s `program_sizes` run
did not, and that disturbed datapath's repeats); every gain under 10% is benched twice before it
is logged; `cargo test -p oxide-chunkgen` runs with `oxide-noise` before every commit.

| # | What | Before (cps) | After (cps) | Digest | Verdict |
|---|---|---|---|---|---|
| 4 | Memoise `cell_bounds` per cell, keyed by how many corner grids exist | 163.8 | 168.9, 156.6 | ok | **reverted**: inside the noise, and datapath E1 already cuts the calls from ~24k to 768 per chunk in `fill.rs` |
| 2 (repeat) | 3a1b789 benched again | | 154.6, 169.8 | ok | a 10% spread on identical code: the original 157.4 is not distinguishable from 1's 152.0 at this noise |
| 3b (repeat) | 4abb9a3 benched again | | 189.1, 181.4 | ok | not comparable: the Minecraft servers on the box were stopped between these runs and the earlier ones |

From here on: all 8 cores are free (both Minecraft servers stopped), the bench script is the new
one (builds under `flock -s` at nice 10, only the timed run is exclusive), and every other
build, test or profile on the box uses `flock -s ~/oxide-perf/.lock nice -n 10`. Earlier numbers
need a quiet re-bench before they can be compared with new ones.

### In flight (Opus sub-agents, one scratch worktree and bench name each)

| Bench name | Experiment |
|---|---|
| `noise-1` | Corner grid evaluated a column (49 corners) at a time: `Program::run_lanes`, lane-batched `NormalNoise`/`PerlinNoise`/`BlendedNoise` with octave-outer loops |
| `noise-2` | `preliminary_surface_level` / `find_top_surface`: one exact speedup of the per-step scan |
| `noise-3` | Scalar `ImprovedNoise` micro-optimisations, `improved_noise.rs` only |

Each experiment gets one interleaved A/B pair, run on top of a merge of `perf/datapath`. The
auditor's scratch merge of this branch with datapath benched 401 and 421 cps
(`bench-audit-combined.tsv` on `perf/audit`), and datapath's E1 changed what `fill` asks of this
crate. Gate: on the merged tree, the patched digest equals the baseline digest.

OmniRoute is no longer used, at the user's direction (too slow). It was also down for every
listed model when tried: gemini flash over quota, `auto/kimi` without tool calls, kimi-web's
upstream token expired, deepseek and gemini-pro returning empty answers.

| # | What | Before (cps) | After (cps) | Digest | Verdict |
|---|---|---|---|---|---|
| 5 (noise-1) | Corner grid a column at a time: `Program::run_lanes` + lane-batched Perlin/Normal/Blended noise with octave-outer loops, bit-identity test added | 415.0 (merged with datapath b7e9f59, A/B baseline) | 401.1 | a977961a58122047 both | **rejected**: exact, but no gain. Overlapping lanes did not pay for the extra lane arrays and `Vec` traffic. Interleaving lanes in scalar code is not enough; real SIMD (AVX2 gathers, which are slow on Zen 1) would be needed, or less noise work per corner |

Merged-tree baseline (this branch + `perf/datapath`): **415.0 cps**, digest a977961a58122047.
New experiments are judged against that.
| 6 (noise-2) | `find_top_surface` keeps the density registers between steps and re-runs only the y-dependent ops (16 of 25). The offset/factor splines are recomputed at every 8-block step because the aquifer samples outside the chunk's 17x17 2D cache. Exact, with a bit-identity test | 438.3 (merged 447defb) | 410.0 | a977961a58122047 both | **not landed**: no visible gain in one pair. Patch kept at `/tmp/noise-2-final.patch` while that tree exists. Untried follow-up: the upper bound (ops 0-12) and step 1 compute the same splines once per call |
| 7 (noise-3) | `ImprovedNoise`: doubled `[u8; 512]` permutation (mask each coordinate once, no masking on the nested lookups), `#[inline(always)]` on the hot helpers. Float work untouched. Old implementation kept as a bit-identity reference test over 64 seeds x 4000 points, with negative and saturated coordinates | 385.7 (merged 367bb13) | 443.5 | a977961a58122047 both | **kept**: exact and strictly fewer instructions. Gain not yet separated from noise (see below) |

The three merged-tree baselines so far are 385.7, 415.0 and 438.3 on identical code, a 13% spread.
A single A/B pair cannot resolve anything under ~15% at that noise, even on the quiet box.

### Profile of this branch merged with datapath (before 7), generation only

Noise crate is 55.6% inclusive. Corner-grid build 33% (`once` caches inside it 23%, 2D column
caches 11%). `ImprovedNoise` 25.6%. `preliminary_surface_level` from the aquifer 9.9%; most of
its calls land outside the chunk's 2D caches. Per-block `final_density` through the cell memo is
no longer visible.

In flight: `noise-4`, corner columns shared across chunks (9 of a chunk's 25 corner columns are
its neighbours' edge columns). `noise-5`, a shared per-column memo of `preliminary_surface_level`,
gated on a compile-time proof that the program does not depend on y. Both caches store the
full key with each entry, so a hit is exact.
