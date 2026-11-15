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

## Handover, session 2 (stopped at the usage limit)

Head of `perf/noise`: 6dfa05a plus log commits. Kept this session: 7 (doubled permutation table).
On this branch merged with datapath: patched 443.5 against baseline 385.7. Baselines on identical
merged code spread from 385.7 to 438.3, so the size of the gain is unconfirmed. Digest unchanged:
`a977961a58122047`.

**In flight when stopped.** Both were stopped mid-bench, with no BENCH line recorded. Diffs are
committed under `docs/perf/wip/` and apply on top of 6dfa05a merged with `origin/perf/datapath`:
- `noise-4-shared-corner-columns.patch`: corner columns shared across chunks through a
  direct-mapped, sharded cache on `NoiseRouterEvaluator` that stores full keys. The agent
  reported both crates' tests passing before it queued its patched bench.
- `noise-5-shared-surface-memo.patch`: a shared per-column memo of slots proven y-independent
  at compile time (`preliminary_surface_level`). `oxide-noise` tests passed; `oxide-chunkgen`
  test status was not reported before it stopped.

**Next three things to try:**
1. Finish noise-4 and noise-5: re-run both crates' tests on the merged tree, then one A/B pair
   each. These target the two largest costs left in this crate: the corner-grid build (33%) and
   the surface level outside the chunk (10%).
2. To measure gains under ~15%: bench A and B back to back two or three times. Single pairs on
   this box spread 13% even with all 8 cores free.
3. Real AVX2 `ImprovedNoise` across a column of corners (gathers on the doubled table). Scalar
   lane interleaving (5) gave nothing, so this only pays with SIMD arithmetic.

## Session 3, 2026-11-15

`perf/noise` fast-forwarded to `origin/perf/datapath` 18bb0f9, which already contains this
branch up to f2b239a and benches at about 614 cps. New experiments are A/B'd against that head,
two pairs each. Datapath's profile puts the corner-grid build at 44.5% of generation.

| # | What | Verdict |
|---|---|---|
| 8 (noise-5) | Shared per-column memo of `preliminary_surface_level`, gated on a compile-time y-independence proof (34 noise + 192 chunkgen tests passed) | **dropped unbenched**: datapath landed the same idea as E18 (a6c1fcd, a per-thread memo keyed by `NoiseRouterEvaluator::id()`, +3.8%), and two memos of one value must not both land. The patch is kept at `docs/perf/wip/noise-5-shared-surface-memo.patch` for its `y_independent_ops`/`cache_pure` analysis |
| 9 (noise-4) | Corner columns shared across chunks: a set-associative cache (64 sharded `Mutex`es x 128 sets x 4 ways, about 13 MB) on `NoiseRouterEvaluator` stores full `(slot, x, z)` keys. `build_corner_grid` reads a chunk's 16 edge columns from it and computes the rest as before. Geometry is checked before it is attached. The bit-identity test asserts the expected hit counts on shared edges | 571.0, 636.2 (A/B baselines at 18bb0f9) | 686.2, 641.8 | a977961a58122047 all four | **kept**: mean +10% (664.0 vs 603.6). The pairs disagree (+20%, +1%), but patched is higher in both |
| 10 | `INLINE_REGISTERS` 16 -> 64: nested programs inside `final_density`'s interpolated argument reach 40 instructions, so 16 meant a heap `Vec` per corner evaluation | 682.0, 622.0 (A/B baselines at d2433fb) | 623.5, 614.6 | a977961a58122047 all four | behind in both pairs; the 1-thread verdict is in the next section. Likely cause: 512 bytes of stack per nesting level costs more than the allocation it saves |

### Method from here on (auditor, after 9)

Every experiment is judged by datapath's method: four or more alternating rounds of prebuilt
binaries in ONE exclusive hold (`scratchpad/ab.sh` builds each tree under `flock -s` at nice 10,
then runs A B A B under `flock -x` and records the load average at the start and end of the
hold). Small changes run on `--threads 1`, where the same binary spreads ~8% instead of 15-25%.
Noise-4's gain was kept on two disagreeing 8-thread pairs, which is below that bar. The auditor
is re-running it against 18bb0f9 for AUDIT.md. This log adds hit/miss/eviction and lock
contention counters for the shared column cache, plus an 8-thread vs 1-thread comparison, so
contention is ruled out rather than assumed.
| 11 (noise-6) | AVX2 `ImprovedNoise`, 4 independent octave samples per call through a sample queue (Perlin, Normal and Blended), accumulated in the original order. Add/sub/mul/div only, no FMA; range guard to scalar at 2^30; `dx = d - cvtepi32(i)` so -0.0 matches; 6 new bit-identity tests (800k lanes plus verbatim copies of the old callers) | 657.0, 723.0 (A/B baselines at d2433fb) | 608.5, 625.9 | a977961a58122047 all four | **rejected**: behind in both pairs, about -10%. Zen 1 splits 256-bit ops in two; building the gradient vectors from ~96 scalar loads per 4 samples, plus the queue's array traffic, outweighs the float work. Patch kept at `docs/perf/wip/noise-6-avx2-octaves.patch`. A retry should build the gradients with table permutes and keep the hashes in SIMD registers, and profile before benching |

### Hold 1: 1 thread, 4 alternating rounds, one exclusive hold (load 1.25 at start, 1.77 at end)

Radius 16, `--threads 1`, ms of core time per chunk. All 16 runs `digest=a977961a58122047`.

| Build | r1 | r2 | r3 | r4 | Mean |
|---|---|---|---|---|---|
| head d2433fb (noise-4 on) | 10.29 | 10.35 | 10.71 | 10.90 | 10.56 |
| head + `INLINE_REGISTERS` 64 (10) | 10.45 | 10.27 | 11.12 | 10.48 | 10.58 |
| head + cache counters | 10.03 | 9.97 | 10.24 | 10.65 | 10.22 |
| 18bb0f9 (noise-4 off) | 11.19 | 11.16 | 12.41 | 11.46 | 11.56 |

- **10 rejected**: level with the head (10.58 vs 10.56). It was behind in both 8-thread pairs too.
- **9 (noise-4) on 1 thread: -8.6% core time per chunk**, ahead in every round. The auditor's
  own hold confirms it independently (`bench-audit-noise.tsv` on `perf/audit`): ahead in all
  four 8-thread rounds, +17% on the means (673.0, 669.4, 721.8, 686.9 against 491.3, 649.3,
  613.2, 591.5 cps), and 10.37/10.58 against 11.41/12.07 ms per chunk on 1 thread. Kept.
- The counters (hits, misses and evictions under the lock, contention via `try_lock` failing)
  cost nothing measurable.

Shared column cache counters, 1 thread, radius 16 (identical in every round, since generation
order is deterministic on one thread): **hits 63,296, misses 53,728, hit rate 0.541, evictions
22,220, contended acquisitions 0 of 170,752.** The hit rate is near its ceiling. A chunk looks
up its 16 edge columns per grid, but only 7 of them are new on average (12 non-corner columns
shared by 2 chunks, 4 corners shared by 4), so the most it can reach is 9/16 = 0.5625, less a
little at the region's edge. The evictions do not cost hits.

### Hold 2: 8 threads, 4 alternating rounds, one exclusive hold (load 2.68 at start, 2.14 at end)

Radius 16, `--threads 8`, cps. All 8 runs `digest=a977961a58122047`.

| Build | r1 | r2 | r3 | r4 | Mean |
|---|---|---|---|---|---|
| head d2433fb + counters (noise-4 on) | 611.1 | 781.7 | 739.0 | 588.1 | 680.0 |
| 18bb0f9 (noise-4 off) | 630.4 | 682.9 | 642.5 | 597.6 | 638.4 |

Shared column cache counters per 8-thread run:

| Round | Hits | Misses | Hit rate | Evictions | Contended | Acquisitions | Contended rate |
|---|---|---|---|---|---|---|---|
| 1 | 59,404 | 57,620 | 0.508 | 26,100 | 189 | 174,644 | 0.00108 |
| 2 | 58,717 | 58,307 | 0.502 | 26,797 | 175 | 175,331 | 0.00100 |
| 3 | 58,552 | 58,472 | 0.500 | 26,952 | 209 | 175,496 | 0.00119 |
| 4 | 57,696 | 59,328 | 0.493 | 27,809 | 145 | 176,352 | 0.00082 |

**Lock contention is ruled out.** About 0.1% of acquisitions had to wait (145-209 per run out of
~175k), and each wait is one short critical section (a 49-value copy). The 8-thread gain is
+6.5% on these means: ahead in three rounds, behind in the first. The auditor's hold had +17%.
On 1 thread it is -8.6% core time per chunk. The 8-thread hit rate (0.49-0.51) is a little
under the 1-thread one (0.541), because chunks finish out of neighbour order and a few more
shared columns are evicted before their neighbour asks for them.

Possible follow-up, small: the cache has 32,768 column slots and evicts ~27k columns per
radius-16 run. A pregen order that walks neighbours (the pipeline's choice), or a larger cache,
could take the hit rate to its 0.5625 ceiling, which is worth at most a few % more of the
corner grid.

| # | What | Hold | Base (ms core/chunk) | Patched | Digest | Verdict |
|---|---|---|---|---|---|---|
| 12 | Gated add: `add(c, mul(gradient, body))` with c a nonzero finite constant, the gradient a `y_clamped_gradient` reaching exactly 0, and `body` proven finite is lowered to `Op::GatedAdd`, which runs `body` (now a nested program) only where the gradient is nonzero. With g = +/-0 and a finite body the product is +/-0, and c + (+/-0) = c bit for bit. Finiteness comes from a conservative magnitude analysis over the program: noise bounds from the real amplitudes (`ImprovedNoise` <= 2 without smear, <= 4 + \|y_scale\| with it), blended noise from its params, splines from coordinate and point bounds, and `None` for `invert`, `weird_scaled_sampler` or anything unbounded. On the overworld it gates both slides around `final_density`'s interpolated argument: above y=256 and at y=-64, `sloped_cheese` and every cave noise are skipped at 10 of 49 corner levels | 1 thread, 5 rounds, one exclusive hold (load 5.24 at start, 1.14 at end) | 10.28, 10.74, 10.06, 10.99, 10.08 (mean 10.43) | 9.88, 9.62, 9.68, 9.88, 9.81 (mean 9.77) | a977961a58122047 all ten | **kept**: -6.3% core time per chunk, ahead in all five rounds |

Tests on the combined tree (12 + the cache counters): oxide-noise 34 passed (new
`gated_add_is_bit_identical_and_skips_where_the_gradient_is_zero`, all four operand orders, every
y from -64 to 320 against the plain formula, with the gate exercised; `gated_add_needs_a_finite_body`,
where `invert` keeps the plain lowering and its NaN), oxide-chunkgen 190 + 1 + 1 + 1 passed.
Correction from the auditor: a high one-minute load average at the start of a hold is not
evidence of unlocked work. A hold starts the moment the builds it waited for end, so that
average is always high then. Only a process seen in `ps` during the hold would count.

### Corner-grid cost model at 8a85270 (instrumented scratch build, radius 16, 8 threads)

Profile: the corner grid is now **27.6% of generation** (33% before 9 and 12; datapath's profile
had it at 44.5%). Inside it, the noise leaves are ~57% and the interpreter and caches ~41%;
`eval_op`'s own dispatch is ~5%.

Noise evaluations during corner-grid builds, per noise:
- `old_blended_noise` (inside `sloped_cheese`): 708,942, up to 40 octaves each (8 main plus 16
  per limit stack, one stack skipped when saturated). Roughly half of all `ImprovedNoise` work.
- Cave noises (`spaghetti_3d_*`, `spaghetti_roughness*`, `cave_entrance`): 708,942 each, the
  same count as blended. The cave branch runs at every corner where `sloped_cheese` runs, so
  there is no further branch to skip in slot 0.
- `cave_cheese`, `spaghetti_2d*`, `pillar*`, `cave_layer`: 272,154 each.
- Noodle (`noodle`, `noodle_thickness`, `noodle_ridge_a`/`_b`, interpolated slots 1-4):
  855-873k each, one octave.

Per cell (`run_cell`): 372,209 cells evaluated (342 of 768 per chunk; the rest culled). Noodle
slots 2-4 were needed in 248,251 of them (67%), slot 0 and 1 in all of them.

**Lazy noodle grids not pursued**: building slots 2-4's corners only for the cells that need
them would save well under half of three single-octave noises, under 1% of generation. Slot 0
cannot be lazy, because the culling bounds read every one of its corners.

What is left in this crate is the per-sample cost of `ImprovedNoise`. `noise-7` (in flight) tries
SIMD within one sample: the 8 corner dot products and 7 lerps across lanes, gradients from a
per-hash `[gx, gy, gz, 0]` table, no cross-sample queue (the cost that sank 11).

### Housekeeping

- `bench-noise.tsv` now carries the hold runs of each kept head, as datapath's does:
  d2433fb (hold 1, 1 thread), d2433fb+ (hold 2, 8 threads; the "+" is the cache counters that
  later landed in 8a85270) and ff02449 (hold 3, 1 thread). The BENCH fields are the hold's own;
  the timestamps are approximate (backfilled in run order, since the holds did not record
  them).
- The bench box disk was at 96%. My finished dirs there are deleted (`noise-1` to `noise-6`,
  `noise-merged`, `count`, the finished `ab-*` trees and their binaries, `noise-prof/target`).
  From here on each scratch dir is deleted as soon as its experiment is logged.

| # | What | Hold | Base (ms core/chunk) | Patched | Digest | Verdict |
|---|---|---|---|---|---|---|
| 13 (noise-7) | SIMD within one `ImprovedNoise` sample: the 8 corner dot products as two 4-lane vectors (`[d0,d4,d2,d6]`, `[d1,d5,d3,d7]`) from a shared 32-byte-aligned `GRAD4[16]` table of `[g0,g1,g2,0]`, in-lane unpacks only; x-lerp in 256 bits, y-lerp on the 128-bit halves, z-lerp scalar. Same ops and operands per lane, no FMA. 36 noise tests (640k kernel calls and lattice-edge cases with exact `dx == 1.0` and +/-0) and 190+3 chunkgen tests passed | 1 thread, 5 rounds, one exclusive hold | 9.54, 9.98, 9.96, 8.91, 9.21 (mean 9.52) | 9.77, 9.67, 10.22, 9.38, 9.56 (mean 9.72) | a977961a58122047 all ten | **inconclusive, not landed**: behind in 4 of 5 rounds, but only +2% core time, inside the ~4% build-to-build floor datapath measured, so one pair of builds cannot decide it. A sample is bound by its dependent-load chain (`p[xm]`, `p[a]`, `p[k]`, gradient row), not by its flops. SIMD cuts instructions but not the chain, and the shuffles add latency, while the scalar code already overlaps independent octaves. Patch kept at `docs/perf/wip/noise-7-simd-within-sample.patch` |

Three ways to vectorise `ImprovedNoise` have now been tried: scalar lanes across corners (5),
AVX2 across octaves (11) and SIMD within one sample (13). None wins on Zen 1: 11 lost about 10%,
and 5 and 13 are within noise of the base. What remains is
doing fewer samples (cache or skip) or shortening the load chain, not wider arithmetic.

The CORNER counters were identical on all ten runs (hits 63,296, misses 53,728, evictions
22,220, contended 0), which also confirms the 1-thread counts logged above.

### Measurement floor (auditor, after 13)

Two separately built binaries of the same source can sit ~4% apart on 1 thread (datapath's
four-way hold; its two-head-build hold is measuring the floor now). Until that floor is known,
nothing is kept or rejected on a gap under 5% from one pair of builds; every hold includes a
second fresh build of the base. Of this session's verdicts, 12 (-6.3%, 5/5 rounds) and 9
(auditor's hold) clear that band; 13 (+2%) does not and is relabelled inconclusive.

## Handover, session 3 (2026-11-15, at ~80% of the usage limit)

Head of `perf/noise`: **8f3738f**. It contains `perf/datapath` 18bb0f9 plus this session's
kept work, digest `a977961a58122047` throughout:
- **9** (d2433fb): corner columns shared across chunks. 1 thread -8.6% core per chunk; 8
  threads +6.5% (mine) and +17% (auditor). Lock contention ruled out (0.1% of acquisitions).
  Hit rate 0.541, ceiling 0.5625.
- **12** (ff02449): gated add. Slide bodies are skipped where their gradient is exactly 0.
  1 thread -6.3%, 5/5 rounds.
- 8a85270: the column cache's hit, miss, eviction and contention counters, printed when
  `OXIDE_CORNER_STATS` is set. They cost nothing measurable.

Reference: head 1-thread ~9.5-9.8 ms of core time per chunk at radius 16 (hold 3, noise-7's
base). The corner grid is ~28% of generation and its noise leaves just over half of that, so this
crate's share is now ~16% plus small callers (aquifer, surface, climate).

**In flight**: `noise-8` (Opus sub-agent, worktree `/tmp/noise-8`, bench dirs `noise-8`,
`ab-*` on the box). It batches the corner grid a column at a time (experiment 5's structure,
patch in `docs/perf/wip/noise-1-lanes.patch`) and reuses the 8 lattice hashes across
consecutive lanes of one octave when the cell is unchanged (exact: the hashes depend only on
the cell). It reports a measured reuse rate and a 5-round 1-thread hold against two fresh base
builds. If the session ends first, its diff will be at `/tmp/noise-8-final.patch`.

**Tooling** (scratchpad, not in the repo; recreate from these descriptions if it is gone):
- `vps-run.sh <tree> <name> '<cmd>'` syncs a tree to `~/oxide-perf/<name>` and runs the
  command under `flock -s ~/oxide-perf/.lock nice -n 10`.
- `ab.sh <rounds> <threads> <radius> label=tree ...` builds each tree under the shared lock and
  copies the binary to `~/oxide-perf/ab-bin/<label>`. Then, in one `flock -x` hold, it alternates
  them, printing `round r label t=<unix> BENCH... CORNER...` and the load average at both ends.
- The profiler is an LD_PRELOAD SIGPROF sampler in `~/oxide-perf/noise-prof` (`prof2.so`
  frame-pointer stacks; `sym2.py`, `ana.py`, `ana2.py`). Build with
  `-C force-frame-pointers=yes` and `CARGO_TARGET_DIR=../noise-prof/target`, and delete that
  target afterwards (disk).
- On the box, delete each scratch dir as soon as its experiment is logged.

**Next three things to try**:
1. Finish noise-8. If hash reuse wins, extend it to the per-block and aquifer callers.
2. Fewer samples per corner: the noodle-cave toggle (slot 1) and slots 2-4 are four single-
   octave grids built in every chunk (~10% of the noise-leaf work). A per-cell lazy build saves
   under 1% (measured above), so only revisit this if the corner grid shrinks further.
3. With the noise leaves near their Zen 1 floor, the remaining large costs are datapath's
   (surface, aquifer, fill). Help there through narrow additive APIs in this crate, like the
   y-independence predicate datapath's E27 needs, rather than with more leaf micro-work.

### API for datapath's E27: `NoiseRouterEvaluator::is_y_independent(slot)`

A predicate on the compiled program, replacing the parse of `dump_program` text. It is true
when the slot's value is a function of x and z alone: `sample_in_chunk` gives bit-identical
values at every y through any chunk's caches, and `sample` is likewise y-independent. The two
entry points can differ from each other: with caches a `flat_cache` node samples at the quart
origin, without them at the exact position. The analysis comes from noise-5 (`y_independent_ops`
plus `cache_pure`, extended for `GatedAdd`). A 2D `shifted_noise` qualifies only with a constant
`shift_y` that is neither -0.0 nor NaN, a plain `noise` with `y_scale` 0 does not (`y * 0.0`
keeps the sign of y), and any `interpolated` node disqualifies the program.

On the real overworld (radius-16 harness datapack): temperature, vegetation, continents,
erosion, ridges and preliminary_surface_level are **true**; depth, final_density, barrier and
lava are **false**. That matches E27's hand-made argument. The new test
`y_independence_predicate` covers 8 shapes: the climate shape, nonzero and -0.0 `shift_y`, 3D
noise, plain 2D noise, interpolated, a gradient, and a sum. Where the predicate is true it
checks bit identity at 7 y values, through two chunks' caches and without caches. Tests:
oxide-noise 35, oxide-chunkgen 190 + 1 + 1 + 1, all passed. This is an API, not a speed change,
so there is no hold.

| # | What | Hold | Bases (s per run, median) | Patched | Digest | Verdict |
|---|---|---|---|---|---|---|
| 14 (noise-8) | Corner grid a column at a time (`Program::run_lanes`, experiment 5's structure, now with `GatedAdd` per lane) and lane-batched Normal/Perlin/Blended noise. Each octave reuses the last lane's 8 lattice hashes when `(i, j, k)` is unchanged (`ImprovedNoise::noise_with_scale_reusing`), with the float work unchanged. Measured reuse on the real datapack: Perlin octaves 0.845, blended 0.340, all 0.648 (`hash-stats` feature). Tests: noise 37 (3 new bit-identity tests), chunkgen 190 + 3 | 1 thread, 5 rounds, base + fresh base2 + batch, one exclusive hold (load 5.78 at start) | base 10.963, base2 11.208 (2.2% apart; 0.1% without base2's outlier round 4) | 10.667 | a977961a58122047 all fifteen | **inconclusive, not landed yet**: ahead of base in 5/5 and base2 in 4/5, but -2.7% / -4.8% by median, under the 5% bar. Patch kept at `docs/perf/wip/noise-8-column-batch-hash-reuse.patch`. A 7-round confirmation hold with two fresh bases is running |

### For datapath: APIs in flight that cut surface and aquifer sampling (handover item 3)

From datapath's latest profile (11ccdeb, 1 thread): surface 16.5% (2.4% of it `column_state`
noise), aquifer 12.9% (fill 10.5%). Today the aquifer reaches this crate through
`router.sample(...)` with no caches (`Erosion`/`Depth` in `surface_level`, `Lava` per cell,
`Barrier` lazily per block pair), so every cache node evaluates straight through. The surface
samples 2D `NormalNoise`s directly at `(x, 0.0, z)`. Two sub-agents work on trees merged with
datapath b55a3b2:
- `noise-9`: profiles the aquifer's noise share per slot, then adds ONE narrow exact API in
  `oxide-noise` for the largest part. One candidate is a per-slot column memo of a program's
  y-independent ops (depth's offset spline), with the y-dependent ops run per call in their
  original order. The minimal `aquifer.rs` caller change is kept as a separate diff for datapath
  to land.
- `noise-10`: an exact 2D fast path inside `ImprovedNoise`, with no API change. For y = +/-0.0,
  `y_scale == 0` and `yo != 0`, `d1 == yo` exactly, so the y lattice coordinate, fraction, fade
  and y hashes are instance constants. It speeds up the surface noises, climate, `continents`,
  `erosion`, `ridges` and the shift noises without any caller change.
Both are judged on a 5-round 1-thread hold against two fresh base builds (bar: >5% against both,
or ahead in every round). What lands, and the caller diff to take, will be stated here.
