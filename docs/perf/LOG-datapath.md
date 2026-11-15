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

### Round 3 (in flight), based on 689eb30 (~584 cps)

Profile of the head, generation only: corner-grid build 39.9% (noise), surface 15.3%, aquifer
13.6% (`compute_kind` 8.3%, surface level 5.3%), carvers 8.6%, ore veins 6.3%, `run_cell` 5.6%
(noise), biomes 4.9%, fill loop self 4.6%.

Sub-agents:
- `datapath-8`: surface round 3. Make the pass profileable with `#[inline(never)]` first,
  then cut the dominant part. Sharing the compiled program across chunks is allowed if the
  lifetimes permit. E14's per-chunk folding is not to be repeated as is.
- `datapath-9`: aquifer round 2 (`find_top_surface` scan cost, `compute_fluid`'s uncached
  `router.sample` calls, the per-block remainder).
- `datapath-10`: carvers (sequential `LegacyRandom`, so only skips that touch no draw) and
  ore veins (per-cell `vein_ridged` bound, cost of the random draw).

All three hit the 5-hour usage limit before writing any code, and were resumed after the
reset with the same rules.

### E15: aquifer round 2, branch-free top-3 -- rejected as neutral

Sub-agent experiment in `/tmp/datapath-9`. Where the aquifer's 13.4% goes, from the base profile:
- `compute_kind` own work is 6.0%: the 12 integer distances and the top-3 search.
- `preliminary_surface_level` is 5.7%, all inside `sample_in_chunk`, split three ways:
  - ~1.6% is allocation: each neighbour `ChunkCaches` allocates a 289-entry
    `vec![None; COLUMNS]` per `cache_2d` slot it touches (`cache.rs`, `column_cached`).
  - ~1.7% is noise sampling.
  - ~2.4% is the interpreter and cache-lookup overhead of the 8-block scan steps.
- `compute_fluid` and `status()` are ~1.1%.

`Erosion` and `Depth` in `compute_fluid` cannot move onto the caches. `router.sample` evaluates
`flat_cache` at the exact (x, z), while `ChunkCaches` snaps it to the quart corner, so the
values would differ. The E13 argument does not carry over.

Change tried: centres stored one coordinate per array, and the `>=` cascade replaced by a
branch-free `min`/`max` chain on keys `distance << 4 | (15 - i)`. Exact: the key's order is
the cascade's order (distance ascending, later candidate first on ties).
`BENCH chunks=1089 threads=8 secs=2.106 cps=517.1 ms_per_chunk_core=15.47 digest=a977961a58122047` (load ~5-6)
`BENCH chunks=1089 threads=8 secs=1.855 cps=587.2 ms_per_chunk_core=13.62 digest=a977961a58122047`
No gain over ~584. The branches were probably well predicted. Rejected; the diff is kept at
`docs/perf/rejected-E15-aquifer-branchless-top3.diff`.

Open, outside this track's files or rules:
- Noise track: `cache.rs` allocates 289 entries per `cache_2d` slot per `ChunkCaches`. A
  neighbour cache touches a handful of columns, so a sparse or lazily grown column store
  would remove ~1.6%.
- The rest of the surface-level cost is each neighbour column recomputed by every chunk whose
  aquifer reaches it, up to ~20 times. Removing that needs state shared across chunks: a
  per-thread memo keyed on router identity, with exactness argued as in E13. Not done: it
  crosses the "nothing shared across chunks" line, and the key needs care.

### E16 (pending A/B): surface round 3, fold by preliminary-surface fact and y bands

Sub-agent experiment in `/tmp/datapath-8`. The first profile that splits the surface pass
(temporary `#[inline(never)]`; 1 thread, radius 8):

| Frame | Inclusive | Self |
|---|---|---|
| `SurfaceSystem::apply` | 14.3% | 0.5% |
| per-column body | 13.8% | 3.0% (slot walk, cursor, biome lookup) |
| `eval_program` | 8.8% | 3.5% |
| `eval_condition` | 5.3% | 5.2% |
| `column_state` | 1.4% | |
| `SurfaceSystem::new` | 0.8% | `compile_rule` 0.6% |

The cost is the interpreter: ~163k condition tests per chunk at ~5-8 ns each. Draws, noise and
setup are small. Sharing the program across chunks would save at most 0.8%.

Change: `new` folds the flattened program once for "at or above the preliminary surface". It
then folds once for "below it", which leaves about 8 ops, and folds that small program once
per y band (about 5 bands, plus a 384-entry y-to-band table). The scan picks a program by
`y >= column.min_surface_level`, the same comparison the condition makes. A test is dropped only if its answer
is fixed across the program's positions and evaluating it has no side effect: preliminary
surface, depth-free `y_above`, and `vertical_gradient` only where it answers without
drawing. Below the surface, four tests per position become about one.
Unlike E14 (rejected), the folding is a few small passes per chunk, not lazy builds per
fact key.

`BENCH chunks=1089 threads=8 secs=2.054 cps=530.1 ms_per_chunk_core=15.09 digest=a977961a58122047`
`BENCH chunks=1089 threads=8 secs=1.796 cps=606.3 ms_per_chunk_core=13.19 digest=a977961a58122047`
Inconclusive. Waiting for an interleaved A/B against the head. Tests: chunkgen 189 passed.

### E16 kept, E17 rejected: three-way interleaved A/B

One exclusive hold, three rounds of head, E16 and E17 in turn; load average falling from 4.3 to 1.4:

| | round 1 | round 2 | round 3 | mean |
|---|---|---|---|---|
| head 689eb30 | 558.1 | 592.1 | 592.6 | 580.9 |
| E16 surface folding | 567.9 | 605.2 | 639.7 | 604.3 |
| E17 carvers + ore | 544.4 | 529.8 | 579.7 | 551.3 |

All nine `digest=a977961a58122047`. Caveat: the head binary on the box was the build with
`--features profile` (pprof linked in, profiler inactive without `--profile-out`).

**E16 kept**: ahead in all three rounds, +4.0%. Tests: chunkgen 189 passed.

**E17 rejected**: behind in all three rounds, -5.1%.
- Carvers: each cave tunnel's walk was recorded once per thread and replayed per chunk, with
  a padded bounding-box skip.
- Ore veins: `vein_toggle` read through whole-cell `sample_cell`; the gap between the bands
  skipped; a per-type edge-roundoff bound on the veinless-cell test.
- The exactness arguments hold (a tunnel walk reads only its own RNG and inputs; all ore skips
  drop positions where `vein_state` returns nothing). The cost is most likely the thread-local
  `HashMap` lookup and `Rc` clone per tunnel per chunk, against a walk that was cheap.
- The two halves were not measured apart. If anyone retries, try the ore half alone first.
- Diff kept at `docs/perf/rejected-E17-tunnel-replay-and-vein-cells.diff`.

Also from the sub-agent: a per-cell `vein_ridged >= 0` skip cannot be bounded today.
`vein_ridged` is `abs`/`max` over interpolated nodes, and `Program::cell_bounds` returns `None`
for `abs`. Interval rules for `abs` would be a small change in the noise crate.

### Current state and next three (updated after round 3)

Head: ae011ba, **~604 cps** (interleaved A/B against 689eb30: 580.9 -> 604.3). Session start: 138.2.
Gate M1 (400) passed; M2 is 1,000. All kept changes leave `digest=a977961a58122047`.

Biggest costs left at 689eb30 (generation only): corner-grid build 39.9% (noise track),
surface ~13% after E16, aquifer 13.6%, carvers 8.6%, ore veins 6.3%, biomes 4.9%, fill loop
4.6%.

Next three:
1. Aquifer: memoise neighbour `preliminary_surface_level` columns across chunks. Each column
   is recomputed by up to ~20 chunks, ~5% of generation. Exact by the E13 argument (the value
   is a pure function of quart (x, z) for this pack), but the memo needs a key that cannot
   alias two different routers. E17 showed thread-local hash lookups can cost more than they
   save, so only for work as expensive as a `find_top_surface` scan.
2. Surface: the above-surface program and the per-column body (3% self: slot walk, cursor,
   biome lookup), now that below-surface positions are cheap.
3. Ask the noise track (their files): an interval rule for `abs` in `Program::cell_bounds`,
   which would allow a per-cell `vein_ridged` skip, and sparse column storage in `cache.rs`
   (~1.6% in neighbour caches).

PR into `feat/native-decoration` is not open: no `gh` on this host.

### Additive change in `oxide-noise` (noise track's crate): `NoiseRouterEvaluator::id()`

The round-4 aquifer sub-agent (`datapath-11`) found no safe cross-chunk memo key within
chunkgen, and stopped without changing anything. Chunkgen only sees `&NoiseRouterEvaluator`:
- addresses are reused after a drop (the FFI frees and builds worlds);
- a seed does not identify a datapack;
- the router's contents cannot be fingerprinted cheaply through its public API.

So, as PLAN.md allows ("the smallest additive change, say so in your log"): a private
`id: u64` field set in `new` from a process-wide `AtomicU64`, and `pub fn id(&self) -> u64`.
Nothing renamed or reshaped. The router is not `Clone`, so ids are never shared. One field and one
accessor; if the noise track changes `new`, the merge conflict is two lines.

### E18: per-thread cross-chunk memo of neighbour surface levels -- kept

Sub-agent experiment in `/tmp/datapath-11`, on 91bec92, which adds `NoiseRouterEvaluator::id()`.
- Each thread keeps one `SurfaceMemo`: the router id it was filled for, plus up to 16 dense
  pages of 64x64 quart columns, one `i32` each. That is at most 256 KiB per thread, with LRU page reuse.
- An `Aquifer` takes the memo out of the thread-local in `new` and calls `bind(router.id())`,
  which clears every page if the id differs. `Drop` puts the memo back. An aquifer built while another holds the
  memo starts with an empty one.
- Lookup order: the per-aquifer window, then the thread page, then the real computation, which
  writes both.
- It cannot alias, because ids come from a process-wide counter that only grows and the router
  is not `Clone`.
- Values are exact by the E13 argument: the same function, the same router, the same quart
  column, so the same bits whichever caches computed it. Only an `i32` is stored.
- New test `surface_memo_never_crosses_routers`: four routers on one thread, all with seed 7
  but different constant surface levels, three aquifers each. Every column must return its
  own router's value.

Tests: chunkgen 190 passed (one new), noise 30 passed; `id()` compiled here for the first time.
Sub-agent runs: `cps=610.9`, `646.1` (box load ~5).

Interleaved A/B, one exclusive hold, head rebuilt plain (without the profile feature) at 91bec92 versus 91bec92 plus E18:

| | pair 1 | pair 2 | pair 3 | mean |
|---|---|---|---|---|
| head 91bec92 | 574.8 | 593.1 | 581.9 | 583.3 |
| + E18 | 620.7 | 639.6 | 555.9 | 605.4 |

All six `digest=a977961a58122047`. +3.8%, ahead in two of three pairs. Kept: it matches the ~4%
of generation E15 attributed to recomputing neighbour columns, and it is exact and tested.
The datapack caveat from E13 still applies, now across chunks.

### E19: surface round 4 (per-chunk fold, banded below-surface fast path) -- rejected

Sub-agent experiment in `/tmp/datapath-12`, on e5aa047. Profile at e5aa047 (inline(never)
split): surface 11.9% of generation. The per-column scan was 3.8% self, `eval_program` 2.0%,
`eval_condition` 3.3%, `column_state` 1.2%.

Change:
- Fold the above-surface program per chunk, twice: for `stone_depth_above` 1 and >= 2, by
  biome sets on which all the chunk's biomes agree, and by the chunk's lowest preliminary
  surface.
- Fold each below-surface band once per chunk.
- Below the surface, drop the depth and water walk; this was checked at `new` to be unread
  there.
- Handle each band by its shape: an empty band is skipped, a single block is a direct write,
  anything else is evaluated.

Exactness reviewed: the same positions are written in the same per-column order, and only
pure tests are dropped.

Sub-agent's runs (on e5aa047, not interleaved): `cps=662.1`, `647.3`.
Rebased onto a6c1fcd (conflict-free: E18 is in `aquifer.rs`). Tests: chunkgen 190 passed on
both builds. Interleaved A/B, one exclusive hold:

| | pair 1 | pair 2 | pair 3 | mean |
|---|---|---|---|---|
| head a6c1fcd | 575.2 | 626.1 | 578.6 | 593.3 |
| + E19 | 564.1 | 487.9 | 624.7 | 558.9 |

All six `digest=a977961a58122047`. E19 is behind in two of three pairs. Rejected; the diff is kept at
`docs/perf/rejected-E19-surface-chunk-fold.diff`. The 487.9 run may be an outlier, so a second
A/B could revive it, but the surface pass is now ~12% and its cheap wins are taken.

Head a6c1fcd reference from this hold: ~593 cps.

### E20: merge `origin/perf/noise` at f2b239a (doubled permutation table) -- neutral

The noise track handed over and stopped. Its kept change since the last merge is 6dfa05a:
the Perlin lattice is read from a doubled permutation table. The merge was clean. Tests on the
merged tree: noise 31, chunkgen 190, core 57, biome 6, all passed. Interleaved A/B, one
exclusive hold:

| | pair 1 | pair 2 | pair 3 | mean |
|---|---|---|---|---|
| pre-merge a6c1fcd | 615.6 | 569.4 | 661.4 | 615.5 |
| merged 2b7bd3e | 631.4 | 624.2 | 583.4 | 613.0 |

All six `digest=a977961a58122047`. No measurable change, matching the noise log's "gain
unconfirmed". Merged anyway, so this branch carries both tracks' kept work.

**For whoever picks up the noise track (flagged, not acted on):**
- `docs/perf/wip/noise-5-shared-surface-memo.patch` overlaps E18. Both memoise
  `preliminary_surface_level` per column across chunks. Only one should land. E18 is
  measured (+3.8%), exact by the E13 argument and keyed on `NoiseRouterEvaluator::id()`, so
  drop noise-5 or rebase it onto E18.
- `docs/perf/wip/noise-4-shared-corner-columns.patch` shares the 9 of 25 corner columns a
  chunk has in common with its neighbours. It targets the corner-grid build, now ~40% of
  generation and the largest single cost left. It is the best next lever, and it is noise-crate work.

Reference after this hold: **~614 cps** (pre-merge and merged both).

### Profile of the merged head (0e3cf61; 1 thread, radius 8, generation only)

| Share | Where | Owner |
|---|---|---|
| 44.5% | corner-grid build | noise |
| 5.8% | per-block density (`run_cell` etc.) | noise |
| 3.9% | ore veins' density sampling | noise |
| 14.2% | surface pass | datapath |
| 10.4% | aquifer (9.4% per block from the fill, 1.0% surface level) | datapath |
| 8.3% | carvers (1.4% of it in the aquifer) | datapath |
| 4.8% | fill loop self | datapath |
| 4.5% | biomes | datapath |
| 3.1% | ore veins' own code | datapath |

The noise crate is now over half of generation. Removing every datapath cost outright would
take this to ~2x, so further datapath gains are a few percent each, and each needs an
interleaved A/B to see through the box's ~15% spread. The biggest lever is the noise track's
corner grid (`wip/noise-4`).

Round 5, on 0e3cf61:
- `datapath-13`: the aquifer's per-block cost in the fill (incremental integer distances,
  provable whole-row answers).
- `datapath-14`: carvers with no cross-chunk state (per-step savings in the walk,
  `carve_ellipsoid`).

### bench-datapath.tsv backfilled (auditor's request)

The tsv had no row after 1da3c44's two invalid runs (226 and 264): every number since then came
from interleaved A/B holds run by hand, so they reached only this log. Appended, in the
script's format, one row per timed run of each kept head from those holds:
- 1da3c44 (3 runs);
- 85f6075 (3);
- ae011ba, the E16 build (3);
- a6c1fcd: the E18 build and the head side of the E19 and merge holds (9);
- 2b7bd3e, the noise merge (3).

Timestamps are each hold's box-clock times converted with `date -d`, a few seconds apart
within a round. Rejected variants (E14, E15, E17, E19) are not added: their rows are in
their own `bench-datapath-<n>.tsv`. From now on each A/B hold appends its kept-head rows here
as it runs.

Noted from the auditor: the noise session is back, has been told to drop noise-5 in favour of
E18, and will do noise-4 (shared corner columns) on top of this head. `NoiseRouterEvaluator::id()`
stays as the one datapath change in `oxide-noise`; the rest of that crate is left to noise.

### Round 5: E21 (aquifer per block) and E22 (carvers) -- three-way A/B inconclusive

**E21** (`/tmp/datapath-13`, `aquifer.rs` + `fill.rs`). Counters per chunk:
- 40,957 `compute_kind` calls; 73.5% of them are `density > 0` early returns.
- ~10.8k full searches, 68% ending at `similarity12 <= 0`.
- `Barrier` sampled 54 times.

The cost is the search itself, ~75 ns each. Changes:
- The fill answers `density > 0` itself.
- A one-row run memo. After a full fast-path search, `note_run` proves how far along +x the
  same answer holds, from exact integer gaps that are linear in x:
  - rule 1: every other candidate is >= 25 further, so `similarity12 <= 0`;
  - rule 2: every candidate whose cached status differs is >= 50 further, so everything weighed
    has the nearest's status and pressure returns 0.0 before any noise.

  Uncached statuses count as different. Proof reviewed by the master.
- Sub-agent runs: 630.5, 641.2. Tests: chunkgen 190 passed.

**E22** (`/tmp/datapath-14`, `carver.rs`):
- A private `WalkRandom` takes a tunnel step's seven LCG draws from precomputed jump
  constants instead of a chain of seven dependent multiplies; it is bit-identical to
  `LegacyRandom` (new test: 8 seeds, 2000 steps, interleaved splits).
- The step radius is computed only after `can_reach`.
- A conservative pre-check skips calls that `carve_ellipsoid`'s own first test rejects.
- `zd` and `yd` tables per ellipsoid, with the same expressions and summation order.
- `carvers_at`'s Vec is iterated by reference.
- No useful exact pre-walk skip for a whole tunnel exists: `can_reach` already ends far
  tunnels within a step or two.
- Sub-agent runs: 676.9, 599.7. Tests: chunkgen 191 passed (one new).

Three-way interleaved A/B (rows of the head are in `bench-datapath.tsv`):

| | round 1 | round 2 | round 3 | mean |
|---|---|---|---|---|
| head 0e3cf61 | 544.1 | 679.0 | 587.0 | 603.4 |
| + E21 | 635.6 | 667.7 | 578.7 | 627.3 |
| + E22 | 658.9 | 504.7 | 617.3 | 593.6 |

All nine `digest=a977961a58122047`. The head alone spread 544-679 (25%) in one hold, much more
than the ~4% being looked for. Next: head against head+E21+E22 combined (they touch different
files), four rounds.

### E21 and E22: rejected after the combined A/B

Head against head+E21+E22, both built at 0e3cf61, four rounds in one exclusive hold.
Tests on the combined build: chunkgen 191 passed.

| | r1 | r2 | r3 | r4 | mean |
|---|---|---|---|---|---|
| head 0e3cf61 | 630.8 | 679.4 | 642.5 | 659.5 | 653.1 |
| + E21 + E22 | 654.7 | 549.5 | 651.6 | 614.7 | 617.6 |

All eight `digest=a977961a58122047`. The combination wins two rounds and loses two, and its mean is 5% lower.
Neither shows a gain across 7 rounds in two holds, so both are rejected. Diffs kept at
`docs/perf/rejected-E21-aquifer-row-memo.diff` and `rejected-E22-carver-lcg-jump.diff`; both
are exact, and the proofs are above if someone wants to retry with better measurement.

### Handover (current state, next three)

Head: 0e3cf61 code (161812e with logs), **~610-650 cps**. The last two holds measured the
head at 603.4 and 653.1. Session start: 138.2. Gate M1 (400) is passed; M2 is 1,000.
Every kept change leaves `digest=a977961a58122047`.

Kept this session: E1, E2, E3, E5, E6 (bound margin), E8-E13, E16, E18, plus the two
noise merges. Measured as no gain: E4 (kept, harmless). Rejected after A/B, with diffs in
`docs/perf/rejected-*.diff`: E14, E15, E17, E19, E21, E22.

Where the time is (0e3cf61): the noise crate is over half of generation (corner grid 44.5%).
The datapath parts are 3-14% each. The noise session is doing noise-4 (shared corner
columns) on this head.

Measurement is now the limit. In one exclusive hold the same binary spreads 15-25%, more than
any remaining datapath change can win. Next three:
1. Better measurement before more datapath micro-work. Run A/B at a larger radius (24, 2401
   chunks) or on 1 thread, where scheduling noise is smaller, and use 5+ alternating rounds.
   Retry E21 and E22 under that: both are exact, and their counters say they remove real work.
2. Surface (~14%): the per-chunk folds lost twice (E14, E19), and the cheap folds are in
   (E16). Next would be evaluating the above-surface program column-wise across y runs,
   instead of per position.
3. Wait for noise-4, then re-profile. If it lands, the datapath shares rise and the items
   above become worth more.

PR into `feat/native-decoration` is still not opened: no `gh` on this host.

### Measurement check: 1-thread A/B of head vs head+E21+E22

Four rounds, one exclusive hold, radius 16, `--threads 1`; ms of core time per chunk:

| | r1 | r2 | r3 | r4 | mean |
|---|---|---|---|---|---|
| head 0e3cf61 | 10.85 | 11.76 | 11.73 | 11.56 | 11.48 |
| + E21 + E22 | 10.74 | 11.80 | 11.06 | 13.58 | 11.80 |

All eight `digest=a977961a58122047`. Still no consistent gain (better in two rounds, level in one, much worse in one), so the
rejection stands. On one thread the head spreads 8%, against 15-25% on eight threads, so this is the
better method for small changes. But the load average was 8.6 when this exclusive hold
began: something on the box runs outside the lock. Flagged for the auditor. Until that stops,
no A/B here can resolve a change under ~5%.

Single-thread head reference: **~11.5 ms core per chunk** (session start, from PLAN.md:
43.8 ms single-thread).

### Correction (from the auditor): the "load outside the lock" flag was wrong

The 8.6 load average I read at the start of the last hold is not evidence of unlocked work.
It is a decaying one-minute average, and a hold starts the moment the shared-lock builds it
waited for finish, so it reads high every time. The auditor's samples showed every heavy
process under the lock. Retracted. From now on a hold is judged by `ps` sampled during it,
and only a named process is flagged. The one real case this session was the pipeline track's
`oxide-pregen` running unlocked, seen by name in `ps`. The auditor confirms my head: a29cef5
at 620.3 and 695.4 cps on 8 threads, and 10.82 and 12.07 ms per chunk on 1 thread, against
Pumpkin's 17.4 ms terrain.

Next-three item 1 is running: 1-thread A/B, 5 alternating rounds of head, E21 and E22, with
`ps` sampled each round. Item 2, the column-wise surface program, goes to a sub-agent on
`datapath-16`.

### Item 1 done: E21 and E22 under a 1-thread, 5-round A/B -- both stay rejected

One exclusive hold, rounds in the order head, E21, E22. `ps` sampled at each round's start showed nothing
heavy besides the timed run. Radius 16, 1 thread, ms of core per chunk:

| | r1 | r2 | r3 | r4 | r5 | mean |
|---|---|---|---|---|---|---|
| head 0e3cf61 | 12.27 | 11.79 | 11.59 | 11.71 | 11.05 | 11.68 |
| E21 | 11.78 | 11.68 | 11.72 | 10.67 | 11.14 | 11.40 |
| E22 | 12.04 | 12.01 | 11.86 | 10.51 | 11.24 | 11.53 |

All fifteen `digest=a977961a58122047`. E21 is ahead in 3 of 5 rounds and 2.4% faster on the mean, but
most of that comes from round 4; the median per-round difference is ~1%. E22 is ahead in 2 of 5.
Neither gain is established, so both stay rejected. E21 is the weakly positive one: if the
aquifer share rises after noise-4, it is the first retry.

Method note: the head always ran first in each round, and round 4 ran faster for both variants,
so drift within a round is visible. Rotate the order across rounds from now on.

### E23 (next-three item 2): column-wise reuse of the above-surface program -- rejected

Sub-agent experiment in `/tmp/datapath-16`, `surface.rs`. Profile at the base (1 thread, radius 8,
inline(never) splits):
- surface 11.48% of generation;
- the above-surface `eval_program` 4.95% (`eval_condition` 3.23%);
- below-surface programs 0.33%;
- `apply` self 4.33%, `column_state` 1.32%.

So the interpreter cost left is almost all above the surface. Change: evaluate the above-surface program at a position, have each test on the decision path
report how far down the same run its answer is certain to hold, and reuse the outcome down to
the minimum of those. Reuse is dropped at air, fluid, an all-air section, and a biome change
when a biome or temperature test was on the path.
- Per-test spans: `y_above` and `water` with stone depth never change down a run; without it,
  until the threshold. Floor and ceiling `stone_depth` from the depth arithmetic.
  `vertical_gradient` gets span 0 inside its draw band. 3D noise, temperature above y 80 and
  a nested `not` get span 0.
- Debug builds re-run the full evaluation at every reused position and assert equality. All
  190 tests pass with that on, including `cell_culling_is_exact` on the reference datapack.
  Reviewed by the master.

Rotating-order A/B, 1 thread, 5 rounds, one exclusive hold; `ps` showed nothing heavy. ms core per chunk:

| | r1 | r2 | r3 | r4 | r5 | mean |
|---|---|---|---|---|---|---|
| head 0e3cf61 | 11.33 | 11.55 | 11.71 | 11.56 | 11.22 | 11.47 |
| + E23 | 12.03 | 11.55 | 10.91 | 11.23 | 12.21 | 11.59 |

All ten `digest=a977961a58122047`. Ahead in 2 of 5, level in 1, mean 1% slower. Rejected: the ceiling was ~3%,
and the per-test span work on every remaining evaluation costs about what the reuse saves.
The diff is kept at `docs/perf/rejected-E23-surface-column-reuse.diff`.

Next-three status: item 1 done (E21 and E22 stay rejected), item 2 done (E23 rejected), item 3
waits on the auditor's confirmation of noise-4 (d2433fb) before merging and re-profiling.

### E24 (next-three item 3): merge `origin/perf/noise` with noise-4 (d2433fb, shared corner columns)

Confirmed by the auditor's A/B against 18bb0f9, one exclusive hold (rows in
`bench-audit-noise.tsv` on `perf/audit`):
- 8 threads: 673.0, 669.4, 721.8, 686.9 against 491.3, 649.3, 613.2, 591.5. Ahead in all four
  rounds, +17% on the means.
- 1 thread: 10.37 and 10.58 ms per chunk against 11.41 and 12.07.
- `digest=a977961a58122047` on every run.

Merged clean. The merge tree is identical to the trial merge in `/tmp/datapath-17`, which passed
on the box: chunkgen 190, noise 32, all passed. Merged on top of noise-4: the shared
corner-column cache is noise's, keyed and owned in `oxide-noise`, and `NoiseRouterEvaluator::id()`
is untouched. Re-profiling on 1 thread next, to plan round 6.

### Profile on top of noise-4 (11ccdeb; 1 thread, radius 8, 10.32 ms per chunk with the profiler on)

Shares of generation:

| Share | Where | Owner |
|---|---|---|
| 37.4% | corner grid (was 44.5%) | noise |
| 7.0% | per-block density | noise |
| 3.6% | ore veins' density sampling | noise |
| 16.5% | surface (14.1%, plus 2.4% `column_state` noise) | datapath |
| 12.9% | aquifer: fill 10.5%, carvers 1.5%, surface level 0.9% | datapath |
| 8.0% | carvers, excluding the aquifer | datapath |
| 5.5% | fill loop self | datapath |
| 5.3% | biomes: sampling 2.8%, search 2.5% | datapath |
| 3.3% | ore veins' own code | datapath |

The datapath share is now ~52%, up from ~45%.

Round 6:
- E21 retried on 11ccdeb (`datapath-18`): the aquifer is now a bigger slice, and E21 was the weakly
  positive one (+2.4% mean, 3 of 5).
- `datapath-19` sub-agent: `SurfaceSystem::apply`'s own per-block work (~4.3%: slot walk,
  run state, cursor, biome lookup). Not the interpreter, where E14, E19 and E23 all lost.
- One standard 8-thread `bench-vps.sh` row of 11ccdeb for the progress page.

### E21 retried on top of noise-4 -- now kept

E21 (aquifer row memo, and the fill answering `density > 0` itself; described in round 5) rebased onto 11ccdeb
unchanged. Tests: chunkgen 190 passed. Rotating-order A/B, 1 thread, 6 rounds, one exclusive
hold, `ps` clean. ms core per chunk:

| | r1 | r2 | r3 | r4 | r5 | r6 | mean |
|---|---|---|---|---|---|---|---|
| head 11ccdeb | 10.55 | 10.22 | 10.09 | 10.24 | 10.81 | 10.51 | 10.40 |
| + E21 | 9.95 | 10.09 | 9.47 | 9.83 | 10.26 | 10.20 | 9.97 |

All twelve `digest=a977961a58122047`. Ahead in 6 of 6 rounds, with the order rotated: -4.2% core time.
Kept. Before noise-4 it was +2.4% with 3 of 5; with the corner grid cheaper, the aquifer is a
larger slice, and the rotating 1-thread method resolves it. The rejected-diff file is removed,
since the change is now in the tree.

### E25: surface per-block scan (column-major kinds, band shapes, top start) -- rejected

Sub-agent experiment in `/tmp/datapath-19` on 11ccdeb. Split profile: `apply` 14.7% inclusive, of which:
- interpreter 6.4%;
- own body 3.2%;
- `column_state` 2.0%;
- biome lookup 0.9%;
- kinds build 0.7%;
- writes 0.9%;
- `run_bottom` 0.6%.

Change:
- kinds as one column-major `u8` array;
- each column's walk starts at its top non-air byte, found 8 bytes at a time;
- below-surface bands classed Empty, Const or Eval: Empty is skipped, Const written directly;
- the biome looked up only when the program has a `biome` or `temperature` test, from a per-chunk
  section×quart table;
- `run_bottom` found lazily with one `rposition`.

Exactness reviewed by the master: air resets all scan state, empty and leading-block programs
have no side effects, and `flatten` leaves no nested `not` around a biome test. Tests: chunkgen
190 passed.

Rotating 1-thread A/B, 6 rounds, against 11ccdeb (ms core per chunk):

| | r1 | r2 | r3 | r4 | r5 | r6 | mean |
|---|---|---|---|---|---|---|---|
| head 11ccdeb | 10.52 | 11.22 | 10.25 | 10.10 | 9.92 | 10.85 | 10.48 |
| + E25 | 10.44 | 10.20 | 9.97 | 10.87 | 10.40 | 10.71 | 10.43 |

All twelve `digest=a977961a58122047`. Ahead in 3 of 6, behind in 3, mean -0.4%: no gain. Rejected; the diff is
at `docs/perf/rejected-E25-surface-column-major-scan.diff`. If anyone retries, try the band
shapes without the column-major transpose.

Head is now 20d218d (with E21): about 10.0 ms core per chunk on 1 thread from the E21 hold.

### Disk cleanup (auditor's request)

The bench box was at 95%. Deleted my finished scratch dirs `~/oxide-perf/datapath-{1..19}`,
`datapath-ab` and `datapath-m` (~35 GB); the disk is now at 77%. Kept `datapath` (head),
`datapath-20` (E22 retry, in flight) and `datapath-21` (biomes sub-agent, in flight). From now
on each `datapath-<n>` is deleted when its experiment is logged.

## Handover, session 1 (written at ~85% of the usage limit)

**Head: 20d218d** (741d96f with logs): datapath E1-E13, E16, E18, E21, plus noise merged up to
d2433fb (noise-4). Every kept change leaves `digest=a977961a58122047`.
- 1-thread reference: **~10.0 ms core per chunk** (E21 hold: 9.47-10.26, mean 9.97).
- 8 threads: 697.0 cps at 321c6eb, the head before E21; a fresh 8-thread row for 20d218d is in flight.
- Session start: 138.2 cps on 8 threads, 43.8 ms on 1 thread (PLAN.md).
- Pumpkin's terrain, per the auditor: 17.4 ms.

**In flight at handover:**
- `datapath-20`: E22 (carver LCG jump-ahead, ellipsoid tables) rebased onto 20d218d, followed by
  one `bench-vps.sh` 8-thread row of the head, then a rotating 1-thread 6-round A/B, head
  against E22. Keep E22 only if it is ahead in at least 5 of 6 rounds.
- `datapath-21`: sub-agent on `fill_biomes` (5.3%). The main idea is sampling y-independent
  climate slots once per quart column, which needs a bit-exact y-independence argument from
  this pack's programs. It only builds; the master A/Bs.
- **Not yet done:** merge `origin/perf/noise` at ff02449 (noise experiment 12: a gated add
  that skips the slide bodies where the gradient is exactly zero; the noise track's 1-thread hold
  was 9.77 against 10.43 ms, ahead in 5 of 5), run the tests, and re-reference on 1 thread. It
  waits for the E22 job, which rsyncs the main tree.

**Method that works** (scripts in the session scratchpad; recreate them from this description):
- Builds and tests on the box run as `flock -s ~/oxide-perf/.lock nice -n 10 sh -c '...'`.
- Each variant gets its own `~/oxide-perf/datapath-<n>` dir.
- A/B: one `flock -x` hold, 1 thread, radius 16, 5-6 rounds, order rotated each round, `ps`
  sampled inside the hold. A single 8-thread run spreads 15-25%; this spreads ~5-8%.
- Judge a hold by `ps`, never by load average.
- Kept heads get a `bench-vps.sh` 8-thread row in `bench-datapath.tsv` for the progress page.

**Profile on top of noise-4** (11ccdeb, before E21, generation only):
- noise crate ~48%: corner grid 37.4%, per-block density 7.0%, ore density 3.6%;
- surface 16.5%; aquifer 12.9% (now lower with E21); carvers 8.0%; fill loop 5.5%;
  biomes 5.3%; ore 3.3%.

**Rejected after A/B, diffs in `docs/perf/rejected-*.diff`:**
- surface folding and reuse: E14, E19, E23, E25;
- E15 (branch-free aquifer top-3);
- E17 (tunnel replay cache);
- E22 (pending its retry above).
The surface interpreter has resisted four attempts. Gains there need a different idea, not more folding.

**Next three:**
1. Merge noise at ff02449, test, and re-reference on 1 thread with a rotating hold, head before
   against head after. Then decide E22 and the biomes result against the merged head.
2. Re-profile the merged head, splitting surface and aquifer with temporary `#[inline(never)]`,
   to see what E21 left in the aquifer and whether `column_state` (2.4%, preliminary surface plus
   the surface noise per column) can share neighbour columns as E18 does.
3. Carvers (8%): E22 is the last cheap idea. If it fails again, the remaining cost is the
   tunnel walk itself, which is tied to `LegacyRandom`'s sequential draws.
   Leave carvers until the surface and aquifer shares are re-measured.

The PR into `feat/native-decoration` is still not opened: no `gh` on this host.

### E26: merge `origin/perf/noise` at 0c83b38 (includes ff02449, noise experiment 12)

Merged clean onto 2730aeb as a939acf. Tests on the box (`datapath-22`, shared lock): noise 34,
chunkgen 190, core 57, biome 6, all passed. Release build in `~/oxide-perf/datapath-22`. Pushed
before its own A/B so the branch carries it. The 1-thread re-reference (head 20d218d against this
merge, rotating hold) runs after the E22 A/B frees the lock queue. The noise track's own hold for
experiment 12 was 9.77 against 10.43 ms, ahead in 5 of 5.

### E22 retry lost to the full disk; E27 (biomes) reviewed; four-way hold queued

- The E22 rebuild in `datapath-20` failed with "No space left on device" before the cleanup above.
  Its A/B timed only the head (1 thread: 10.36, 10.81, 10.25, 10.70, 10.51, 10.80 ms). E22 is
  unmeasured, not rejected. It is being rebuilt now that the disk is at 77%.
- E27 (`datapath-21`, sub-agent). `fill_biomes` is ~4.1% of generation: climate ~2.0% (half of it the bare
  `shifted_noise` temperature and vegetation, evaluated per quart), search ~1.9%.
  - Change: the five non-depth climate slots are sampled once per quart column per chunk, in a
    thread-local keyed by `(router.id(), chunk pos)`. A hinted biome search seeds each column's
    search with the previous y's leaf, as a distance bound only.
  - Exactness: temperature and vegetation are `ShiftedNoise` with `y_scale` 0 and a shift
    `Const(0)`, so the noise y is `y*0.0 + 0.0 = +0.0` for every finite y. `Shift Xz`/`Zx`
    read no y. The other three slots are `FlatCache` over the same shape. Depth is still per quart.
    The hint bound `d_hint + 1` prunes only subtrees farther than the hint leaf, so the search
    returns the same first leaf at the minimum distance.
  - New tests: the hinted search against the plain one, for all hint kinds; the column fill against per-quart
    sampling on 5 chunks of the real pack; the proof rejecting y-dependent shapes.
  - **Caveat:** the y-independence proof parses `dump_program`'s text listing. It fails closed,
    but it belongs inside `oxide-noise` as a predicate on the compiled program (noise track's
    file). Expected gain is ~1-2%, so it is kept only on a clear win.
  - Sub-agent's run: 9.71 ms, 1 thread. Tests: biome 7, chunkgen 192 passed.
- One rotating 1-thread hold, 4 rounds, decides three things at once: the head (20d218d), the
  noise merge (0823fd8, `datapath-22`), E27 (`datapath-21`) and E22 (`datapath-20`).

### Four-way hold: suspect, decided nothing

Tests for the E22 rebuild: chunkgen 191 passed. Rotating 1-thread hold, 4 rounds, ms core per chunk:

| | r1 | r2 | r3 | r4 | mean |
|---|---|---|---|---|---|
| head 20d218d (`datapath`, built 10:55 by `bench-vps.sh`) | 10.35 | 10.50 | 10.25 | 10.54 | 10.41 |
| noise merge 0823fd8 (`datapath-22`) | 9.32 | 9.42 | 9.90 | 11.25 | 9.97 |
| E27 biomes (`datapath-21`) | 10.15 | 9.32 | 9.53 | 10.03 | 9.76 |
| E22 carvers (`datapath-20`) | 9.93 | 9.61 | 9.80 | 10.52 | 9.97 |

All sixteen `digest=a977961a58122047`. **Not trusted.** E27 beats the head by 6.3%, which is more than all of
`fill_biomes` (4.1% of generation), and all three variants beat the head by similar margins.
The head binary itself looks ~4% slow, from build or layout variance between separately built
binaries, not from code. Nothing is kept or rejected on this hold.

**Next hold (do this first):** include two separately built binaries of the same head source,
`datapath` and a fresh `datapath-h2`, to measure binary-to-binary variance. Then compare each variant
against both. If the two head builds differ by ~4%, every A/B of separately built binaries
this session has had that floor. The E21 keep (6 of 6, -4.2%) should then be confirmed again too.

## Handover update (at the usage limit)

- Head: **0823fd8** = 20d218d + noise merge 0c83b38 (noise experiment 12). Tests pass (noise 34,
  chunkgen 190, core 57, biome 6). Its own A/B is in the suspect hold above, so it is
  merged on the noise track's measurement (9.77 against 10.43 ms, 5 of 5).
- 8 threads: 701.5 cps at 741d96f (= 20d218d code), the last tsv row.
- Built and waiting for a trustworthy hold: E22 in `~/oxide-perf/datapath-20` (on 20d218d), E27 in
  `datapath-21` (on 741d96f). Both diffs are in those worktrees: `/tmp/datapath-20`, `/tmp/datapath-21`.
  Delete those dirs, and `datapath-22`, after the next hold.
- Next three:
  1. Two-head-binary hold as above, to set the variance floor. Then decide E22 and E27 against
     it, and re-check E21.
  2. If E27 lands, move its y-independence check into `oxide-noise` as a predicate on the
     compiled program (ask the noise track) instead of parsing `dump_program` text.
  3. Re-profile 0823fd8 on 1 thread with `#[inline(never)]` splits of surface and aquifer,
     and pick the next target from it.

### Variance hold: two head builds against the variants -- E22 and E27 kept

Rotating 1-thread hold, 4 rounds. `datapath` and `datapath-h2` are two separate builds of the
identical head source (20d218d). On the box, the sources differ only by each variant's intended
files (checked with `diff -rq`). ms core per chunk:

| | r1 | r2 | r3 | r4 | mean |
|---|---|---|---|---|---|
| head build 1 | 10.27 | 11.04 | 10.44 | 10.22 | 10.49 |
| head build 2 | 10.42 | 9.71 | 10.55 | 10.32 | 10.25 |
| noise merge 0823fd8 | 10.00 | 8.64 | 9.95 | 10.00 | 9.65 |
| E27 biomes | 9.51 | 8.98 | 10.02 | 9.71 | 9.56 |
| E22 carvers | 9.80 | 9.88 | 9.73 | 9.99 | 9.85 |

All twenty `digest=a977961a58122047`.
- The two head builds differ by 2.3%, mostly in one round, so build variance does not explain the
  four-way hold. The suspicion about it was wrong.
- **E27** beats both head builds in all 8 comparisons, mean -7.8%. That is more than its 4.1% profile
  share, so part of it is probably layout or profile under-count. It is exact and has three new tests. Kept.
- **E22** beats both head builds in 7 of 8, mean -5.0%. Kept. It was neutral on the pre-noise-4
  head, where carvers were a smaller slice.
- The noise merge: -7%, matching the noise track's hold.

Both are applied onto 0823fd8; they touch different files from each other and from the merge.
Combined tests: biome 7, chunkgen 193, noise 34, all passed. The combined head itself has not yet
been through a hold against 0823fd8. That is the first thing for the next session, together
with moving E27's y-independence check into `oxide-noise`.

8-thread row for 841a929 (combined head): `BENCH chunks=1089 threads=8 secs=1.552 cps=701.8 ms_per_chunk_core=11.40 digest=a977961a58122047`.
Level with 741d96f's 701.5. At 8 threads one run spreads 15-25%, so the 1-thread holds above are
the measure of record. Box dirs `datapath-20`, `-21`, `-22` and `-h2` are deleted; only `datapath` remains.

## Session 2

### E28: merge `origin/perf/noise` at 65ba9fc; E27's text parser replaced by `is_y_independent`

The auditor read 841a929 for exactness and it holds: hinted search bound, column climate, LCG
jump, reach skip, zd/yd tables.

Merged 65ba9fc as e3d85c9. It brings b1e4a5c, `NoiseRouterEvaluator::is_y_independent(slot)`: a
predicate proven on the compiled program. A 2D `shifted_noise` qualifies only with a constant
`shift_y` that is neither -0.0 nor NaN. Any `interpolated` node disqualifies the program. `biome_grid.rs` now
calls it in place of parsing the `dump_program` listing. The parser (~200 lines) and its
parser-only test are gone. `column_climate_fill_matches_per_quart_sampling` still asserts that
the overworld's five slots qualify and depth does not. `cargo fmt` also reflowed E21's and
E22's unformatted lines in `aquifer.rs` and `carver.rs` (formatting only).

Tests on the box: chunkgen 192 (193 minus the parser test), biome 7, noise 35, core 57, all
passed.

### Profile of the combined head (b1f77a6; 1 thread, radius 8, 11.15 ms per chunk with the profiler on)

Shares of generation:

| Share | Where | Owner |
|---|---|---|
| 34.8% | corner grid | noise |
| 8.4% | per-block density | noise |
| 4.5% | ore density sampling | noise |
| 19.4% | surface: pass 16.1%, `column_state` 3.3% | datapath |
| 11.9% | aquifer: fill 8.3%, carvers 2.8%, surface level 0.8% | datapath |
| 8.2% | carvers, excluding their aquifer calls | datapath |
| 4.9% | fill loop self | datapath |
| 3.7% | biomes (was 5.3%, before E27) | datapath |
| 3.5% | ore veins' own code | datapath |

Round 7 (auditor's item 3), two sub-agents on the two largest datapath items. Both only build;
the master measures with rotating 1-thread holds against two base builds.
- `datapath-23`, surface. Splits first, then the dominant non-interpreter part. The lead is
  `column_state`'s exact-column `PreliminarySurfaceLevel` scan: 256 per chunk, where the aquifer
  memoises the same slot per quart. Is the value quart-constant, given the `flat_cache` snapping?
- `datapath-24`, aquifer. A column-run memo along -y for the carvers' call order, plus counters
  on why the fill's row memo still misses.

Item 2 (the combined head against two builds of 0823fd8) is running. The profile dir
`datapath-p` is deleted.

### E29: surface samples `PreliminarySurfaceLevel` per quart, not per column -- parked

Sub-agent experiment in `/tmp/datapath-23`. Split profile at b1f77a6:
- `apply` 15.6% inclusive, of which `eval_program` 7.3% and `eval_condition` 4.7%;
- `apply`'s own body 5.3%;
- `column_state` 2.5%: the `PreliminarySurfaceLevel` scan 1.4%, the `surface` noise 0.9%. The noise
  depends on exact (x, z), so it is left alone.

A cross-chunk memo would not help here: each block column belongs to one chunk.

Change: `PreliminarySurfaceLevel` is sampled at each quart's origin column, which the z, x scan
reaches first, and reused for the other 15 columns. Exact for this pack: the scan's inputs `offset`
and `factor` are `cache_2d` reads of `flat_cache` nodes, which sample at the quart origin, and the
rest is gradients, constants and arithmetic. `is_y_independent` covers the cache-purity half.
New tests: the exact and quart-origin samples are bit-equal at all 256 columns of 5 reference
chunks, and the whole pass gives identical blocks per quart and per column.

Parked, not landed:
- It proves the quart-snapping half by parsing `dump_program`, which E28 just removed from
  `biome_grid.rs`.
- Its ceiling is ~1.3% of generation, below the bar and below what a 1-thread hold resolves.

**Ask for the noise track:** `NoiseRouterEvaluator::is_quart_invariant(slot)`, a predicate on the
compiled program like `is_y_independent`: the value at `(x, z)` equals the value at
`(x & !3, z & !3)` through any caches. With that, E29 is ~10 lines in `surface.rs`. The diff is at
`docs/perf/parked-E29-preliminary-per-quart.diff`.

### Auditor item 2: combined head against 0823fd8 -- no measurable gain

Rotating 1-thread hold, 5 rounds. `datapath` is the combined head (b1f77a6 code:
0823fd8 plus E22, E27 and noise 65ba9fc). `datapath-b1` and `datapath-b2` are two separate
builds of 0823fd8. ms core per chunk:

| | r1 | r2 | r3 | r4 | r5 | mean |
|---|---|---|---|---|---|---|
| combined b1f77a6 | 9.84 | 8.90 | 9.71 | 9.27 | 9.12 | 9.37 |
| 0823fd8 build 1 | 9.66 | 9.41 | 9.22 | 9.56 | 9.88 | 9.55 |
| 0823fd8 build 2 | 9.44 | 9.07 | 9.26 | 9.40 | 9.48 | 9.33 |

All fifteen `digest=a977961a58122047`. The combined head is -0.7% against the base builds' mean (9.44):
-1.9% against build 1 and +0.4% against build 2. The two base builds differ by 2.3%.

**So E22 and E27 show no measurable gain on top of the noise merge.** In the variance hold they
each beat the pre-merge head (20d218d) by 5-8%, more than E27's whole profile share. Either
their gains overlap with what the noise changes (experiment 12, and 65ba9fc) removed, or that
hold was biased against its head binaries in a way the source diff did not show. Both stay in
the tree: they are exact (the auditor read them), tested, and not slower. **Their speed claims
are withdrawn.** The 2.3% spread between two builds of identical source is the floor for any
claim from separately built binaries. A change needs a consistent lead well beyond it.

Box dirs `datapath-b1` and `datapath-b2` are deleted.

### E30: aquifer anchor slab and -y column run -- not landed (below the build floor)

Sub-agent experiment in `/tmp/datapath-24`, `aquifer.rs`. Counters per chunk (temporary build):
- the fill: 10,492 calls reach `compute_kind`;
- the row memo misses mostly because rule 2 fails partway along a row: some status uncached
  or truly different (1,934 of 1,940 remaining searches). Not at the anchor-segment end (34);
- carvers: 873 calls, and the row memo hit only 13, because their order is x, z, y-descending.

Change:
- **Anchor slab:** prove one answer for every in-chunk column mapped to the anchor, over a y range.
  Within one anchor the gap `d_j - d_p` is affine in each axis (the squares cancel), so its
  minimum over the slab's x/z box is at the box ends, and the E21 rules give a y interval by
  floor division.
- **Column run:** along -y for the carvers, clamped above the lava floor and inside the anchor.

Full searches per chunk: fill ~1,940-2,980 down to 1,409; carvers 857 down to 410. Reviewed as exact by the master.
Tests: chunkgen 192 passed.

Rotating 1-thread hold, two head builds (b1f77a6 code), 5 rounds, ms core per chunk:

| | r1 | r2 | r3 | r4 | r5 | mean |
|---|---|---|---|---|---|---|
| head build 1 | 9.31 | 10.47 | 9.59 | 9.82 | 9.23 | 9.68 |
| head build 2 | 9.58 | 9.57 | 9.32 | 9.96 | 8.93 | 9.47 |
| + E30 | 9.53 | 9.21 | 9.85 | 9.37 | 9.26 | 9.44 |

All fifteen `digest=a977961a58122047`. -1.5% against the head builds' mean, -0.3% against build 2.
The two identical head builds differ by 2.2%, so this is inside the floor. Not landed; the diff
is at `docs/perf/rejected-E30-aquifer-slab-and-column-run.diff`. Its own estimate was
1.5-2.5% of generation, so the result is consistent with a real but unresolvable gain.

### Where the datapath track stands (session 2)

Head **b1f77a6**: 1-thread ~9.4-9.5 ms core per chunk in the last two holds. Session-1
start: 43.8 ms (PLAN.md). 8 threads: 701.8 cps at 841a929, which was level with the pre-merge
head at that spread.

Every remaining datapath item is now one where an exact change can win only ~1-2.5% of
generation:
- surface interpreter: four attempts lost;
- `column_state`: ~1.3%, parked as E29, waiting on a predicate;
- aquifer: E30;
- carvers: E22 landed, gain unconfirmed;
- biomes: E27 landed, gain unconfirmed.

The measurement floor between separate builds is ~2.2-2.3%. Until a lever above that turns up,
further datapath micro-work cannot be confirmed on this box. The noise crate is ~48% of
generation. Next three:
1. Have the noise track add `is_quart_invariant(slot)`. Then E29 is ~10 lines. Measure E29 and
   E30 together: combined they are ~3-4%, which a hold can resolve.
2. A structural lever, not a micro one: the surface pass and the fill both walk every column. A fused
   fill-plus-surface pass per column, writing each block once, may save the kinds rebuild and
   the second scan. It is a larger change and needs a design pass first.
3. Re-measure with `--radius 24` (2,401 chunks) and 7+ rounds, to get the floor below 2%.

## Session 2, continued: the auditor's picks

The auditor answered the open question:
- (a) merge noise at 3362440 first;
- (b) do the fused fill-plus-surface pass, with the design and the exactness argument first;
- (c) not radius 24: use 7 rounds at radius 16 instead;
- (d) a bigger lever. 8 threads has stayed at ~700 cps (11.4 ms core per chunk) while 1 thread fell
  to ~9.4 ms, so ~20% is lost to scaling, and the plan's gates are 8-thread numbers.

### E31: merge `origin/perf/noise` at 3362440

Brings 01528de, lane-batched corner columns with hash reuse (noise: -6 to -9% on 1 thread
against two bases in a 7-round hold). Merged clean as 10c8523. Tests on the box: noise 38,
chunkgen 192, core 57, biome 6, all passed.

Two sub-agents, both based on this head:
- `datapath-25`: the fused fill-plus-surface pass. A written design and exactness argument
  first, then the implementation, building only.
- `datapath-26`: where 8-thread scaling is lost: per-chunk allocation and the allocator,
  thread-local memo footprint against Zen 1's caches, the shared corner-column cache, rayon
  scheduling. Measured with alternating 8-thread holds of 6+ rounds.

### Fused fill-plus-surface pass (`datapath-25`): stopped after the design stage, as allowed

From the sub-agent's design (profile at 0f15cef, 1 thread):
- `fill_chunk_culling` 45.2% inclusive;
- `SurfaceSystem::apply` 20.4%: `eval_program` 9.6%, `apply` self 6.9%, `column_state` 3.2%;
- ore 4.3%, carvers 10.3%, heightmaps 0.5%.

The surface rules need the whole column: `stone_depth_above` and water height come from above,
`stone_depth_below` from below. The fill runs layer by layer, y ascending, which the noise
track's cell batching relies on. So a full fusion means a column-major fill: a rewrite with a
remap of all 98k positions at the end, saving nothing. The largest palette-neutral piece is the
fill emitting `kinds` for surface (and folding the heightmap into the fill), worth ~1-1.5%: below
the 2.2-2.3% build floor. Not implemented; that was a valid result under the brief. The box dir is deleted.

**Finding: the digest does not cover palette order.** `merkle.rs` `hash_sub_region` hashes
`block.to_string()` per position, so `a977961a58122047` cannot see palette order or dead palette
entries. Both reach anvil NBT and `bits_per_entry`. Every palette-touching change this session
(E1, E2, E9, E10, E11, E16, E22) argued that it preserves first-write order, but the digest did not
check it. Checking now: a throwaway harness build (not committed) prints a second digest over
every section's block and biome palettes in order, plus their packed longs. It runs at the
session start (3fd5cf6) and at the head (0f15cef).

### 8-thread row for 0f15cef: 392.9 cps -- under investigation

`BENCH chunks=1089 threads=8 secs=2.772 cps=392.9 ms_per_chunk_core=20.36 digest=a977961a58122047`,
against ~700 at b1f77a6. Either the box was disturbed, or the 3362440 noise merge (01528de,
lane-batched corner columns with a shared cache, measured by noise on 1 thread only) regresses
at 8 threads. The scaling sub-agent (`datapath-26`) is told to make 0f15cef against b1f77a6 at 8
threads its first exclusive hold.

### The gate covers block states only -- not palettes, biomes or heightmaps

`crates/oxide-harness/src/merkle.rs` `hash_sub_region` hashes `section.block_states.get(..)`
`.to_string()` per position, and nothing else. So `a977961a58122047` does not see:
- palette order or dead palette entries (NBT and `bits_per_entry`);
- **biomes at all**: E1, E12 and E27 changed biome code, checked only by their own unit tests (E27's on 5 chunks);
- **heightmaps**: E1 rewrote `compute_heightmaps`. The surface `steep` rule reads `OceanFloorWg`
  indirectly, but the stored maps go to NBT unchecked.

Reading review (during the s5 reservation), change by change:
- **Palettes:** E1, E2/E11/E16, E9, E10 and E22 all intern a state at its first write into a section
  via `index_of_or_insert` on the same `BlockState`, where the old per-block `set` did.
- **Biomes:** E1 and E12 keep per-quart values and `(qy, qz, qx)` write order. E27 rests on its
  y-independence and hint-bound arguments.
- **Heightmaps:** E1 uses the same air-by-name / default-fluid-by-equality / else-solid
  classification, with the same stopping point.

No violation found by reading.

Empirical check, queued behind the reservation: a throwaway harness (not committed) at 3fd5cf6
and 0f15cef prints, besides `BENCH`, `PALETTE digest=` (every section's block and biome palettes
in order, plus packed longs, which also covers biome values) and `PALETTE heightmaps digest=` (all
six heightmaps, packed, in a fixed order). They must match between the two commits.

**Proposal (for the auditor; the harness is not this track's):** make the gate cover all three.
Either fold the palette and heightmap hashes into the `digest=` field, or print them as extra
`BENCH` fields, so `bench-vps.sh` records them and every track's changes are checked for them.
A new gate value then has to be established once, at a commit everyone agrees on.

### Interim gate (auditor): pregen NBT digest for every kept terrain change

The auditor is taking the harness change to the user; the harness has no owner, so it is not committed here. Interim evidence: pipeline's pregen digest (blake3 over
the chunk NBT) was `df701ee858e2bdd3` at radius 16 on every merge with datapath up to 2127bd7.
That covers whatever of palettes, biomes and heightmaps the NBT carries, for E1-E28.

From now on, every kept terrain change also gets one `oxide-pregen --no-write` run, on a tree merged
with `origin/perf/pipeline`, under the shared lock, with the digest logged. A scratch merge
(head a95c316 + perf/pipeline d7d5ea8; `scripts/bench-vps.sh` conflict taken from pipeline,
never pushed) is ready in `/tmp/datapath-pg`. It runs at radius 16, so it compares directly
against `df701ee858e2bdd3`. The only terrain change on this branch since 2127bd7 is the
3362440 noise merge.

Order once the box is back (~15:55 UTC):
1. the scaling sub-agent's 8-thread hold, 0f15cef against b1f77a6. The 392.9 row was timed at
   14:56 UTC, minutes before the reservation, so it is suspect;
2. the queued throwaway palette/heightmap check, 3fd5cf6 against 0f15cef;
3. the pregen r16 digest on `/tmp/datapath-pg`.

### Palette and heightmap check: identical at the session start and the head

Throwaway harness (not committed), radius 16, 8 threads, run after the reservation (~15:58 UTC):

| | 3fd5cf6 (session start) | 0f15cef (head) |
|---|---|---|
| block-state gate `digest=` | a977961a58122047 | a977961a58122047 |
| `PALETTE digest=`: block and biome palettes in order, plus packed longs (covers biome values) | 04da69623d0ddc8f | 04da69623d0ddc8f |
| `PALETTE heightmaps digest=`: all six heightmaps, packed, in a fixed order | 16f8026ca33ac948 | 16f8026ca33ac948 |

So every change this session (E1-E28 and the noise merges) leaves palette order, dead entries,
biomes and heightmaps exactly as they were. The reading review was right.

The same job's head run gave `cps=712.0 ms_per_chunk_core=11.24` at 8 threads. That was under the shared
lock, not an exclusive timing, but it is far from the suspect 392.9 row: that row looks disturbed
rather than a regression. The scaling sub-agent's exclusive 8-thread hold will settle it. The box dirs
`datapath-pa` and `datapath-pb` are deleted.

### Pregen NBT gate on the head: `df701ee858e2bdd3`, unchanged

The scratch merge of a95c316 (0f15cef code) with `origin/perf/pipeline` d7d5ea8 was built and run under the shared lock:
`oxide-pregen --datapack reference --seed 12345 --world out/gate --center-x 0 --center-z 0 --radius 16 --threads 8 --no-write --bench`.
Without `--bench`, pregen prints no BENCH line.
`BENCH chunks=1089 threads=8 secs=5.712 cps=190.6 ms_per_chunk_core=41.96 digest=df701ee858e2bdd3 generate_ms=2983 decorate_ms=2528 serialise_ms=106 compress_ms=95 write_ms=0 compression=zlib6 mb_disk=0.0`
This matches pipeline's reference from merges up to 2127bd7, so the 3362440 noise merge also leaves
the full NBT unchanged. The box dir `datapath-pg` is deleted.

### 8-thread scaling (`datapath-26`): interim report

**The 392.9 row was the box, not the code.** A java server (another user's) was at 362% CPU on
the box at 16:04, outside the bench lock. In an exclusive 6-round rotating hold at 8 threads
(17:04-17:10 CET), 0f15cef averaged 750.5 cps (627-834) against b1f77a6's 728.5 (707-747). No
regression from the 3362440 noise merge.

- **CPU, 1 thread against 8:** generation user+sys is 10.3-10.5 s against 11.1-11.7 s (+8-12%). Tail
  and start-up idle are 0.4-0.6%. In clean rounds the loss is only ~2-5%. Some rounds of both binaries run ~25% more
  CPU (13.2 against 10.2 CPU-s), cause unknown. The suspect is a neighbour VM on the KVM host:
  steal reads 0, so it cannot be seen from inside.
- **Per phase, CPU per chunk at 1 and 8 threads:** fill 5.72 and 6.46-6.50 ms (+13%, most of the increase);
  ore 0.67 and 0.69; surface `new` 0.18 and 0.19; surface `apply` 1.69 and 1.72-1.79; carvers 1.25 and 1.24-1.30.
- **E18 duplicates work across threads:** `preliminary_surface_level` is 1.37% of CPU on 1 thread
  and 4.10% on 8, because each thread rescans columns its neighbours already scanned. Proposed fix:
  a lock-free table shared by all threads, with one atomic u64 per column holding key and level.
  First hold: +8% (base 684.8, fix 739.9 cps). Not trusted yet: java was not logged, and both
  binaries show the fast/slow split. A third hold, with java logged per round, is asked for.
- **Allocation:** ~11.4k allocations and 7 MB per chunk, flat from 1 to 8 threads. Most are in
  oxide-noise: fill makes 7.2k allocations and 6 MB, mostly scratch vectors per `run_cell`/`run_lanes`
  call (noise track). Carvers make 2.7k, including the harness's `carvers_at` Vec clone 289 times
  per chunk (harness). sys time is +0.1-0.2 s and minor faults are flat, so the allocator is not the main cost.
- **Shared corner-column cache:** hit rate 54.1% on 1 thread and 50.0% on 8 (about 4.8k more columns
  recomputed, ~1% of CPU). A lock was waited for in 0.06% of acquisitions. Contention is ruled out.

**For other tracks:**
- noise: the ~6 MB per chunk of scratch allocation in `run_cell` / `run_lanes` (reuse it per thread);
  this may also explain fill's +13% at 8 threads.
- harness: `carvers_at` clones the carver Vec for each of the 289 source chunks per chunk.

**New rule (auditor):** log `pgrep -c java` and the java CPU at the start of every hold, and do
not trust an 8-thread hold taken while it is busy. `ab1.sh` now does this every round.

### Noise merge bd442e7 (`is_quart_invariant`, fd4bfe2): tests pass

Tests on the box: chunkgen 192 and noise 39 passed. This merge was pushed with 1b0682e before its tests
had finished, which was a process slip; they passed afterwards, so nothing needs reverting.

### E29 + E30 together, against two head builds: rejected

E29 now uses `router.is_quart_invariant(RouterSlot::PreliminarySurfaceLevel)` instead of parsing
`dump_program`; the surface pass samples at y 0, so quart invariance alone is enough. Its two tests stay. E30
(aquifer anchor slab and -y column run) is unchanged. Both are applied on bd442e7. Tests: chunkgen
194, noise 39, all passed.

Rotating 1-thread hold, 7 rounds, ms core per chunk:

| | r1 | r2 | r3 | r4 | r5 | r6 | r7 | mean |
|---|---|---|---|---|---|---|---|---|
| head build 1 | 9.60 | 9.30 | 9.67 | 10.44 | 9.60 | 9.67 | 10.05 | 9.76 |
| head build 2 | 9.77 | 9.62 | 9.45 | 8.98 | 9.83 | 9.57 | 9.71 | 9.56 |
| + E29 + E30 | 9.50 | 10.02 | 9.92 | 10.15 | 9.26 | 10.25 | 10.13 | 9.89 |

All twenty-one `digest=a977961a58122047`. +2.3% (slower) against the head builds' mean. Rejected.
The diff is at `docs/perf/rejected-E29-E30-combined.diff`.

Caveat: one java process was present in every round. My log printed `ps -o pcpu` (124-167%), which is
a lifetime average, not the current load. A one-second `/proc` sample right after the hold showed
java at **98% of one core**, so the hold ran with roughly one of 8 cores busy outside the lock. A
rotating 1-thread hold should tolerate that, but it is noted. The helper now samples java over 1 s
from `/proc/<pid>/stat` at each round.

Box dirs `datapath-27` and `datapath-h2` are deleted.

The auditor accepts the 8-thread result: the 392.9 row is struck. The `run_cell` / `run_lanes` scratch
allocation is passed to the noise track.

On the E18 shared table, the auditor requires, before any hold, a written argument that a lookup can
never return another column's or another router's level. If the key is truncated to fit beside the
level in 64 bits, a collision would be a silent wrong value that the digest at one seed may not catch. So:
either store the full (router id, x, z) and verify it on read, or prove the packing injective over the
whole reachable coordinate range. Racing writers are fine, since the value is deterministic. Then:
an 8-thread hold of 6+ rounds with java's current CPU sampled per round (from `/proc`, not
`ps -o pcpu`), and a 1-thread hold to show the table costs nothing there. Passed to the sub-agent.

### Hold method under outside load (auditor, while the java server stays)

The java server on s5 (another user's) has used 1-6 cores for hours and is not ours to stop.
Until it goes:
- every timed run is wrapped in `/usr/bin/time -f '%U %S'`, and its user+sys CPU seconds are logged
  beside the BENCH line;
- every run also logs java's CPU seconds during that run (utime+stime delta from `/proc/<pid>/stat`);
- holds rotate the order. Rounds where java used more than ~2 CPU-seconds per 10 s of wall time are
  dropped, and the decision rests on the rest;
- the comparison is on CPU seconds, which is steadier than wall cps under outside load and
  fairer at 8 threads. Wall cps is still recorded.

`/usr/bin/time` covers the whole harness process: datapack load and the single-threaded digest pass
as well as generation. Those parts are identical work for every variant, so compare absolute CPU-second
differences between variants; ratios are diluted. The master's helper (`ab2.sh`, scratchpad) does
this. The scaling sub-agent is switched to it: 8 rounds, no more waiting for java, and the
key-injectivity argument for the shared table first.

### 8-thread scaling (`datapath-26`): final report; E32 shared surface table parked

**Where the ~20% went.**
- Mostly the box, not the code. The java server (up to 598% CPU) and other jobs run outside the bench
  lock; in one 15:56 run, 8 threads got about 1.5 cores.
- 0f15cef is not slower than b1f77a6 (750.5 against 728.5 cps).
- Idle time is ~0.5%. In quiet rounds, CPU per chunk at 8 threads matches 1 thread (~9.6 ms).
- Real extra CPU at 8 threads is ~8-12% of user+sys:
  - fill +13% (5.72 to 6.46 ms per chunk);
  - E18's per-thread memo rescanning other threads' columns (1.37% to 4.10% of CPU);
  - the corner-column cache hit rate falls from 54% to 50% (~1%; lock waits 0.06%, so not contention).
- About half the 8-thread rounds of every binary burn ~25% more CPU, cause unexplained. Host-level interference is
  likely: steal reads 0 inside the VM.

**E32: a table of preliminary-surface levels shared across threads, in `aquifer.rs`.**
- Each entry is one `AtomicU64`. Bits 63..40 hold `qx`, bits 39..16 `qz`, both 24-bit two's complement
  from `(x & !3) >> 2`. Bits 15..0 hold the level as an `i16`.
- The table is used only when both `qx` and `qz` lie in [-2^23, 2^23), i.e. +-33.5M blocks, beyond
  the +-30M world border. On that range the 24-bit truncation is a bijection, so the 48-bit key is
  injective, and the full key is compared on every read. Columns outside the range skip the table.
- Levels outside [-32767, 32767] are not stored. `i16::MIN` (0x8000) marks an empty entry and is never
  stored, so an empty word never matches, including column (0, 0), whose key is 0.
- There is one table per router, found by `router.id()`. Ids are never reused, and a router is
  immutable after construction. The registry holds 8 routers: an evicted table stays alive in the
  aquifers holding it, and a fresh table only ever receives its own router's values.
- A single aligned atomic load or store can't tear, and `Relaxed` ordering suffices. Racing writers store the same
  deterministic value. A direct-mapped collision overwrites, and the other column then misses.
- The value is a function of (router, quart column) by the E13 argument, already relied on by E18.
- New test `shared_surface_entries_are_exact`. Tests: chunkgen 193, core 57, biome 7, all passed.
  `digest=a977961a58122047` on every run.

Holds, CPU-seconds method with java CPU-seconds per run; a round is dropped when java used more than 0.2 times its wall time:
- **8 threads, 8 rounds:** only 3 base and 2 fix rounds survived the java filter. Kept: base 717 cps
  (37.38 CPU-s), fix 731 cps (36.49 CPU-s). All 16 runs: 692.4 against 691.1 cps, 37.56 against
  37.08 CPU-s. No gain resolved.
- **1 thread, 8 rounds, all kept** (java 0.66-1.76 CPU-s per ~35 s run): median 9.63 s against 9.78 s,
  CPU 34.84 against 35.31 s. It costs 0 to ~1.5%, inside the spread.

**Parked, not landed:** no measured gain, and it adds cross-thread state. The diff is at
`docs/perf/parked-E32-shared-surface-table.diff`. Retry it at 8 threads once the box is quiet: the
profile says it removes ~2% of CPU there.

**For other tracks:**
- noise: fill makes ~7.2k allocations and 6 MB per chunk, mostly scratch `regs`/`vec!` buffers in every
  `Program::run_cell` / `run_lanes` call. Reuse them per thread or per `ChunkCaches`. The
  `CornerColumns` shards are 104 bytes apart, not padded to 64; padding is cheap but worth well
  under 1%.
- harness: `carvers_at` clones the carver Vec 289 times per chunk (~2.7k allocations).
  `SurfaceSystem::new` runs per chunk (~0.18 ms, ~2%) and needs an API to build once per thread.

The sub-agent deleted `datapath-26b` and `-26c`; `datapath-26` is deleted now.
Housekeeping noted by the sub-agent: two retry loops took the exclusive lock for ~2 s each and aborted with nothing timed.

### Live server ~115 cps against harness ~700: where the 6x can go (read-only analysis, for the user to assign)

A sub-agent read the code and logs; nothing was built or run, and no server was touched. Same terrain code: the ~115 cps
was measured on perf/datapath bf577ba, Vertex path, `native-decoration=false` (LOG-pipeline.md
647-650), and `oxide-ffi` and `plugin/` are unchanged since. At 115 cps on 8 cores a chunk costs
~70 ms of core time, of which Oxide is ~15 ms and the boundary under 1 ms. **The rest, ~55 ms, is
Java.**

**Call path.**
1. Vertex calls `OxideChunkHook.generate`. The hook is registered for `BIOMES, SURFACE, CARVERS`
   (`OxideVertexModule.java:51-52`), and the handle is cached in a `ConcurrentHashMap`.
2. Java opens `Arena.ofConfined()` per chunk and allocates 196,608 B for blocks plus 3,072 B for biomes
   (`OxideChunkHook.java:72-74`). It makes a Panama downcall `oxide_generate_chunk` (`OxideNative.java:270-275`, `ffi/lib.rs:247-293`).
3. Rust: `OxideGenerator::generate_chunk`, `terrain()`, then `oxide_chunkgen::generate_chunk`, then `write_out` (`handle.rs:341-395`).
4. Java: `toArray` copies blocks and biomes. The hook re-reads both palettes as `String[]` and calls
   `target.setBlocks` / `setBiomes` (`OxideChunkHook.java:81-117`).

Not serialised anywhere: the only shared locks are read-only palette `RwLock`s.

**Candidates, in order of share:**

| # | Candidate | Evidence | Share of the 6x |
|---|---|---|---|
| 1 | Vanilla Java stages: features, lighting, structure starts, heightmaps, saving, Chunky/scheduler | The PLAN.md profile: 25% of CPU in JIT Java, 17% libc. Decoration is 37-50% of offline pregen. | ~4x (~55-60 ms of ~70) |
| 2 | Vanilla noise still run in Java for structure placement | The Vertex path hooks neither `getBaseHeight` nor the biome source (`handle.rs:456-459`); the Bukkit path does | part of 1; a few ms? |
| 3 | **Vanilla NOISE fill may still run** after Oxide's hook | `NOISE` is not in the claimed stage set (`OxideVertexModule.java:52`). If Vertex does not skip `fillFromNoise`, Java regenerates terrain that matches Oxide's by design, so nobody would notice | **unverified; tens of ms if true** |
| 4 | FFI-only Rust work | `carvers_at` runs an uncached `ClimateSample::sample` (six full router programs, `caches: None`) plus `tree.nearest` and a Vec clone, for each of the 289 source chunks per chunk (`handle.rs:367-381`). `write_out` does two 196 KB `fill(0)`s, `to_string` per palette entry, and a `RwLock` plus SipHash lookup per entry; ~150-250 small allocations per chunk. The live profile puts Oxide at ~1.3x the harness | ~2-5 ms per chunk, ~0.2-0.4x |
| 5 | Boundary copies and palette strings | ~400 KB memset/copy, ~130 Java strings and downcalls. The biome palette carries every datapack biome, ~65 strings, though a chunk uses 1-3 | under 1 ms, under 2%, unless Vertex's `setBlocks` parses state strings per chunk |
| 6 | Box contention | The java server on s5 used 1-6 cores during bench runs | could make either number unrepresentative |
| 7 | Different datapack and seed | Live uses the container datapack and seed 4489057056054590644; the harness uses `reference` and 12345 | unknown |

**What reading cannot settle, and the measurement that would:**
1. Does Vertex skip NOISE, and what does `ChunkTarget.setBlocks` cost? Read the Vertex source, or take
   JFR / async-profiler during Chunky and look for `fillFromNoise` / `NoiseChunk` frames.
2. The Java-side split: an async-profiler wall-plus-CPU profile of a Chunky run, grouped by stage
   (features, light engine, structure starts with `getBaseHeight`/`Climate.Sampler`, region files with zlib, GC).
3. Oxide's real cost behind the FFI: `crates/oxide-ffi/examples/bench_native.rs` at 8 threads on
   `reference` with seed 12345, against the harness at the same radius (isolates row 4); then on the live datapack and seed (row 7).
4. Whether 115 cps is CPU-bound: the server process's utime+stime per wall second during Chunky.
5. The JVM-only cap: Chunky with a hook that returns a constant chunk. PLAN.md's "~240 cps" has no
   derivation in the repo; this model puts it nearer 120-140.

Row 4 is datapath-adjacent: `carvers_at`'s per-source climate sampling could use chunk caches or a
per-source-chunk memo. That is in `oxide-ffi`, not this track's, so it is not acted on here.

### Auditor's next two jobs

**(1) Stack the small exact changes into one measurable change.** E32 (shared surface table), plus
`SurfaceSystem`'s compiled program built once per router and thread (a per-thread cache keyed
by `router.id()` inside oxide-chunkgen, with no API change), plus noise's scratch pool
(`docs/perf/wip/noise-12-scratch-pool.patch` on `origin/perf/noise`), applied in a scratch tree.
Expected ~5-8% at 8 threads by the profile. Hold it against two base builds with the CPU-seconds
method, 8 rounds, at 8 threads and at 1.
- Stack tree `/tmp/datapath-28`: noise-12 and E32 apply clean on aa8ffab. noise-12 touches
  `oxide-noise/src/compiled.rs` and `lib.rs`; it is applied in the scratch tree only, and lands via the noise track.
- Sub-agent `datapath-29`: the per-thread cached `SurfaceSystem` program. The cached form must
  own its data or hold indices, since `CompiledCondition<'a>` borrows `&'a NormalNoise` and
  `results` borrow `&'a BlockState`. No `unsafe`.
- Bases: `datapath` and `datapath-h2`, two builds of the bd442e7 code.

**(2) Read-only sub-agent:** why the live server reaches ~115 cps where the harness reaches ~700 on the
same terrain code: the FFI chunk path, per-chunk JNI conversion and copying, locks, thread count,
the Vertex stages. Findings go in this log, for the user to assign.

### E33: `SurfaceSystem`'s compiled program built once per router and thread (part of the stack)

Sub-agent experiment in `/tmp/datapath-29`, `surface.rs` only; `lib.rs` and the public API are unchanged.
- **What is cached:** an owned `SurfaceProgram` holding the folded `program`, `below_programs`,
  `below_band_of_y`, owned result `BlockState`s, owned biome sets, noise ids, `memo_slots` and the
  `vertical_gradient` factories. `CompiledCondition` / `Op` lose their lifetime: `Noise2d` and
  `Noise3d` hold a slot into `noise_ids`, which `SurfaceSystem::new` binds once per chunk to the
  router's `NormalNoise` (one `router.noise(id)` per distinct noise).
- **Where:** a `thread_local!` `Vec<Arc<SurfaceProgram>>` of up to 4 entries, oldest dropped first. `Arc`
  keeps `SurfaceSystem` `Send`. No `unsafe`.
- **Key:** `router.id()`, `noise.min_y`, `noise.height`, and a field-by-field comparison of the whole
  `surface_rule` against an owned copy (`rule_eq` / `condition_eq` / `anchor_eq`, thresholds by bits).
  The router id alone is not enough: the router does not store `surface_rule`, and a caller can change
  settings in place under the same router. Every match arm lists every field, and the fallback arms list every
  variant, so a new datapack field fails to compile until it is compared. `biome_temperatures` is not
  read in `new`.
- **Exactness:** conditions, operands, evaluation order and memo numbering are unchanged. Only where the data
  lives changes.
- New test `program_cache_never_serves_another_surface_rule`: one router, the rule swapped in place
  between chunks. Tests: chunkgen 193. The workspace builds (harness, ffi, pregen). Digest `a977961a58122047`.
- The diff is at `docs/perf/parked-E33-surface-program-per-thread.diff` until the stack is decided.

**The stack (`datapath-28`):** E32 + E33 + noise-12, on aa8ffab. It is being tested, then held with the
CPU-seconds method against two base builds, 8 rounds at 8 threads, then 8 at 1.
