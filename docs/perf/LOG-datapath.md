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
