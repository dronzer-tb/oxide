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
