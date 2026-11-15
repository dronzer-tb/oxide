# Log: datapath track

Bench: `scripts/bench-vps.sh datapath 16 8`. Digest gate at radius 16: `a977961a58122047`.
Newest entry last. Every row in `bench-datapath.tsv` has an entry here.

## 2026-11-15 session 1

### Baseline

`BENCH chunks=1089 threads=8 secs=7.880 cps=138.2 ms_per_chunk_core=57.89 digest=a977961a58122047`

Lower than the 160.5 in PLAN.md on the same code. The bench box had another user's Java
server at ~40% of a core during the run, and another track was building; treat run-to-run
noise as larger than 8% until that changes.

### Profile (step 1 of the track)

`perf` is not usable by the bench user (`perf_event_paranoid=4`, no sudo). The harness has a
SIGPROF sampler behind `--features profile`; this works and is what to use:

    cd ~/oxide-perf/datapath && flock ~/oxide-perf/.lock sh -c '
      ~/.cargo/bin/cargo build --release -p oxide-harness --features profile &&
      ./target/release/oxide-harness --datapack reference --seed 12345 --radius 8 \
        --time-only --threads 1 --profile-out prof.folded'

1 thread, radius 8, 50.4 ms per chunk. Shares below are of generation time only (the digest
pass, which is outside the timed part, is 30% of the raw samples and is excluded).

| Share | Where | Owner |
|---|---|---|
| 42% | `sample_in_chunk` called from the fill loop (corner grids 18%, per-block interpreter `run`+`clamp` 26%) | noise, but the call count is set by `fill.rs` |
| 16% | surface rules: `apply`, `eval_rule`, `eval_condition`, palette `set` | datapath |
| 10% | aquifer `compute_substance` (4% of it is `preliminary_surface_level` noise) | datapath |
| ~8% | fill loop itself: `cell_bounds` per cell row, palette writes | datapath |
| 6.5% | ore veins (mostly `vein_toggle` sampling, plus a `BlockState` clone per block) | datapath |
| 4-5% | carvers | datapath |
| 3% | `compute_heightmaps` (string compares per block, run twice) | datapath |
| 3% | `fill_biomes` + `Node::search` | datapath |

The harness does not show the 17% libc share the live-server profile had: most of that was
the JVM side and the decorator. Allocation here is a few percent, spread through the rows
above. The largest single lever inside this track's files is how often the fill loop asks for
a density sample, not `BlockState` churn.

Tests cannot run on the agent host (no `cc`), so they run on the bench box in the track's
own directory, under the same lock: rsync as `bench-vps.sh` does, then
`cargo test -p oxide-chunkgen -p oxide-biome` there.

### E1: integer block ids in fill, aquifer, ore veins, heightmaps -- kept

- `fill.rs`: the per-cell "all air" bound is computed once per cell (768 a chunk) into a
  table, instead of once per row of blocks crossing a cell (~24k calls, each allocating three
  vectors inside `Program::cell_bounds`). The loop writes palette indices for the three
  states it can place; no `BlockState` is built per block.
- `aquifer.rs`: new `compute_kind` returns a `FluidKind` tag; `compute_substance` wraps it
  for the carver. Additive, nothing renamed.
- `ore_veins.rs`: "is this the default block" is a palette-index compare.
- `compute_heightmaps`: palette classified once per section; all-air sections skipped.
- `biome_grid.rs`: corners compared by reference, sections written by palette index.
- `carver.rs`: `get_block` returns a reference instead of a clone.

Before: `BENCH chunks=1089 threads=8 secs=7.880 cps=138.2 ms_per_chunk_core=57.89 digest=a977961a58122047`
After:  `BENCH chunks=1089 threads=8 secs=5.898 cps=184.6 ms_per_chunk_core=43.33 digest=a977961a58122047`
Repeat: `BENCH chunks=1089 threads=8 secs=6.386 cps=170.5 ms_per_chunk_core=46.91 digest=a977961a58122047`

Digest unchanged. Tests: chunkgen 189 passed, biome 5 passed.

### E2: compiled surface rule tree -- kept

`surface.rs`: the rule tree is compiled once per `SurfaceSystem` into a form with every
string resolved: noises to `&NormalNoise`, anchors to a y, randomizer names to their
factory, `block` results to an index. Per chunk, biomes are numbered and each `biome`
condition becomes a mask over those numbers. Column-only conditions (2D noise thresholds,
`steep`, the badlands band offset, the temperature noise) are computed at most once per
column. Results are interned into a section's palette on first write and written by index
after that; the palette classification is extended on a new entry rather than rebuilt per
write. All-air sections are stepped over. Clay bands are built on first badlands hit, not
per chunk. Formulas and evaluation order are unchanged.

What this removed per chunk: a `ResourceLocation` clone (two allocations) for every stone
block, a `BlockState` clone plus hash for every rewritten block (all of deepslate, ~16k),
and a `SectionClass` rebuild for each of those writes. Under 8 threads this was worth more
than its single-thread profile share, which points at allocator contention.

Before: `BENCH chunks=1089 threads=8 secs=6.386 cps=170.5 ms_per_chunk_core=46.91 digest=a977961a58122047`
After:  `BENCH chunks=1089 threads=8 secs=3.934 cps=276.8 ms_per_chunk_core=28.90 digest=a977961a58122047`
Repeat: `BENCH chunks=1089 threads=8 secs=3.954 cps=275.4 ms_per_chunk_core=29.05 digest=a977961a58122047`

Digest unchanged. Tests: chunkgen 189 passed.

### Profile after E2 (1 thread, radius 8, 26.9 ms per chunk, was 50.4)

Shares of generation time:

| Share | Where |
|---|---|
| 58% | `sample_in_chunk` from the fill loop: corner grids 27%, per-block `cache_once` subtree (the cave terms under the `min`) 25%, interpreter and `lerp3` the rest |
| 13% | aquifer: `compute_kind` 7%, `preliminary_surface_level` 6% |
| 9% | surface rules |
| 8% | ore veins |
| 5% | carvers |
| 4% | biomes |

The fill loop's density sampling is now most of the chunk. The noise track owns the cost
per sample; this track owns how many are asked for. Idea for the noise track, not acted on
here: a lower bound on the non-interpolated cave terms per cell would let the fill skip
per-block evaluation in solid cells as it already does in air cells.

### E3: aquifer nearest-centre selection and surface-level memo -- kept

`aquifer.rs`: the nearest-centre pick keeps three candidates with plain compares (it kept
four through `rotate_right`, 3% of samples, and never read the fourth). The
`preliminary_surface_level` memo is a dense quart grid around the chunk instead of a
SipHash map.

Before: `BENCH chunks=1089 threads=8 secs=3.954 cps=275.4 ms_per_chunk_core=29.05 digest=a977961a58122047`
After:  `BENCH chunks=1089 threads=8 secs=3.883 cps=280.5 ms_per_chunk_core=28.52 digest=a977961a58122047`
Repeat: `BENCH chunks=1089 threads=8 secs=3.857 cps=282.4 ms_per_chunk_core=28.33 digest=a977961a58122047`

About 2%, inside run-to-run noise but the same on both runs. Kept: it is less code and the
digest is unchanged. Tests: chunkgen 189 passed.

### E4: skip ore-vein sampling in cells that cannot hold a vein -- kept, gain unconfirmed

`ore_veins.rs`: `vein_state` returns nothing when `|vein_toggle| + edge_roundoff < 0.4` and
`edge_roundoff <= 0`, so a cell whose `vein_toggle` corner bounds sit strictly inside
+-0.4 is skipped whole (same `cell_bounds` the fill uses). Cells with no provable bound are
evaluated block by block as before.

Before: `BENCH chunks=1089 threads=8 secs=3.857 cps=282.4 ms_per_chunk_core=28.33 digest=a977961a58122047`
After:  `BENCH chunks=1089 threads=8 secs=3.738 cps=291.3 ms_per_chunk_core=27.46 digest=a977961a58122047`
Repeat: `BENCH chunks=1089 threads=8 secs=5.324 cps=204.6 ms_per_chunk_core=39.11 digest=a977961a58122047`
Repeat: `BENCH chunks=1089 threads=8 secs=4.983 cps=218.5 ms_per_chunk_core=36.61 digest=a977961a58122047`

The two slow repeats ran while other `rustc` jobs were on the box (seen in `ps` just before
each; there is a `noise-prof` directory building outside the bench lock). They say nothing
about this change either way. Kept because it strictly removes samples and the digest is
unchanged; the size of the gain is not established. Tests: chunkgen 189 passed.

Measurement note: with foreign load on the box a single wall-clock run can be 30% low.
From here each experiment also records the harness's user+sys CPU seconds, which waiting
for a core does not inflate.

### E5: `PalettedContainer` without per-section index arrays or maps -- kept, gain small

`oxide-core/src/storage/paletted.rs`: a container that still holds only its default value
keeps no index array; `indices()` hands out a shared static zero slice. Sections that stay
all air (most of the 24 in a chunk) no longer allocate and zero 16 KiB. Palettes up to 8
entries are searched linearly, and the `AHashMap` reverse lookup is only built past that.
Public API and behaviour unchanged; two new tests cover the empty-index path and the switch
from linear search to the map. Tests: core 20, anvil 6, chunkgen 189 passed.

Before (E4): `cps=291.3`, `204.6`, `218.5` (noisy box)
After:  `BENCH chunks=1089 threads=8 secs=3.521 cps=309.3 ms_per_chunk_core=25.86 digest=a977961a58122047`
Repeat: `BENCH chunks=1089 threads=8 secs=3.921 cps=277.7 ms_per_chunk_core=28.81 digest=a977961a58122047`
Repeat: `BENCH chunks=1089 threads=8 secs=4.220 cps=258.1 ms_per_chunk_core=31.00 digest=a977961a58122047`

CPU seconds for the whole harness process, radius 16, 8 threads, 2 runs each (this includes
the ~25 s single-threaded digest pass, so it dilutes the generation difference):
before user 63.2 / 57.5 s, sys 2.4 / 1.7 s; after user 54.8 / 58.7 s, sys 1.1 / 1.2 s.
Sys time halved (less page zeroing). The generation difference is a few percent at most and
not separable from the noise. Kept: strictly less memory and work, digest unchanged.

### Next three (written 2026-11-15 after E5, on request)

The log had no handover list yet; this is what is pending, in order:

1. Auditor ask: put a margin on the strict-bound skips (fill's all-air cells, E4's veinless
   cells), so each skip is provable under float rounding rather than relying on the digest.
2. Merge `origin/perf/noise` (whole-cell `final_density` via `sample_cell`, cell-bounds
   fixes) and bench the merge as the first combined number, with a repeat.
3. Auditor ask: confirm or refute E4's gain on a quiet box, by A/B against E4 reverted.

Auditor's ruling on E5: kept as a memory win only; its speed gain is not established
(309/278/258).

### E6 (next-three #1): margin on the strict-bound skips -- kept (correctness)

`fill.rs` now skips a cell as all air only when its `final_density` upper bound is
`<= -1e-9` (was `<= 0`); `ore_veins.rs` skips a cell only when `vein_toggle` is inside
`+-(0.4 - 1e-9)` (was `+-0.4`). The constant is `fill::CELL_BOUND_MARGIN`, documented there:
the bounds come from corner min/max through interval arithmetic, the per-block value from a
floating-point blend of the corners, and the rounding of the latter can land a few ulp
outside the former. With the margin, each skip holds whatever the rounding does, instead of
relying on no value ever landing on the edge. It can only make fewer cells skip.

`BENCH chunks=1089 threads=8 secs=3.576 cps=304.6 ms_per_chunk_core=26.27 digest=a977961a58122047`
`BENCH chunks=1089 threads=8 secs=4.214 cps=258.4 ms_per_chunk_core=30.95 digest=a977961a58122047`

Same digest and the same spread as E5's runs: no measurable cost. Tests: chunkgen 189 passed.

### E7 (next-three #2): merge `origin/perf/noise` -- first combined number

Merged `origin/perf/noise` at 945cbde (whole-cell `final_density` behind `sample_in_chunk`,
cache slots shared between equal nodes, `cell_bounds` on the stack). Clean merge, no
conflicts: the noise track touched only `crates/oxide-noise`. The fill already walks y
outermost within a section, which is the order the noise track's per-slot layer memo
expects, so it picks up the cell batching with no change here.

Tests (on the box, shared lock): noise 30, core 20+57, chunkgen 189 passed.

First combined: `BENCH chunks=1089 threads=8 secs=2.771 cps=392.9 ms_per_chunk_core=20.36 digest=a977961a58122047`
Repeat:         `BENCH chunks=1089 threads=8 secs=3.139 cps=346.9 ms_per_chunk_core=23.06 digest=a977961a58122047`

The repeat ran while the box was loaded (load 20 from other tracks' test builds just
before). Auditor's scratch merge of the two heads: 401 and 421 cps, same digest. Datapath
alone was ~280 to 300 on a busy box, noise alone 163.8; started at 138.2.

### Process notes

- OmniRoute is not used any more (user instruction). The one OmniRoute attempt this session
  failed before doing anything: `auto/kimi` has no tool-calling target, and
  `gemini-3-flash-preview` returned 429.
- Parallel experiments run as Opus sub-agents in `/tmp/datapath-<n>` worktrees, benching
  under `datapath-<n>`; the master reviews diffs and commits winners here.

### E8: fill reads `final_density` a whole cell at a time via `sample_cell` -- kept, gain not established

Sub-agent experiment in `/tmp/datapath-1`, bench name `datapath-1` (rows in
`bench-datapath-1.tsv`). `fill.rs` gets densities through `NoiseRouterEvaluator::sample_cell`
into a one-layer-of-cells buffer. A cell is evaluated the first time one of its blocks needs a
density, so skipped cells are never evaluated. Visit order, write order and every
`compute_kind` call are as before. A cell `sample_cell` declines falls back to
`sample_in_chunk`.

Base (E7): `cps=392.9`, `346.9`; auditor's merge: 401, 421.
Run 1: `BENCH chunks=1089 threads=8 secs=2.434 cps=447.4 ms_per_chunk_core=17.88 digest=a977961a58122047`
Run 2: `BENCH chunks=1089 threads=8 secs=2.610 cps=417.3 ms_per_chunk_core=19.17 digest=a977961a58122047`

About 5% over the auditor's base numbers, within noise. Kept: it removes the per-block
dispatch and memo lookup with the digest unchanged. Run 1 ran before the tests (it was
launched before the tests-first rule). Tests: chunkgen 189 passed.
Noted by the sub-agent: `Program::run_cell` allocates its register vector once per cell
(noise crate, not this track's).

### E4 A/B (next-three #3): refuted -- no measurable gain

At the merged head (0e5afee), one exclusive lock hold, alternating the head's binary with a
build whose only change is E4's skip loop never running (`0..(cells_y * 0)`):

| | run 1 | run 2 | mean |
|---|---|---|---|
| E4 on  | 386.1 | 459.2 | 422.7 |
| E4 off | 404.0 | 442.9 | 423.5 |

All four runs `digest=a977961a58122047`. No difference. After the noise merge, `vein_toggle`
sampling is cheap enough that skipping it in veinless cells buys nothing measurable. E4's
earlier "291" was noise. The skip is left in, since it is correct (with the margin) and the
ore-vein palette work in flight builds on it. Removing it is a fair simplification if
anyone wants the code smaller.

Also from this pair: the run-to-run spread inside one exclusive hold on a quiet box is still
~15% (386 vs 459 for the same binary). Single-run comparisons cannot resolve gains under
~10%.

### E9: ore veins write by palette index -- kept, gain not established

Worktree `/tmp/datapath-4`, bench name `datapath-4`. Written by the master: the OmniRoute
worker it was meant for failed to start. `vein_state` returns the static block name, and the
six vein states are built once per chunk. Each section interns a state on its first write
(the same place the old per-block `set` added it, so palette order is unchanged), then
writes by index. No `BlockState` is built or hashed per vein block any more.

Base (merged head): 386 to 459 in the E4 A/B.
Run 1: `BENCH chunks=1089 threads=8 secs=2.544 cps=428.0 ms_per_chunk_core=18.69 digest=a977961a58122047`
Run 2: `BENCH chunks=1089 threads=8 secs=5.261 cps=207.0 ms_per_chunk_core=38.65 digest=a977961a58122047` -- invalid: the box's 5- and 15-minute load averages were 21 and 25 while this exclusive run timed, so something ran outside the lock
Run 3: `BENCH chunks=1089 threads=8 secs=2.396 cps=454.5 ms_per_chunk_core=17.60 digest=a977961a58122047`

Inside the base's range. Kept as strictly less work with the digest unchanged. Run 1 came
before the tests (launched before the tests-first rule). Tests: chunkgen 189 passed.

### E10: carvers on palette indices -- kept, gain not established

Sub-agent experiment in `/tmp/datapath-3`, bench name `datapath-3`. `apply_carvers` wraps the
chunk in a private `Target` with per-section caches; its public signature is unchanged.
`can_replace` is a table read on the raw palette index. The table is classified from the
palette once and extended when the palette grows. Carving calls `compute_kind`, not
`compute_substance`. Each of five result states (carver `lava[level=0]`, plain air, and the
aquifer's air, water and lava) is interned at its first write into a section, the same point
the old per-block `set` added it, and written by index after that. Geometry and float code
untouched.

Base (merged head): 386 to 459 in the E4 A/B.
Run 1: `BENCH chunks=1089 threads=8 secs=2.620 cps=415.7 ms_per_chunk_core=19.25 digest=a977961a58122047`
Run 2: `BENCH chunks=1089 threads=8 secs=2.553 cps=426.6 ms_per_chunk_core=18.75 digest=a977961a58122047`

Inside the base's range (carvers were ~5% of the profile). Kept: strictly less work, digest
unchanged. Tests: chunkgen 189 passed.

### E11: surface rules as a flat program; block kinds read once per chunk -- kept

Sub-agent experiment in `/tmp/datapath-2`, bench name `datapath-2`. Profile first (1 thread,
radius 8, merged head): `SurfaceSystem::apply` was 13.5% of generation. Inside it: the
recursive rule walk (`eval_rule`, its `find_map` closure, `eval_condition`) ~62%, the
per-block palette read and classify ~27%, per-column noise ~10%.

- The compiled rule tree is flattened into a list of steps. A condition is a test that jumps
  past its subtree on failure, which is the old walk's "return None to the enclosing
  sequence". Leading `not`s fold into a negate flag. Conditions, memos and random draws run
  in the same order.
- Block kinds for the whole chunk are read into one flat array before the scan. A column
  reads only its current position (before writing it) and positions below it (not written
  yet), so the snapshot gives the same answers. The per-write reclassification is gone.
- `surface_secondary` is sampled only when a `stone_depth` condition with a nonzero
  secondary range asks for it. It depends only on (x, z).

Reviewed by the master: the jump semantics match the old recursion, including a condition
nested directly in a condition.

Head without it (a baseline run the sub-agent had queued before the no-baseline rule): `BENCH chunks=1089 threads=8 secs=2.626 cps=414.7 ms_per_chunk_core=19.29 digest=a977961a58122047`
Run 1: `BENCH chunks=1089 threads=8 secs=2.299 cps=473.7 ms_per_chunk_core=16.89 digest=a977961a58122047`
Run 2: `BENCH chunks=1089 threads=8 secs=2.343 cps=464.8 ms_per_chunk_core=17.21 digest=a977961a58122047`

Both runs above the head's best (459) in the E4 A/B: ~12% less core time per chunk. Tests:
chunkgen 189 passed.

### Combined head after E8-E11 (1da3c44): first runs invalid

Tests on the box: chunkgen 189, core 57 and the rest passed.
`BENCH chunks=1089 threads=8 secs=4.815 cps=226.2 ms_per_chunk_core=35.37 digest=a977961a58122047`
`BENCH chunks=1089 threads=8 secs=4.119 cps=264.4 ms_per_chunk_core=30.26 digest=a977961a58122047`

Invalid as speed numbers: right after them `ps` showed `oxide-pregen` (pipeline track) at
144% CPU, outside the bench lock. Only `oxide-harness` timed runs take the lock
exclusively, so a pregen run competes with whatever is being timed. The earlier E9 207 run
was most likely the same. Flagged for the auditor: pregen runs on the box need
`flock -x ~/oxide-perf/.lock` if they are timed, or `flock -s` with `nice` if not. Remeasuring with an
interleaved A/B (combined vs E11-only) in one exclusive hold.

### Combined head (1da3c44): interleaved A/B -- new reference ~479 cps

One exclusive hold, three alternating pairs: the combined head against the E11-only build
(the `datapath-2` binary: the merged head plus E11, without E8-E10). Load average falling
from 5.3 to 2.4 during the hold.

| | pair 1 | pair 2 | pair 3 | mean |
|---|---|---|---|---|
| combined (E8-E11) | 492.3 | 475.3 | 470.3 | 479.3 |
| E11 only | 450.0 | 482.0 | 446.5 | 459.5 |

All six `digest=a977961a58122047`. E8-E10 together add about 4% over E11, ahead in two of
three pairs: small, but it agrees with keeping them. Reference for the next round:
**~479 cps, ~16.7 ms core per chunk** (session start: 138.2 cps, 57.9 ms). M1 (400 cps) is
passed; M2 is 1,000.

### Profile of the combined head (6b8562f; 1 thread, radius 8, 15.9 ms per chunk)

Shares of generation time:

| Share | Where | Owner |
|---|---|---|
| 36.0% | corner-grid build (Perlin) | noise |
| 18.8% | aquifer: `compute_kind` 10.3%, `preliminary_surface_level` 8.5% | datapath |
| 13.7% | surface pass | datapath |
| 8.0% | carvers (2.3% of it in the aquifer) | datapath |
| 5.9% | ore veins | datapath |
| 5.6% | fill loop self | datapath |
| 6.2% | biomes: search 4.1%, climate sampling 2.1% | datapath |
| 4.7% | `run_cell` (per-block density blend) | noise |

Round 2 of sub-agents, all based on 6b8562f, with disjoint file boundaries:
- `datapath-5`: `aquifer.rs`, covering `compute_kind` and `preliminary_surface_level`. Warned that
  a cross-chunk cache is exact only if the value is shown not to depend on which chunk's
  caches sampled it.
- `datapath-6`: `surface.rs`, round 2 (skip positions no rule can match; dispatch cost).
- `datapath-7`: `biome_grid.rs`, `fill.rs` and `oxide-biome/src/search.rs` (search layout; fill loop self
  time). Tie rule and visit order are load-bearing; no `lastResult` seeding.

Process fix: my VPS helper ran profile builds under `flock -s` at nice 0. Every locked
command on the box now runs as `flock -s ... nice -n 10 sh -c '...'`, the profiled run
included.

### E12: flat biome search, corner reuse, fill-loop index tables -- kept, gain not established

Sub-agent experiment in `/tmp/datapath-7`, bench name `datapath-7`, based on 6b8562f.
- `oxide-biome/src/search.rs`: the R-tree built by the same `build` is laid out as one
  flat array of plain min/max arrays and child ranges. The search walks children in stored
  order with the same `<=`-keeps-the-incumbent rule and starts from nothing. It keeps one
  running (leaf, distance) pair instead of returning through the recursion; a subtree can
  only replace the incumbent with a strictly closer leaf, which is what the parent accepted
  before. It returns once the best distance is 0, when every remaining child would be pruned.
  New test `flat_search_returns_the_recursive_searchs_leaf`: about 10k climate points on the
  overworld preset, the identical leaf as a kept copy of the old recursive search.
- `biome_grid.rs`: a boundary section reuses its 8 corner lookups instead of taking them again
  at the same positions (pure functions of position).
- `fill.rs`: per-block `div_euclid` for cell x and z replaced by 16-entry tables built once
  with the same formula; the air-cell lookup resolved per (y, z) row. Visit order, write
  order and every density and `compute_kind` call unchanged.

Reviewed by the master: the in-place best and the early return are equivalent to the old
recursion, ties included.

Base: ~479 (interleaved A/B, 6b8562f).
Run 1: `BENCH chunks=1089 threads=8 secs=2.146 cps=507.4 ms_per_chunk_core=15.77 digest=a977961a58122047`
Run 2: `BENCH chunks=1089 threads=8 secs=2.168 cps=502.2 ms_per_chunk_core=15.93 digest=a977961a58122047`

About 5%, inside noise. Kept: less work, digest unchanged. Tests: biome 6 passed (one new),
chunkgen 189 passed.

### E13: aquifer neighbour-chunk caches and per-anchor candidate centres -- kept

Sub-agent experiment in `/tmp/datapath-5`, bench name `datapath-5`, based on 6b8562f.

Why `preliminary_surface_level` cost 8.5%: in the overworld pack `PreliminarySurfaceLevel`
is a `find_top_surface` scan, in 8-block steps, over a density built from
`cache_2d(offset)` and `cache_2d(factor)`. Those are splines over continents, erosion and
ridges. Nearly every aquifer sample is outside the chunk, because centres reach up to three
chunks away. `ChunkCaches` only holds the chunk's 17x17 columns, so every step of every scan
re-ran the whole spline and noise stack.

- Columns in other chunks of the aquifer's memo window are sampled through a private,
  lazily built `ChunkCaches` for that chunk, owned by this chunk's aquifer and dropped with
  it. Nothing is shared across chunks or threads.
- Exactness (checked by the sub-agent against the overworld JSON, reviewed by the master):
  - the program has no `interpolated` node, the only cache kind that clamps to a chunk's
    grid;
  - every `cache_2d` / `flat_cache` argument depends on (x, z) alone, bit for bit:
    `shifted_noise` with `y_scale` 0 gives `y*0.0 + 0.0 = +0.0` for every y, `shift_a` and
    `shift_b` ignore y, and `blend_*` are constants;
  - `cache_once` is keyed on the exact position.
  So a cached value equals the uncached one. Vanilla's `Cache2D` also ignores y.
  **Datapack assumption:** a pack that put an `interpolated` node or a y-dependent
  `cache_2d` argument in this slot would get different values from the old path. This is a
  correctness boundary for custom packs, not a digest risk for the vanilla one.
- `compute_kind`: the 12 candidate centres around a horizontal anchor are kept, tagged with
  the anchor's y, and refilled only when the anchor changes (at most 4 per y layer in the
  fill's order). `location()` draws are positional, so fill order cannot matter. The
  per-block work left is the integer distances and the unchanged `>=` top-3 cascade.
  Anchors whose 12 cells are not all inside the grid use the original per-block loop.
  Everything after the top-3 search is moved verbatim.

Base: ~479 (interleaved A/B, 6b8562f).
Run 1: `BENCH chunks=1089 threads=8 secs=2.012 cps=541.2 ms_per_chunk_core=14.78 digest=a977961a58122047`
Run 2: `BENCH chunks=1089 threads=8 secs=1.907 cps=571.0 ms_per_chunk_core=14.01 digest=a977961a58122047`

Both runs above the base's best (492.3) in the A/B: about 15% less core time per chunk.
Tests: chunkgen 189 passed.

### E14 (in flight): surface round 2, folded rule programs

Sub-agent experiment in `/tmp/datapath-6`, based on 6b8562f. The profile could not split the
surface pass further, because the scan loop is inlined into one frame. Counters (temporary
build) show per chunk: ~25.9k positions offered, 85% of them below the preliminary surface,
6.3 ops per position on average, ~2.6k `vertical_gradient` draws.

Change: each position runs a copy of the program folded for facts known cheaply there: the
y band (edges at every `vertical_gradient` bound and every depth-free `y_above`), above the
preliminary surface or not, stone depth 1 or at least 2, water above or not. Folded copies
are built lazily per fact key. Settled tests are dropped, and a level that always places a
block ends its siblings. `run_bottom` is walked only when a `ceiling` `stone_depth` is
evaluated. Exact because every condition is a pure function of the position, its column and
the chunk. Draws are positional: `WorldPositionalFactory::at` seeds a fresh RNG from the
position, so skipping one affects no other.

Sub-agent's runs (base 6b8562f): `cps=414.8` (box load 10 right after) and `cps=504.3`,
both `digest=a977961a58122047`. Inconclusive. It is 263 lines of folding logic, so it lands
only if an interleaved A/B against the current head (85f6075, with E12 and E13) shows a gain.
That A/B is running: three alternating pairs in one exclusive hold, after tests on both builds.

### E14 A/B: rejected. Head 85f6075 is the new reference, ~584 cps

Tests passed on both builds (biome 6, chunkgen 189). Then one exclusive hold, three
alternating pairs, load average falling from ~3.7 to 2.1:

| | pair 1 | pair 2 | pair 3 | mean |
|---|---|---|---|---|
| head 85f6075 (E8-E13) | 592.2 | 599.4 | 559.2 | 583.6 |
| head + E14 | 565.5 | 571.8 | 588.3 | 575.2 |

All six `digest=a977961a58122047`. E14 is slower in two of the three pairs and slightly slower on average. Rejected: 263
lines of folding buy nothing, most likely because building folded programs per chunk costs
what it saves, with `SurfaceSystem` rebuilt per chunk in `lib.rs`. The diff is kept at
`docs/perf/rejected-E14-surface-fold.diff` so it is not retried blind. Retry it only if
`SurfaceSystem` becomes per-thread rather than per-chunk.

**Reference: ~584 cps, ~13.7 ms core per chunk** (session start: 138.2 cps, 57.9 ms; 4.2x).

PLAN.md updated from `feat/native-decoration`: decoration (`decorator/`, `decorate.rs`,
`region.rs`) now belongs to the pipeline track. No datapath change has touched those
files. The PR into `feat/native-decoration` that rule 4 asks for is not open: this agent host
has no `gh`. Branch `perf/datapath` is ready for one; someone with GitHub access can open it,
or `gh` can be installed here.
