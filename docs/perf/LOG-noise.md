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
