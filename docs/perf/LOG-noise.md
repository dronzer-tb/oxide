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

Queued for after noise-9 and noise-10 are decided: datapath's request (its parked E29) for
`is_quart_invariant(slot)`. Its ceiling is ~1.3% of generation, so it is only worth doing if it
is a few lines on top of `is_y_independent`. Every exclusive hold of this track stays under
~5 minutes: 5 rounds x 3 binaries at 1 thread is ~3 minutes, and the noise-8 confirmation
(7 x 3) ~4.

### 14 (noise-8) confirmed and kept (01528de): 7-round hold, two fresh bases (load 1.22 at start, 1.38 at end, ~4 min)

Radius 16, `--threads 1`, ms of core time per chunk. All 21 runs `digest=a977961a58122047`.
The bases are b1e4a5c (65ba9fc, same crates) built twice; batch is the same head plus the patch,
three-way merged. That is the tree landed as 01528de.

| Build | r1 | r2 | r3 | r4 | r5 | r6 | r7 | Mean | Median |
|---|---|---|---|---|---|---|---|---|---|
| base | 10.07 | 10.16 | 9.55 | 9.35 | 9.34 | 10.18 | 9.77 | 9.774 | 9.770 |
| base2 | 10.25 | 10.06 | 10.64 | 9.58 | 9.55 | 10.01 | 9.98 | 10.010 | 10.010 |
| batch | 9.16 | 8.82 | 8.87 | 8.77 | 10.35 | 9.29 | 8.80 | 9.151 | 8.870 |

**14 kept**: -6.4% against base and -8.6% against base2 by mean (-9.2% and -11.4% by median),
ahead of both in 6 of 7 rounds (round 5 lost). The two bases are 2.4% apart, so this clears the
5% bar against both, and agrees with the first hold (ahead of base 5/5, of base2 4/5). Tests on
the landed tree: oxide-noise 38, oxide-chunkgen 190 + 1 + 1 + 1. Lattice-hash reuse on the real
datapack: 0.648 of `ImprovedNoise` samples overall (Perlin octaves 0.845, blended 0.340),
counted with the `hash-stats` cargo feature. That feature is off by default; when on, it adds a
`HASH ...` line next to `CORNER` while `OXIDE_CORNER_STATS` is set. Batching alone (5) was
break-even, and the gain is the reuse.

| # | What | Hold | Bases (cps, mean) | Patched | Digest | Verdict |
|---|---|---|---|---|---|---|
| 15 (noise-10) | 2D fast path in `ImprovedNoise`. When `yo != 0`, `ym`, `dy`, `dy - 1` and `smoothstep(dy)` for y = +/-0 are precomputed at construction (`y + yo == yo` exactly for both zeros). `PerlinNoise` takes `noise_2d` when `y == 0.0 && y_scale == 0.0`, checked once per call, and both paths share one `lerp_core` with the same operands in the same order. A forced `yo == 0` instance takes the general path. 42 noise tests (old implementations kept as references, both zeros, `dx == 1.0` forced), 193 + 3 chunkgen | 1 thread, 5 rounds, two fresh bases, on b93770d (b1e4a5c + datapath b55a3b2) | 111.84, 108.94 | 111.74 | a977961a58122047 all fifteen | **inconclusive, not landed**: level with base10, inside the build-to-build spread. It led both only in rounds 1-2, while the load was still falling. The saved work (one floor, subtract, smoothstep and mask per 2D octave sample) is too small to see. Patch kept at `docs/perf/wip/noise-10-flat-y-fast-path.patch` |
| 16 (noise-9) | `PointMemo` + `sample_with_memo`: with no chunk caches, each cache node remembers its last exact position and value, which is exact for any program (no snapping, no blend; slots shared only by equal fingerprints). The aquifer's `surface_level` uses it for Erosion and Depth (caller diff for datapath). 37 noise tests (34k mixed-slot samples, foreign-router memo ignored), 193 + 3 chunkgen | 1 thread, 5 rounds, two fresh bases (labels n9b1/n9b2), on b93770d | 107.6, 110.9 | 108.3 | a977961a58122047 all fifteen | **rejected**: +0.6% / -2.4%, inside the build spread. Its ceiling was small: see the profile below. Patches kept at `docs/perf/wip/noise-9-point-memo*.patch` |

**For datapath: the aquifer's cost is not noise.** noise-9's split profile (1 thread, radius 16,
temporary `#[inline(never)]` per slot): the aquifer is 10.78% of generation (entries:
`compute_kind` from the fill 67%, from `carve_ellipsoid` 25%, the constructor ~4%). Of that,
**83.5% is the aquifer's own logic** (9.0% of generation; hot spots `note_run`'s per-candidate
loop, aquifer.rs 529-563, and the nearest-centre loop in `compute_kind`). Depth is 12.4% (1.34%),
of which 64% is noise under the offset spline and 29% the spline itself. Erosion is 3.6%
(0.38%), Barrier 0.3% and floodedness 0.2%; lava, spread and preliminary surface level got 0
samples. No API in this crate can cut the aquifer by more than ~1.7% of generation. The rest is
in datapath's loops.

**Process error, mine**: the noise-8 confirmation's cleanup ran `rm -f ab-bin/base*`. That glob
also matches other holds' `base9`, `base9b` and `base10*`, so it deleted noise-9's base binaries
mid-experiment (its first hold printed only the patched lines and had to be re-run) and very
likely caused noise-10's first hold to produce nothing. From now on every cleanup deletes exact
labels only (`ab-bin/<label>` and `ab-bin/<label>.stderr`), never a glob.

### API for datapath's E29: `NoiseRouterEvaluator::is_quart_invariant(slot)`

True when, **through chunk caches**, the slot's value at `(x, y, z)` equals its value at
`(x & !3, y, z & !3)`, bit for bit. A caller may then sample once per quart and reuse the value
for the other 15 columns at the same y; with `is_y_independent` as well, once per quart column.
It holds through `flat_cache`, which samples at the quart origin whatever its argument reads.
It does not hold through `sample()`, where nothing snaps. `cache_2d`/`cache_once` qualify only
if their argument does, `interpolated` never does, and noise leaves read the exact x/z. About
50 lines on top of `is_y_independent` (`quart_invariant_ops`), plus the test
`quart_invariance_predicate`: 8 shapes, and where true, bit identity at 4 offsets x 3 quarts x
4 y x 2 chunks. Tests: oxide-noise 39, oxide-chunkgen 190 + 1 + 1 + 1. This is an API, not a
speed change, so there is no hold.

Real overworld (both predicates):

| Slot | y-independent | quart-invariant |
|---|---|---|
| temperature, vegetation | true | **false** (bare `shifted_noise`, sampled at the exact x/z) |
| continents, erosion, ridges, preliminary_surface_level | true | true |
| depth | false | **true** (y gradient + `flat_cache`d offset) |
| final_density, barrier, lava, fluid levels, veins | false | false |

So per quart, continents, erosion, ridges and depth need one sample per (quart, y). Depth is
still per y, but its offset part is shared by the whole quart through the flat cache.
Temperature and vegetation must still be sampled per exact column, which is what E27 already
does.

### Design (s5 reserved 16:00-17:00 CET; nothing queued): 17, column-uniform x/z work

With 14 landed, a corner-grid column is evaluated as one 49-lane batch whose lanes share `x`
and `z` exactly. The y-dependent leaves see the same x/z in every lane: `NormalNoise`'s
`x * INPUT_FACTOR`, `shifted_noise`'s shifts (2D caches), and blended noise's `limit_x`. Yet
every lane of every octave recomputes the x/z half of the sample. Split per octave:

- **Per column** (identical in all lanes): `wrap(x * freq)` and `wrap(z * freq)`, `x + xo`,
  `z + zo`, their floors and `as i32`, `dx`, `dz`, `dx - 1.0`, `dz - 1.0`, the fades
  `t = smoothstep(dx)` and `v = smoothstep(dz)`, `xm`, `zm`, and the first-level hashes
  `p[xm]`, `p[xm + 1]`.
- **Per lattice cell** (already shared by 14's reuse): the 8 corner hashes. With them, each
  corner's `A = g0 * xc` and `C = g2 * zc`, where `xc` is `dx` or `dx - 1`, `zc` likewise and
  `g = GRADIENT[h & 15]`. `grad_dot` is `g[0]*x + g[1]*y + g[2]*z`, which Rust evaluates as
  `(g0*x + g1*y) + g2*z`, so `(A + g1*yc) + C` performs the same operations on the same operands
  in the same order.
- **Per lane**: y's wrap, `y + yo`, floor, `dy`, `dy - 1`, the smear, the fade `u`, the eight
  `g1*yc` and two adds per corner, and the 7 lerps.

That is roughly 70 float ops per reused lane instead of ~140. Ceiling: the corner grid's noise
leaves are ~14% of generation, so a few % of generation.

Exactness: compute each per-column value once with exactly the expression the per-lane code
uses (the operations are deterministic). The uniform path is taken only when every lane's x and
z are bitwise equal to lane 0's, checked once per call, with a fallback to today's path.
Signed zeros: `A`, `C` and the per-lane products are the same IEEE products as before, and the
two adds keep their order. Tests: bit-identity (`to_bits`) against `noise_with_scale` over
columns crossing cells, negative and wrapped x/z, `dx == 1.0` exactly, and y-smear; end to end
against `PerlinNoise::get_value` and `BlendedNoise::compute` per lane, plus the non-uniform
fallback. To be measured after 17:00 CET with a 5-round 1-thread hold against two fresh bases
(labels `n11b1`, `n11b2`, `n11`).

Draft of 17 written while s5 was reserved: `docs/perf/wip/noise-11-column-xz-DRAFT.patch`
(worktree `/tmp/noise-11`). It has not been compiled, because this host has no linker. It adds
`ImprovedNoise::column_xz` and `sample_y`, a `ColumnCell` memo holding each corner's `A`, `g1`
and `C`, and `uniform_xz`. `PerlinNoise::get_values` and `BlendedNoise::compute_chunk` (main and
both limit stacks) branch on `uniform` with a plain if/else, so the per-octave
`freq`/`amp`/`pow` updates stay single. Three new tests: `column_path_is_bit_identical_along_columns`
(32 seeds, 6 x/z incl. -0.0, tiny and wrapped-large, 3 smear modes, reuse asserted),
`get_values_column_path_matches_get_value` (uniform and mixed batches) and
`compute_lanes_column_path_matches_compute` (overworld, nether and end parameters). Next: once
the box is free, `cargo fmt`, both crates' tests, then the two-base hold.

## Handover, session 4 (2026-11-15 ~16:10 CET, at ~76% of the usage limit)

Head of `perf/noise`: **33fc192** (code head fd4bfe2). Kept this session, digest
`a977961a58122047` throughout:
- **14** (01528de): corner columns evaluated lane-batched, with lattice-hash reuse. 1 thread:
  -6.4% / -8.6% against two fresh bases, ahead in 6 of 7 rounds.
  **Open question:** datapath's one 8-thread row after merging it is 392.9 cps against ~700,
  timed just before the s5 reservation, so it may be an artefact. Datapath's sub-agent runs the
  8-thread hold of that merge after 15:55 UTC; read it rather than duplicating it. If the drop
  is real, the first suspect is allocation inside the batch path. `run_lanes` allocates
  `bx/by/bz`, `sx/sy/sz` and `regs` per call (and `run_subset` per branch), and `Batch::new`'s
  `cache_once` memo allocates per column. That is harmless on 1 thread but can contend in the
  allocator on 8. Fix: reuse per-thread scratch buffers (a `thread_local!` or a scratch arena
  owned by `ChunkCaches`). Fallback: gate the batch path off with a single switch in
  `build_corner_grid_shared` (go back to per-corner `run`). **From now on, every keep in the
  shared corner-cache path also needs an 8-thread hold.**
- APIs for datapath: `is_y_independent(slot)` (b1e4a5c) and `is_quart_invariant(slot)`
  (fd4bfe2), each with tests and a real-overworld table above.
- Rejected or inconclusive, patches in `docs/perf/wip/`: 13 (SIMD within a sample), 15 (2D
  fast path), 16 (aquifer point memo; its profile shows the aquifer is 83.5% datapath logic).

**In flight:** 17 (noise-11, column-uniform x/z work). It is drafted but not compiled, in
`/tmp/noise-11` and `docs/perf/wip/noise-11-column-xz-DRAFT.patch`. A session cron fires at
17:07 CET to resume it once the s5 reservation ends. Steps: `cargo fmt -p oxide-noise`, both
crates' tests, then a 5-round hold with two fresh bases on **1 thread and on 8 threads** (labels
`n11b1`/`n11b2`/`n11`), each hold under ~5 minutes. Delete box dirs by exact label, never by
glob.

**Next three things to try:**
1. Settle 14 at 8 threads (above), then 17.
2. If 17 lands, the per-lane remainder is the y half and 7 lerps. The next exact cut is
   per-cell reuse of `dy`-independent lerp terms where `u` repeats (only when consecutive lanes
   share `dy`, which is rare). Probably not worth it; re-profile first.
3. This crate's remaining share is mostly the corner grid's leaves (~14% of generation at the
   last profile). The larger costs now sit in datapath (surface 16.5%, the aquifer's own loops
   ~9%). Further help from here is narrow APIs on request, like the two predicates.

| # | What | Holds | Bases | Patched | Digest | Verdict |
|---|---|---|---|---|---|---|
| 17 (noise-11) | Column-uniform x/z work: `ImprovedNoise::column_xz` once per octave per column, `sample_y` per lane with a `ColumnCell` memo of each corner's `g0*xc`, `g1` and `g2*zc`, taken when `uniform_xz` (bitwise x and z equal in every lane) holds, in `PerlinNoise::get_values` and `BlendedNoise::compute_chunk`. It compiled first time (only the three touched files are formatted). Tests: noise 42 (3 new bit-identity tests), chunkgen 190 + 1 + 1 + 1 | 5 rounds, two fresh bases at the head (fd4bfe2 crates), **1 thread** and **8 threads**, labels n11b1/n11b2/n11 | 1 thread: 101.4, 97.5 cps; 8 threads: 601.9, 596.9 | 1 thread: 100.1; 8 threads: 687.7 | a977961a58122047 all thirty | **inconclusive, not landed**: on 1 thread (the measure of record for small changes) -1.4% / +2.6%, ahead 2/5 and 4/5. On 8 threads +14.3% / +15.2%, ahead 4/5 of each, but each base has one 455-cps outlier round, typical of 8-thread spread. An exact change with no 1-thread effect is unlikely to give 15% at 8, so this is not trusted. Patch at `docs/perf/wip/noise-11-column-xz.patch` |

**For datapath's open question about 01528de at 8 threads:** this hold timed two fresh builds of
the head, which contains 01528de, at **601.9 and 596.9 cps** on 8 threads (per round: 696.7,
454.8, 608.7, 597.4, 651.8 and 640.3, 665.4, 454.9, 658.3, 565.7). That is far from the suspect
392.9 row, so it points to that row being disturbed rather than a regression. Datapath's own
8-thread hold of the merge is still the measure of record. Box note: the Java server started for
the live test (16:13 CET) was still running at 17:07, idle by `top`. Holds were started under it.

| # | What | Holds | Bases | Patched | Digest | Verdict |
|---|---|---|---|---|---|---|
| 18 | Per-thread reusable scratch (`scratch.rs`: a `thread_local!` free list per element type, RAII `Scratch<T>` returning on drop, at most 64 buffers of up to 1M elements). It replaces every per-call allocation on the batch and cell paths: `run_lanes` (`bx/by/bz`, `sx/sy/sz`, `regs`), `run_cell` (`regs`, `other`), the range-choice partitions, `interval_select` (per-lane index, then function order: no nested `Vec<Vec>`), the gated add's `open`/`body`, `cache_once`'s `missing` and per-batch memo slots, `run_subset`, and `Program::run`'s spill above 16 registers. There is no arithmetic change: `Scratch::filled(n, v)` gives exactly `vec![v; n]`'s contents. Tests: noise 40 (new `reused_buffers_start_as_fresh_ones`), chunkgen 190 + 1 + 1 + 1 | 5 rounds, two fresh bases at the head, 1 thread and 8 threads, labels scb1/scb2/scr | 1 t: 98.0, 97.1 cps; 8 t: 584.2, 623.9 | 1 t: 101.5; 8 t: 586.0 | a977961a58122047 all thirty | **unmeasured, not landed: the box was disturbed.** The Java server on s5 (pid 1742511, restarted after the reservation) used **64 CPU-seconds per ~10.5 s run on average (max 99)** during the 1-thread hold and 51 (max 110) during the 8-thread hold, i.e. ~5-6 cores busy while timing. On 1 thread the change was +3.6% / +4.5%, ahead 4/5 against both; on 8 threads +0.3% / -6.1%, ahead 2/5, with 397/453 cps outlier rounds. Neither hold can decide it. Patch at `docs/perf/wip/noise-12-scratch-pool.patch`. Re-hold when the Java server is stopped or idle (`java_cpu_s` near 0 per run) |

**For the auditor:** `ab.sh`'s timed loop now runs `ab-bin/noise-holdloop.sh` on the box, which
prints `java_cpu_s` (the server's utime+stime delta during each run) on every round line. The
reading shows the Java server is not idle. A one-off `top` snapshot at 17:07 said it was, but it
peaked above 6 cores during this hold, and over 5 s afterwards it used 1.4 cores (129% average
over its 45-minute life). Every hold on s5 while it runs, datapath's included, is suspect unless
its per-round Java CPU is near zero.

### 18 re-held under the auditor's method (rotating rounds, CPU-seconds, Java CPU per round)

Method: `noise-holdloop.sh` rotates the start label each round and wraps every run in
`/usr/bin/time -f '%U %S'`. `ab-analyze.py` drops a round if any run in it saw the Java server
above 2 CPU-s per 10 s wall. **No round was dropped**: Java used 0.7-1.4 CPU-s per ~36 s run on
1 thread, 0.5-0.8 per ~27 s run on 8 threads. Labels sp0a/sp0b (two fresh builds of 13cbfb2,
crates = fd4bfe2) and sp1 (the same plus `noise-12-scratch-pool.patch`). Digest
`a977961a58122047` in all 42 runs.

**Allocations** (LD_PRELOAD counter `noise-prof/alloccount.so`, whole process, 1 thread, radius
16, deterministic):

| build | malloc-family calls | bytes |
|---|---|---|
| sp0a (head) | 247,240,858 | 11,821,129,637 |
| sp1 (pool) | 238,957,124 | 5,210,679,059 |
| difference per chunk (/1089) | **-7,607 calls** | **-6.07 MB** |

That matches the auditor's estimate (~7.2k allocations and 6 MB per chunk from fill). The
remaining ~219k calls per chunk are whole-process (setup included) and outside this patch.

| hold | sp0a cpu_s / cps | sp0b cpu_s / cps | sp1 cpu_s / cps | sp1 vs sp0a | sp1 vs sp0b |
|---|---|---|---|---|---|
| 1 thread, 7 rounds | 36.21 / 102.8 | 36.33 / 103.8 | 35.40 / 106.0 | cpu -2.2%, fewer in 5/7; cps +3.1% | cpu -2.5%, fewer in 5/7; cps +2.1% |
| 8 threads, 7 rounds | 37.59 / 690.9 | 37.48 / 687.5 | 36.68 / 717.6 | cpu -2.4%, fewer in 5/7; cps +3.9% | cpu -2.1%, fewer in 4/7; cps +4.4% |

**Verdict: inconclusive, not landed** (below the bar: not >5% against both bases, not ahead in
every round). All four comparisons point the same way, and the allocation cut is real and
deterministic, so the patch stays in `docs/perf/wip/noise-12-scratch-pool.patch` as the first
candidate to re-decide with a sharper measure.

**Why the CPU column is blunt here (corrected):** `cpu_s` is whole-process. A 1-thread run takes
~36 s wall, of which only ~10.4 s is generation (`secs`). I first wrote that the other ~26 s was
setup; **that was wrong.** A 4-round radius-0 hold (labels su0/su1, one chunk each) puts setup at
**0.36-0.48 CPU-s** in both builds (su0 0.39/0.48/0.37/0.36, su1 0.46/0.42/0.36/0.36). The ~25 s is
the merkle digest that `--time-only` computes after the timed part (`build_merkle` per chunk,
single-threaded, `crates/oxide-harness/src/main.rs:262`). It is identical code over identical
chunks, but it dilutes a generation effect about 3.5x on 1 thread and its own noise lands in
`cpu_s`. If the digest's CPU is equal, sp1's -0.81 CPU-s on 1 thread is ~8% of generation CPU
(still an inference).

**Measure fixed without a harness change:** `noise-holdloop.sh` now also logs `gen_cpu_s`. The
harness's stdout goes through a fifo, and when it prints `generated N chunks` (the end of the
timed part, before the digest), the loop reads the harness's own utime+stime from
`/proc/<pid>/stat` with a blocking `read`, with no polling. That is setup (~0.4 s) plus
generation only. `ab-analyze.py` reports it first. 18 is being re-held on it.

### 18 decided on generation CPU: no measurable effect, not landed

Rotating holds of 4 rounds at radius 16. Labels su0/su0b are two fresh builds of 319e212
(crates = fd4bfe2), and su1 is the same plus `noise-12-scratch-pool.patch`. `gen_cpu_s` (setup +
generation, no digest) was logged on all 24 runs. Java used 0.5-1.1 CPU-s per run; no round was
dropped. Digest `a977961a58122047` on all 24.

| hold | su0 gen_cpu_s | su0b gen_cpu_s | su1 gen_cpu_s | su1 vs su0 | su1 vs su0b |
|---|---|---|---|---|---|
| 1 thread | 11.10 | 10.91 | 10.93 | -1.6%, fewer in 3/4 | **+0.2%**, fewer in 3/4 |
| 8 threads | 13.24 | 12.00 | 12.38 | -6.5%, fewer in 3/4 | **+3.2%**, fewer in 1/4 |

Per round, 1 thread: su0 11.33/11.12/11.05/10.91, su0b 10.83/11.19/10.79/10.83, su1
12.03/10.58/10.60/10.51. 8 threads: su0 12.70/13.69/14.37/12.19, su0b 12.68/11.54/11.83/11.95,
su1 13.78/11.43/12.37/11.95.

**Verdict: no measurable effect; not landed.** On the measure that excludes the digest, the pool
sits between the two bases at both thread counts. The earlier whole-process -2.2%/-2.5% and my
"~8% of generation" inference are therefore not supported: that gap was in the digest's CPU,
i.e. noise. The cut of 7.6k allocations (6.07 MB) per chunk is real but buys nothing measurable,
so glibc's malloc already serves these short-lived same-size buffers cheaply from its thread
caches, even at 8 threads. The patch stays in `docs/perf/wip/noise-12-scratch-pool.patch`.

**Method notes:**
- At 8 threads the two bases differ by 10% in generation CPU (13.24 vs 12.00, both builds of
  the same commit). Generation CPU at 8 threads is no steadier than cps, so 8-thread holds
  still need two bases and more rounds than 1-thread ones.
- These holds ran ~7 min (1 thread) and ~5.5 min (8 threads) per lock hold, over the ~5 min
  guidance. 25 s of each run is the untimed digest. A `--time-only` flag that skips the
  digest would halve hold length, but it would also drop the per-run byte-exactness check, so
  it is not proposed. The digest stays.

## Handover, session 5 (2026-11-15)

Head of `perf/noise`: see git log. **Update:** 18 (scratch pool) was reopened by the auditor and
**landed** over datapath's head (see "18 landed" below); 17 is rejected.
- The measurement tooling is in this session's scratchpad (not in the repo):
  `noise-holdloop.sh` (rotating rounds, `gen_cpu_s` via a fifo plus `/proc/<pid>/stat`,
  `cpu_s` via `/usr/bin/time`, `java_cpu_s`) and `ab-analyze.py` (drops rounds where Java
  used more than 2 CPU-s per 10 s, then compares `gen_cpu_s` first). **Decide on `gen_cpu_s`,
  not `cpu_s`**: the latter is 70% digest.
- Next: 17 (column x/z, `wip/noise-11-column-xz.patch`) was judged on cps under a busy Java
  server. Re-hold it on `gen_cpu_s` at 1 thread first. It is the only open candidate with a
  plausible exact gain inside this crate.

### 17 re-held on generation CPU: no effect, rejected

1 thread, radius 16, rotating rounds. Labels cx0/cx0b are two fresh builds of e0ba0b6 (crates =
fd4bfe2) and cx1 adds `wip/noise-11-column-xz.patch`. The hold was started at 5 rounds and **cut
after round 4** (the auditor's 4-round limit, which arrived mid-hold), so it held the exclusive lock
~7 min. Java used 0.7-0.9 CPU-s per run; no round was dropped. Digest `a977961a58122047` on all 12.

| | cx0 | cx0b | cx1 | cx1 vs cx0 | cx1 vs cx0b |
|---|---|---|---|---|---|
| gen_cpu_s (mean of 4) | 10.79 | 10.49 | 10.59 | -1.8%, fewer in 3/4 | +0.9%, fewer in 2/4 |
| cps | 104.2 | 107.1 | 106.3 | +2.0% | -0.8% |

Per round gen_cpu_s: cx0 11.16/10.68/11.00/10.31, cx0b 10.38/10.31/10.77/10.51, cx1
10.78/11.08/10.38/10.12.

**Verdict: rejected.** On 1 thread it sits between the bases, as in the first hold. The +15% at
8 threads seen under the busy Java server is not credible for an exact change with no 1-thread
effect, and it is not re-held. The patch stays in wip.

**18 is not closed (auditor):** datapath measured noise-12 stacked on its head aa8ffab (E32+E33)
at +9% at 1 thread, ahead in 7/8 rounds, against my no-effect result on fd4bfe2. The noise crate
is the same in both, so any real gain must come from datapath's new call patterns (more
allocating `run`/batch calls per chunk). Datapath is measuring noise-12 alone on its head. If it
confirms, I re-hold on gen_cpu_s over aa8ffab code and land it if it holds. If not, 18 stays
closed.

### 18 landed (b889079): scratch pool re-held over datapath's head aa8ffab

Datapath measured noise-12 alone on aa8ffab on generation-only CPU (its four-way hold, 26f512f):
**-5.7% at 1 thread, below both bases in 4/5 rounds; -5.1% at 8 threads, 4/6.** My own re-hold
over the same code: labels pa0/pa0b are two fresh builds of aa8ffab, and pa1 adds the patch. 4
rotating rounds per thread count, radius 16. Java used 0.4-1.1 CPU-s per run; none dropped.
Digest `a977961a58122047` on all 24.

| hold | pa0 gen_cpu_s | pa0b gen_cpu_s | pa1 gen_cpu_s | pa1 vs pa0 | pa1 vs pa0b |
|---|---|---|---|---|---|
| 1 thread | 10.38 | 10.09 | **9.62** | -7.3%, fewer in 3/4; cps +7.0% | -4.7%, fewer in 3/4; cps +4.9% |
| 8 threads | 11.09 | 13.18 | 11.02 | -0.7%, fewer in 2/4 | -16.4%, fewer in 3/4 (pa0b has 16.06/13.22/12.75 outliers) |

Per round, 1 thread gen_cpu_s: pa0 10.32/9.80/10.45/10.94, pa0b 10.01/9.55/10.53/10.28, pa1
9.52/9.88/9.67/9.42.

**Verdict: landed, with one caveat stated plainly.** My hold alone misses the bar by 0.3 pp
against one base (-4.7%, not >5%, and 3/4 rounds, not every round). The decision rests on two
independent holds agreeing: mine at -6.0% against the bases' mean, datapath's at -5.7%. Both
are exact (digest on every run). At 8 threads it shows no regression (-0.7% against the steady
base) and datapath has -5.1%; this patch touches the shared corner path, so that 8-thread
check was required. Tests: noise 40, chunkgen 190 + 1 + 1 + 1.

**Why fd4bfe2 showed nothing:** the same crate showed no effect over fd4bfe2 code (10.93 against
10.91/11.10). Over aa8ffab it shows -6%, so the gain depends on datapath's merged call patterns
since then (its base is also ~0.8 s faster per run). I have not traced which callers allocate
more there. It shows when `perf/noise` is merged with datapath; on `perf/noise` alone,
expect no change.

## Session 5, continued: measuring over the shipping tree (auditor)

From now on, every hold is measured over **the shipping tree**: datapath's head with this crate
merged in. Datapath merged `perf/noise` at ef9a35f, so that is ef9a35f itself. (My `/tmp/ship`,
80add17 plus b889079's `crates/oxide-noise`, has crates identical to ef9a35f.) fd4bfe2 on its own
behaves differently (see 18). Two shared base binaries, `ab-bin/shipb1` and `ab-bin/shipb2`, are
separate builds of that code. `ab.sh` now takes `label=@` for a prebuilt binary and `0` rounds
to build only. Holds are 4 rounds at **radius 12**: 625 chunks, ~21 s per run, so a 3-label
hold is ~4 min. The radius-16 digest is checked separately under the shared lock.

**Profile of the shipping tree** (`profile-tree.sh`, frame pointers, radius 12, 8 threads,
2,947 generation samples). Noise is **45.7%** of generation:

| Share of generation | Where |
|---|---|
| 24.0% | corner grid. Self time: `grad_dot` 3.9%, `lerp` 2.4%, `smoothstep` 1.2%, `noise_with_scale_reusing` ~1.6%, `floor` 0.5%, `get_values`/`compute_chunk` ~1.8% |
| 7.5% | per-block `run_cell` excluding the grid. `fill_cell_interpolated` ~2.6% (1.5% of it in range iterators), `[ext]` libc 1.5% |
| 4.2% | ore veins through noise: `sample_in_chunk` from ore_veins.rs:224 (1.8%), :197 (1.4%), :105 (0.9%) |
| 4.9% + 1.7% | the aquifer's `preliminary_surface_level` / `surface_level` samples (datapath's callers) |

Also in self time: `atomic_load::u32` 2.6% (to identify: the `CornerColumns` mutexes or
thread-local access) and `[ext]` 16% overall.

**In flight:** three Opus sub-agents, one exact experiment each. They share a brief
(`scratchpad/subagent-brief.md`) and are judged on `gen_cpu_s` at 1 thread against shipb1/shipb2,
4 rounds. The corner-path one also runs an 8-thread hold.
- **20a (n20a):** the corner grid's Perlin math, vectorized *across lanes* in phases (coordinates
  and smoothstep, hash gathers, gradient dots and lerps), with the same per-lane IEEE expression.
- **20b (n20b):** per-block `run_cell` / `fill_cell_interpolated`: bounds-check-free,
  vectorizable lane loops, and fewer copies.
- **20c (n20c):** ore-vein sampling, a cheaper `sample_via_cell` hit path. If that falls short,
  a cell-batch API with a minimal call-site change in `ore_veins.rs`.

**Two smaller findings from the same profile:**
- **For datapath / oxide-core:** the 2.6% `atomic_load::u32` self time is `Once::is_completed`
  inside `LazyLock<[f32; 65536]>::force`. That is `SIN_TABLE` in
  `crates/oxide-core/src/mth.rs:18`, forced on every `mth::sin`/`cos` call from the carvers. It is
  exact to deref it once per hot loop (`let table = &*SIN_TABLE;`) or to make the table a plain
  `static` filled at build time. Not this crate's; reported only.
- **Noise crate, bundling candidate:** 1.8% of generation is allocation under
  `ChunkCaches::column_cached` (cache.rs:585). That is `vec![None; COLUMNS]` (289 `Option<f64>`)
  per 2D slot per `ChunkCaches`, and the aquifer builds one per neighbouring chunk
  (aquifer.rs:976). It is exact to take those vectors from the scratch pool. At 1.8% it cannot
  clear the 5% bar alone; bundle it with the next landing in the same area.

| # | What | Holds | Bases | Patched | Digest | Verdict |
|---|---|---|---|---|---|---|
| 20c (n20c) | Ore-vein sampling: the per-slot cell memo goes from one `RefCell<[CellMemo; 16]>` to `Cell` tags plus `OnceCell<Box<[Cell<f64>]>>` values, so a hit is plain loads with no borrow flag. `sample_via_cell` and `sample_in_chunk` are `#[inline]`; misses go to an out-of-line `fill_memo` (`run_cell` into a pooled buffer, then a copy). Each value is the same `run_cell` output, so it is exact. New test `interleaved_sparse_memo_reads_are_bit_identical`. Tests: noise 41, chunkgen 192 + 1 + 1 + 1 | 4 rounds, 1 thread, radius 12, rotating, against shipb1/shipb2 | gen_cpu_s 5.51, 5.62 | 5.40 | r12 `582147091732a06d` x12; r16 `a977961a58122047` | **inconclusive, not landed**: -2.0% (3/4) / -3.8% (4/4). Patch at `wip/noise-20c-vein-memo-hit-path.patch` |

**Why 20c can't reach the bar** (temporary counters, radius 8, 1 thread):
- **Reads:** `vein_toggle` is read ~11k times per chunk and misses its memo ~98 times; each miss evaluates a whole cell, and ~113 of its 128 lanes get read. `vein_ridged` is ~3.1k reads and 47 cell evaluations per chunk, ~66 of 128 lanes read.
- **The ~4% splits into:**
  - corner-grid builds for the three vein grids: ~2%;
  - ~4.1M memo hits at ~7 ns: ~1.3%;
  - cell evaluations: ~1.1%.
- **Implication:** wasted lanes are not the cost, and a batch API would only remove the same ~1.3% hit overhead. The vein grids' share belongs to 20a's corner-grid work.
- **Bundling:** 20c's hit path is a bundling candidate together with the 2D-cache allocation (1.8%), both exact. Note: `ab.sh` prints no `=== HOLD` header, so `ab-analyze.py` needs one added by hand to a single-hold file.
| 20b (n20b) | Per-block `run_cell`, two parts. (1) `fill_cell_interpolated` gains `lerp_cell_fixed::<CW>` for cell widths 4 and 8: fixed arrays and `chunks_exact_mut`, no bounds checks, the same lerps in the same order; other widths keep the general loop. (2) `run_cell`'s register file and `range_choice` scratch come from a new `Scratch::overwritten` and are not zeroed per cell. That is valid because every op writes all its `dst` lanes before any later op reads them, and `range_choice` always runs a branch. Tests: noise 42 (2 new, including `fixed_width_fill_is_bit_identical` over 2000 random corner sets with special values, and whole-cell against per-block on 8-wide cells in an order that reuses pooled buffers), chunkgen 192 + 1 + 1 + 1 | 4 rounds, 1 thread, radius 12, against shipb1/shipb2 | gen_cpu_s 5.36, 5.20 | 5.04 | r12 x12; r16 `a977961a58122047` | **inconclusive, not landed**: -5.9% (3/4) / -3.0% (3/4); lower `cpu_s` in 7 of 8 base comparisons. Patch at `wip/noise-20b-cell-lerp-and-regs.patch` |

**NaN note from 20b:** the random test found one case where the old and new code can differ: a corner NaN with a payload meets a NaN made by `inf - inf` inside the same `fadd`. Which NaN wins depends on how LLVM orders a commutative `fadd`'s operands, which Rust leaves unspecified, so the old code doesn't pin it either. Real densities never feed infinities into the lerp. The test accepts "both NaN" only for that two-source case and demands `to_bits` everywhere else.

**Next:** 20b and 20c are both exact, in different parts of the per-block path, and each is about 3-4% in a consistent direction. Bundle them (plus the 2D-cache allocation from the pool) and hold the bundle against the same bases.
| 20a (n20a) | Corner-grid Perlin leaf math, vectorized across lanes. New `ImprovedNoise::noise_block_reusing` (up to 64 lanes) in three phases: (1) coordinates as straight-line SoA f64 code (scale, wrap, offset, floor, fractions, y-smear, the three smoothsteps), with the smear mode fixed by const generics; (2) hashes through the same `CellHashes` memo, in lane order; (3) `lerp_corners_faded`, the same 8 `grad_dot`s and 7 lerps. `PerlinNoise::get_values` runs it in 64-lane blocks. `BlendedNoise::compute_chunk` runs it for the main stack, and for each limit stack over its active lanes (the same `!(factor >= 1)` / `!(factor <= 0)` tests), each sum written back to its lane. Disassembly shows `vmulpd/vaddpd/vsubpd/vroundpd` with no FMA. **Reviewed by me against the scalar path:** the same per-lane expressions, `u = smoothstep(dy)` on the un-smeared dy, `y_max` as the same `y * pow` product. Tests: noise 42 (2 new `to_bits` tests: 48 seeds x 40 blocks, every block length, all smear modes, coordinates up to `f64::MAX` and the wrap half-period, signed zeros; saturating selectors). On perf/noise: chunkgen 190 + 1 + 1 + 1 | 4 rounds, radius 12, rotating, against shipb1/shipb2 (ef9a35f), 1 thread and 8 threads | 1 t gen_cpu_s 5.46, 5.42; 8 t 6.87, 7.82 | 1 t **5.17**; 8 t 7.47 | r12 `582147091732a06d` x24; r16 `a977961a58122047` | **landed (07a6727)**: 1 thread -5.3% / -4.5%, **lower than both bases in 4/4 rounds** (per round n20a 5.10/5.08/5.33/5.18 against shipb1 5.52/5.35/5.59/5.39 and shipb2 5.34/5.68/5.42/5.23). 8 threads (regression check): +8.8% (2/4) against shipb1, -4.4% (3/4) against shipb2, cps +5.7% / +10.4%; shipb2 alone ranged 6.23-9.09, so no regression is visible above the 8-thread spread |
