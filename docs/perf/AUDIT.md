# Audit log

Newest pass last. Digest gate at radius 16: `a977961a58122047`, 8 threads.

## Pass 1, 2026-11-15 02:30 UTC

| Track | Head | Best clean number (8 threads, r16) | Digest |
|---|---|---|---|
| noise | 945cbde | 163.8 cps, 48.85 ms core (single run; own baseline 142.7) | ok |
| datapath | 5c1bae1 | 282.4 cps, 28.33 ms core (repeated: 280.5, 282.4; own baseline 138.2) | ok |
| pipeline | f5d0545 | none: no bench row, no log, no track commit | - |

Against the gates: M1 (400 cps) not reached on any branch. The two code tracks have not been
measured together; datapath's post-E2 profile puts 58% of a chunk in the fill loop's density
sampling, which is exactly what noise's whole-cell evaluation (4abb9a3) cuts, so the combined
figure should be well above either.

### Checked

- Every row in both tsv files carries the right digest, radius 16, 8 threads.
- No FMA, `mul_add`, SIMD, `unsafe` or fast-math in either diff. Both tracks stayed in their
  crates (noise: `oxide-noise`; datapath: `oxide-chunkgen`).
- Tests are logged per commit on datapath (chunkgen 189, biome 5). Noise ran `oxide-noise`
  before each commit but the dependent `oxide-chunkgen` suite only once.
- Datapath E4 (ore-vein cell skip) read line by line against `vein_state`: sound, since
  `edge_roundoff <= 0` and cells without a bound fall back to per-block evaluation.

### Problems

1. **Noise: gains under 10% not repeated.** Experiment 2 (152.0 -> 157.4, +3.6%) and 3b
   (157.4 -> 163.8, +4.1%) are one run each. Both are plausible (strictly less work) but the
   sizes are unestablished; only the cumulative +15% over three steps means anything.
2. **Datapath E4 gain unconfirmed**, as its own log says: 291.3 once, then 204.6 and 218.5
   under foreign load. Clean number stays 282.4 until a quiet repeat.
3. **Bench box contention.** Datapath saw `rustc` jobs from `noise-prof` running outside
   `~/oxide-perf/.lock` during its repeats. Profiling builds must take the lock. Baselines
   also differ by track (142.7, 138.2, PLAN says 160.5), so cross-track comparison of absolute
   numbers is good to about 15%.
4. **Pipeline has pushed nothing** in about 85 minutes: no baseline row, no log. It has
   uncommitted `oxide-anvil` work (bulk region writer, `compress.rs`) and a new
   `scripts/vps-cargo.sh`. Rule 3 (push after every experiment) is not being met.
5. **Strict-bound skips rely on lerp not overshooting corner values.** E4, like the existing
   air-cell skip in fill, compares corner min/max against a threshold with no margin. A
   trilinear lerp can exceed its corners by an ulp, so a corner maximum within ~1e-15 of 0.4
   could skip a block vanilla would test. Not observed, digest passes; a tiny margin
   (say 1e-9) on the comparison would make it provable at no cost.
6. **Noise is stopped** at its usage limit with a clean tree. No PR opened by either track.

### Across tracks

- Overlap: noise experiment 1 and its "next #1" (memoise `cell_bounds`, 24.5k calls) target
  the call count that datapath E1 already cut to 768 per chunk. Noise next #1 is moot once
  the branches meet; noise should go to #2 (corner-grid build) instead.
- Datapath is already merging noise's `sample_cell` into its fill loop (seen in its session).
  That is the right next step and the first combined measurement.

### Messages sent

None. All three track sessions had a "Switch model? (Opus 5.5)" confirmation dialog open;
typing into the pane would have answered it with Enter. Held for the next pass:
- noise: skip next #1; take the bench lock for `noise-prof` builds; repeat sub-10% gains.
- pipeline: push a baseline row and a log now, commit the anvil work in small steps.
- datapath: add a margin to the strict-bound skips; repeat E4 on a quiet box.

### Stopped reading at

noise 945cbde, datapath 5c1bae1, pipeline f5d0545.

## Pass 2, 2026-11-15 02:35 UTC

Run three minutes after pass 1, on request, to deliver the held messages. No new commits on
any branch; best clean numbers are unchanged (noise 163.8, datapath 282.4, pipeline none).

- All three sessions are now on Opus 5.5 and working. Noise has resumed and has a bench
  running; datapath is still waiting on the bench of its merge with noise's `sample_cell`;
  pipeline is writing `oxide-pregen/src/generate.rs`, still with nothing pushed.
- Messages sent (queued behind each session's running turn):
  - noise: skip next #1 (`cell_bounds` memo, already cut by datapath E1) and go to the
    corner-grid build; take the bench lock for `noise-prof` builds; repeat sub-10% gains and
    run the `oxide-chunkgen` tests before each commit.
  - datapath: add a margin to the strict-bound cell skips (E4 and the air-cell skip); repeat
    E4 on a quiet box; log the merge with `sample_cell` as the first combined number.
  - pipeline: run the baseline, start the log, commit and push the anvil work in small steps.
- To verify next pass: each track acted on its message; pipeline has a tsv row and a log.

Stopped reading at: noise 945cbde, datapath 5c1bae1, pipeline f5d0545.

## Pass 3, 2026-11-15 02:52-03:04 UTC

| Track | Head | Best clean number (8 threads, r16) | Digest |
|---|---|---|---|
| noise | bc60b65 | 185 cps, 43.2 ms core (4abb9a3 repeated on the quiet box: 189.1, 181.4) | ok |
| datapath | bea73ad | 282.4 cps, 28.33 ms core (unchanged; E5 runs 309.3/277.7/258.1 and 304.6/258.4 are too spread to move it) | ok |
| pipeline | 813333c | terrain baseline only (98.3, 137.3 on a loaded box); no pregen row pushed yet | ok |
| **noise + datapath** | scratch merge 8c12604 (bea73ad + c5dc61f) | **401.0, 421.3 cps; 19.95, 18.99 ms core** | ok |

**First combined number: about 411 cps, M1 (400) reached** on an unpushed scratch merge
benched by the auditor (`bench-audit-combined.tsv`). The merge is textually clean (the tracks
touch disjoint crates) and the digest holds. This is before datapath's fill loop calls
`sample_cell` directly. The merge is not on any branch: datapath must push its own.

### Changes read

- noise c5dc61f: experiment 4 (`cell_bounds` memo) benched 168.9/156.6 and reverted. bc60b65:
  took the new bench script and repeated its earlier steps. Log now commits to the lock,
  to repeats, and to running chunkgen tests.
- datapath cb83ffb (E5, `oxide-core` paletted container: no index array for single-value
  sections, linear search for small palettes). Inside its crates, tests logged (core 20,
  anvil 6, chunkgen 189), no `unsafe`. Gain not established; kept as less memory. fe61f27:
  margin on the cell-bound skips, as asked in pass 2. bea73ad: new bench script.
- pipeline: six commits since pass 1. Bulk region writer and compression in `oxide-anvil`,
  real generation with per-stage timings in `oxide-pregen`, `vps-cargo.sh`,
  `bench-pregen-vps.sh`, baseline and log. All inside its crates and `scripts/`. Its scripts
  already use shared lock for builds and tests, exclusive for the timed run.

### Bench box

Both Minecraft servers are stopped (user); all 8 cores are ours. The chunkgen test binary
seen at ~6 cores at 02:58 was datapath's, by then under `flock -s` with `nice -n 10`. All
three tracks now carry the shared-lock `bench-vps.sh` from `feat/native-decoration`.

### Sub-agents (user instruction: 2 or 3 Opus sub-agents per track, plus OmniRoute workers)

`/srv/oxide-lab/agents/tree.json` at 03:03: noise 1, datapath 0, pipeline 0. Not chased this
pass: the user's instruction and two auditor messages were still queued in each session.
Chase next pass any track still at 0.

### Messages sent (two rounds this pass, on the user's instructions)

- all: take the new bench script; every build, test and profile on the box under
  `flock -s ~/oxide-perf/.lock` with `nice -n 10`; servers are stopped; GitHub Actions
  pre-compilation allowed but shared-lock builds preferred.
- noise: go on with the corner-grid build.
- datapath: merge `origin/perf/noise` first and bench it twice; re-reference on the quiet box.
- pipeline: new job from the user: opt-in fat-LTO, `codegen-units = 1` profile with a switch
  in `bench-vps.sh`, then PGO on the harness run; digest kept, A/B on the same commit, two
  runs a side, build time logged.

### To verify next pass

Datapath pushed the merge and its numbers agree with ~411; pipeline has LTO/PGO A/B rows and
a pregen row; sub-agent counts; noise's sub-10% rule on new experiments.

Stopped reading at: noise bc60b65, datapath bea73ad, pipeline 813333c.

## Pass 4, 2026-11-15 03:30 UTC

| Track | Head | Best clean number (8 threads, r16) | Digest |
|---|---|---|---|
| noise | bc60b65 (no new commit) | about 185 cps (189.1, 181.4) | ok |
| datapath | bea73ad (no new commit) | 282.4 cps | ok |
| pipeline | c36f728 | terrain: baseline only. Whole pregen path: 69.7 cps on 8 threads (one quiet run; 52.6 on a busy box), 24.6 cps on 3 | terrain ok; pregen digest `df701ee858e2bdd3` |
| noise + datapath | auditor scratch merge | 401.0, 421.3 cps (pass 3; still not on any branch) | ok |

### Main finding: the bench lock queue has stalled the two code tracks

At 03:30 the bench box had 11 timed runs waiting on `flock -x`, the oldest for 26 minutes,
with load average falling to 3. Noise and datapath have pushed nothing for 27 minutes; their
sub-agents are waiting for BENCH lines (one noise sub-agent is past 100k tokens polling).
There are 13 build directories on the box (`noise-1..3`, `datapath-1..4`, `pipeline-1..2`
and the mains). Two causes:

1. `flock` has no fairness. Shared-lock builds keep arriving, so exclusive waiters starve
   until all builds happen to be finished.
2. The exclusive hold covers the whole harness process: a 3 to 7 s timed section plus the
   ~25 s single-threaded digest pass. Eleven waiters is about six minutes of lock time even
   with no builds.

Fixes proposed to pipeline (owner of `scripts/`): a gate lock so builds queue behind a
waiting timed run, and taking the exclusive lock around generation only (or a parallel
digest). The harness is owned by no track; the change has to be small and additive.

### Changes read (pipeline, 813333c..c36f728)

- `[profile.fatbench]` in the workspace `Cargo.toml` (inherits release, fat LTO, one codegen
  unit), selected by `OXIDE_BENCH_PROFILE=fatbench`; additive, default profile untouched.
  **No A/B rows yet**, and PGO not started.
- Pregen path measured: generate 61%, decorate 37%, serialise + compress + write about 2%.
  Its lever outside terrain is decoration, in `oxide-chunkgen/src/decorator/`, which **no
  track owns** (PLAN excludes it from datapath). Needs a decision from the user.
- zlib backend micro-bench: zlib-rs 2.4x faster at level 6, at most ~1.5% of the path.
- Native light scoped, not started; load-test steps written for the user. Nothing touched
  the server.
- `bench-vps.sh` now excludes `/out` from `rsync --delete`.
- Pregen rows share `bench-pipeline.tsv` with terrain rows, marked `pregen=1`, with their own
  digest. Anything reading that file must not compare the two digests.

### Sub-agents

`tree.json`: noise 8, datapath 5, pipeline 3 spawned in total; 3, 3 and 2 live in the panes.
All tracks comply. The cost is the queue above.

### OmniRoute

User instruction received during this pass: no more `omni-agent.sh` / `omni-ask.sh`, Opus
sub-agents only. The auditor has never used either and has nothing running. No OmniRoute
worker process is running on the agent host. Not relayed (one message per track already sent
this pass); check next pass that no track calls it. PLAN rules 6a/6b and the last line of
AUDIT-ROLE.md still describe it and are now out of date.

### Messages sent

- noise, datapath: the queue is the bottleneck; run the bench as one blocking command instead
  of polling; bench only tested changes, one A/B pair per experiment; the combined number is
  401/421. Datapath: push the noise merge now and base sub-agents on it. Noise: judge new work
  on the merged tree.
- pipeline: fix lock fairness and the length of the exclusive hold, ahead of PGO; log the
  fatbench A/B rows.

### To verify next pass

Datapath's merge is pushed and agrees with ~411; queue wait times; fatbench and PGO A/B rows;
no OmniRoute calls; noise and datapath commit again.

Stopped reading at: noise bc60b65, datapath bea73ad, pipeline c36f728.

### Standing rules added by the user after pass 4 (03:40 UTC)

- **Ownership change:** `crates/oxide-chunkgen/src/decorator/`, `decorate.rs` and `region.rs`
  now belong to **pipeline**. Datapath keeps the rest of `oxide-chunkgen`. Check both diffs
  against this boundary every pass. At the time of the change neither branch had touched
  those three paths, and pipeline had touched nothing else in `oxide-chunkgen`.
- **Pipeline's gate** is the pregen digest `df701ee858e2bdd3` (radius 16, seed 12345, blake3
  over every chunk's uncompressed NBT), in addition to the terrain digest for anything that
  reaches the harness. Any pregen row with another digest is a changed world.
- OmniRoute is off for everyone; Opus sub-agents only.

## Pass 5, 2026-11-15 04:18-04:30 UTC (lock audit asked by the user, then the regular pass)

| Track | Head | Best number (8 threads, r16) | Digest |
|---|---|---|---|
| noise | 1bcc15b | merged-tree A/B only: 443.5 with experiment 7 against 385.7 (one pair) | ok |
| datapath | 3683496 (contains noise up to 4abb9a3) | **~479 cps, 16.7 ms core** (its interleaved A/B, three pairs: 492.3, 475.3, 470.3) | ok |
| pipeline | 8ea2c1c | terrain baseline 125.0, 135.4 (unchanged code, loaded box); pregen 69.7; no fatbench or PGO rows pushed | ok / `df701ee858e2bdd3` |

Auditor re-measurement of datapath's head, noise's head and a fresh merge (5a4d1d4) was still
queued behind builds when this was written; results go in the next entry.

### What ran outside the lock

- **pipeline `oxide-pregen`**, seen by datapath at 144% CPU with no lock right after its
  226/264 runs (logged by datapath as invalid).
- **pipeline's `--bench-lock` harness** (68df22e): datapack load before taking `.lock` and
  an 8-thread digest after dropping it. This followed the auditor's own pass-4 suggestion,
  which was wrong on a box where the other tracks do not pass pipeline's gate. Removed by
  pipeline in 8ea2c1c, which also puts every script back on plain `flock -s` / `flock -x`.
- datapath's profiling build was under `flock -s` but at nice 0.
- Not leaks: PGO training, rustc and cargo test were all under `flock -s` with nice 10 in
  four samples. High load from shared-lock builds is by design.

### New problem: the box is saturated by builds

04:30: load average 21 on 8 cores, 16 lock holders or waiters, 20 build directories. Every
job is locked and niced, so timed runs are clean, but a build that took 3.5 minutes now takes
over 9 and each timed run waits for all of them. Sub-agents sit an hour "waiting on bench"
(noise: two at 1h02m, over 100k tokens each). Suggested to the user: cap each track at two
bench directories in flight, or share one `CARGO_TARGET_DIR` per track.

### Changes read

- noise 6dfa05a: doubled permutation table in `ImprovedNoise`. Index arithmetic checked by
  hand (all indices at most 511, same entry as the masked form); float code untouched;
  bit-identity test over 64 seeds. Experiments 5 and 6 rejected, logged. In flight:
  cross-chunk corner-column cache and a shared `preliminary_surface_level` memo; both need a
  close exactness read when they land.
- datapath 0e5afee (merge of noise), E8-E11 (palette-index ore veins and carvers, surface
  rules as a flat program), E4 A/B showing no gain (skip kept). All in `oxide-chunkgen`
  outside the decoration paths; tests logged; no `unsafe`, no float changes.
- Boundary: datapath has not touched `decorator/`, `decorate.rs`, `region.rs`; pipeline has
  not touched the rest of `oxide-chunkgen`. Pipeline's harness edit was additive and is now
  reverted.

### Limits

`tree.json`: noise 98% of its five-hour limit, datapath 87%, pipeline 40%. The auditor shares
noise's account (96%). Noise will stop soon; its handover must be in its log.

### Messages sent

- pipeline: lock every pregen and harness run; stop `--bench-lock`; one locking scheme
  (done in 8ea2c1c).
- datapath: nice 10 on profiling builds and runs.
- noise: none.

Stopped reading at: noise 1bcc15b, datapath 3683496, pipeline 8ea2c1c.

### Auditor re-measurement after pass 5 (runs between 04:35 and 05:20 UTC, all digest ok)

| Tree | Run 1 | Run 2 |
|---|---|---|
| datapath 6b8562f | 473.0 cps, 16.91 ms | 476.9 cps, 16.78 ms |
| noise + datapath merge 5a4d1d4 (adds noise 6dfa05a) | 414.9 cps, 19.28 ms | 495.0 cps, 16.16 ms |
| noise 1bcc15b alone | 173.3 cps | 163.2 cps |

Datapath's ~479 claim is confirmed (475 here). The fresh merge is not separable from it: its
two runs are 19% apart, so noise's experiment 7 gain on the merged tree is not established.
Noise alone is below its earlier 185. Stopped here: the auditor's usage limit was reached.
Scratch worktrees /tmp/audit-{combined,datapath,noise} are left in place.

## Passes 6 and 7, 2026-11-15 07:21-07:48 UTC (first passes after the auditor's limit reset)

| Track | Head read | Best number (r16, 8 threads unless said) | Digest |
|---|---|---|---|
| datapath | a29cef5 | **auditor: 620.3 and 695.4 cps (12.90, 11.50 ms core)**; its own A/B holds: ~614 | ok |
| datapath, **1 thread, radius 6** | a29cef5 | **auditor: 10.82 and 12.07 ms per chunk** (92.4, 82.9 cps) | r6 digest `ffd3a490ec8fb8b0` both runs |
| noise | fast-forwarded onto datapath's head | no number of its own since the handover at f2b239a | - |
| pipeline | 4aca951 | pregen 83.8, 76.2 cps at c585498 (from 69.7); terrain unchanged (~137); PGO 177.4 once | terrain ok; pregen `df701ee858e2bdd3` on every row |

**Against the goal:** Pumpkin's terrain is 17.4 ms per chunk on one thread. Datapath's head,
which carries both tracks' kept work, measures about 11.4 ms on one thread on the same box
(started at 43.8), byte exact at radius 16. That is the terrain-only target met, pending the
user's own check; radius 6 has no reference digest on record, only the radius-16 gate on the
same commit. The full chunk (Pumpkin 28.3 ms with features and lighting) is not comparable yet:
pregen at ~80 cps on 8 threads is about 100 ms of core time per chunk, on old terrain code and
without lighting. Plan gates: M1 passed, M2 (1,000 cps) not.

### Changes read

- datapath 3683496..a29cef5: flat R-tree biome search, aquifer neighbour columns and centre
  memo, surface rule folding by y band, E18 per-thread cross-chunk memo of
  `preliminary_surface_level` keyed by a new `NoiseRouterEvaluator::id()` (17 additive lines in
  `oxide-noise/router.rs`, outside its crates, logged). E18 read in full: pages are cleared
  on a router change, values are a pure function of column and router, the memo is taken
  out of the thread-local for the aquifer's life. Sound. Rejected experiments are logged with
  A/B tables. Second merge of noise (6dfa05a) measured neutral. No `unsafe`, no float
  reordering. 18bb0f9 backfills `bench-datapath.tsv` as asked.
- pipeline 8ea2c1c..4aca951: fat LTO A/B (126.5-145.5 against 136.0-140.0: no gain, settled);
  `bench-pgo-vps.sh` (training run under `flock -s`); zlib-rs backend; decoration work in
  `decorator/` and `region.rs` (ahash, sculk by reference, per-state fact cache, dense
  heightmap table); merge of `feat/native-decoration`. Edits outside its crates: one field
  type and the ahash dependency in `oxide-datapack`, logged.
- noise: handed over at f2b239a with two patches under `docs/perf/wip/`, resumed about
  07:15, then moved its branch onto datapath's head.
- Boundary: datapath clean of `decorator/`, `decorate.rs`, `region.rs`; pipeline clean of the
  rest of `oxide-chunkgen`.

### Problems

1. PGO's +29% is one run on old terrain code. Unrepeated, and not measured on the code that
   would ship.
2. Pregen is measured on a branch without datapath's terrain, so its stage split is stale.
3. Noise's in-flight `noise-5` duplicated datapath's E18.
4. The noise session had six identical supervisor nudges queued at its prompt.
5. No PR is open from datapath (its log says `gh` is missing on the agent host).

### Messages sent (pass 6; none in pass 7)

- noise: drop noise-5; rebase onto `perf/datapath`; take noise-4 (shared corner columns, the
  corner grid being 44.5% of generation) with two A/B pairs.
- datapath: backfill the tsv (done); leave the rest of `oxide-noise` to noise.
- pipeline: repeat PGO two runs a side, then PGO and pregen on a scratch merge with
  `perf/datapath`; keep the `oxide-datapack` edit additive.

Stopped reading at: noise 18bb0f9, datapath 18bb0f9, pipeline 2707402.

## Pass 8, 2026-11-15 08:27-09:26 UTC

| Track | Head read | Best number (r16) | Digest |
|---|---|---|---|
| noise | d2433fb (datapath's head + shared corner columns) | **auditor A/B: 687.8 cps on 8 threads (673.0, 669.4, 721.8, 686.9); 10.48 ms per chunk on 1 thread (10.37, 10.58)** | ok, all runs |
| datapath | 9559df0 (code 0e3cf61) | base in the same hold: 586.3 cps (491.3, 649.3, 613.2, 591.5); 11.74 ms on 1 thread (11.41, 12.07) | ok |
| pipeline | 7252db9 | pregen 87.0 cps (best run); merge-with-datapath and PGO repeats queued, no rows yet | `df701ee858e2bdd3` |

**noise-4 confirmed.** One exclusive hold, base and patched alternating: patched ahead in all
four 8-thread rounds (+17% on the means) and both 1-thread rounds (-11% core time). The
track's own two pairs (+20%, +1%) did not establish it; this does. Terrain is now about
10.5 ms per chunk on one thread against Pumpkin's 17.4, byte exact at radius 16.

### Changes read

- noise d2433fb: set-associative cache of interpolation corner columns on the router, shared
  across chunks and threads (64 mutex shards, about 13 MB), full `(slot, x, z)` key per
  entry, geometry checked before use, bit-identity test with expected hit counts. Only
  `oxide-noise`. Exact if a corner value is a pure function of position, which holds for the
  caches it reads. noise-5 dropped unbenched as asked.
- datapath 161812e..9559df0: E21 (aquifer row memo) and E22 (carver LCG jump) rejected after
  four-round A/Bs on 8 threads and on 1 thread; diffs kept under `docs/perf/rejected-*`.
  Handover written; the session then sat idle at 59% of its limit.
- pipeline: dense `written_top` and heightmap table in `region.rs` (kept, small); overlay
  skip rejected; `oxide-datapack` edit stated in its log; scratch merge with datapath built
  and tested.

### Problems

1. Datapath's "load outside the lock" flags rest on the load average at the start of an
   exclusive hold, which is always high because the hold begins when the builds it waited
   for end. No unlocked process was named, and none was seen in the auditor's samples.
2. Measurement spread is now the limit for datapath-sized changes: 15-25% on 8 threads, 8%
   on 1 thread for one binary. A queue wait of 30-50 minutes for an A/B hold is normal.
3. Datapath went idle after its handover with limit to spare.
4. Pipeline's prompt held unsent text, so the auditor sent it nothing this pass.

### Messages sent

- datapath: numbers confirmed; load average is not evidence, name a process; carry on with
  its next-three (1-thread five-round A/B, retry E21/E22, column-wise surface); re-profile on
  noise-4 once confirmed.
- noise: noise-4 needed a proper A/B (being run); use four or more alternating rounds and
  1 thread from now on; log cache hits and misses and rule out lock contention.
- pipeline: none.

To do next pass: tell noise and datapath that noise-4 is confirmed and datapath should merge
it; check pipeline's merge and PGO rows.

Stopped reading at: noise e6ecc65, datapath ef5597e, pipeline 7252db9.

## Pass 9, 2026-11-15 09:33 UTC

Best clean numbers are those of pass 8: terrain on `perf/noise` d2433fb, 687.8 cps on
8 threads and 10.48 ms per chunk on 1 thread (auditor A/B), digest ok. Pipeline pregen 87.0 cps,
digest `df701ee858e2bdd3`. No kept code change on any branch since.

### Changes read (logs only; no crate changed)

- noise c1ed443, e6ecc65: experiment 10 (64 inline registers) behind in both pairs;
  experiment 11 (AVX2 `ImprovedNoise`, four octave samples per call, add/sub/mul/div only, no
  FMA, six bit-identity tests) **rejected** at about -10%: Zen 1 splits 256-bit operations and
  the gather cost outweighs the float work. Patch kept under `docs/perf/wip/`. The track has
  adopted the alternating-hold method. Its tsv rows match the log.
- datapath 483a359, 95e7e60, ef5597e: load-average flag retracted; E21 and E22 re-run under
  a five-round 1-thread A/B and still not established; E23 (column-wise reuse of surface
  rule outcomes, with a debug-build equality assert) rejected at -1%. Diffs kept. Its
  next-three list is exhausted except for merging noise-4, which waited on the auditor.
- pipeline: nothing pushed since 08:19. A PGO run had been in the queue for 28 minutes.

### Observations

- Datapath was idle at its prompt, 71% of its five-hour limit used. Noise at 49%, pipeline 28%.
- Noise's and pipeline's prompts held unsubmitted supervisor nudges ("Continue the ...
  track"); the auditor's messages were appended to them and submitted.
- Bench box: load 2, two exclusive waiters behind one PGO build; all locked.
- The remaining headroom is in the noise crate's corner grid; datapath-sized changes are
  below what the box can resolve. M2 (1,000 cps) needs about 30% more.

### Messages sent

- all three: noise-4 confirmed, with the numbers.
- datapath: merge `origin/perf/noise`, re-profile on 1 thread, plan the next round.
- noise: keep it; log the cache's hit, miss, eviction and contention counts.
- pipeline: push the queued merge and PGO rows; base the next scratch merge on `perf/noise`.

Stopped reading at: noise e6ecc65, datapath ef5597e, pipeline 7252db9.

## Pass 10, 2026-11-15 10:33 UTC

| Track | Head read | Best number | Digest |
|---|---|---|---|
| noise | 0c83b38 | 1 thread, r16: **9.77 ms per chunk** with experiment 12 against 10.43 (its hold, 5 of 5 rounds) | ok, all ten |
| datapath | 741d96f (code 20d218d, has noise-4, not experiment 12) | 1 thread: **9.97 ms** with E21 against 10.40 (6 of 6 rounds); 8 threads: 697.0 cps (one row) | ok, all twelve |
| pipeline | 59928b8 | pregen on merged terrain: 184.3, 171.5 (with datapath), 194.2, 183.4 (with noise-4); own branch 87.0 | `df701ee858e2bdd3` on all |

The two 1-thread gains are on different branches and not yet measured together; if they
stack, terrain is near 9.4 ms per chunk on one thread (Pumpkin 17.4, start 43.8). Not
re-measured by the auditor this pass: both holds use the rotating multi-round method and the
bench box disk is nearly full.

### Changes read

- noise ff02449 (experiment 12, gated add): `add(c, mul(gradient, body))` skips `body` where
  the gradient is exactly zero. Exact because `c` is required nonzero and finite and `body` is
  proven finite by a conservative magnitude analysis, so `c + (+/-0) = c` bit for bit;
  `invert` and anything unbounded keep the plain lowering. Tests cover every y and all operand
  orders. No float operation is reordered where the body does run. 8a85270: cache counters;
  contention 0.1% of acquisitions, hit rate 0.49-0.54 of a 0.5625 ceiling. Lazy noodle grids
  not pursued (under 1%). Corner grid now 27.6% of generation by its profile. In flight:
  SIMD inside one `ImprovedNoise` sample.
- datapath 0300e5c (merge of noise-4; 697.0 cps), 20d218d (E21 aquifer row memo, retried on
  the new base and kept), E25 (column-major surface scan) rejected at -0.4%. Only
  `oxide-chunkgen` outside the decoration paths.
- pipeline: merge benches and PGO repeats as asked; `bench-pgo-vps.sh` now deletes its build
  dirs. No crate change.

### Findings

1. **PGO shows no gain on the merged terrain**: 593.3 and 684.9 against release 632.2 and
   657.6. On the old terrain it was about +23% (167.7, 159.4 against 135.9, 130.5), which no
   longer matters. Fat LTO was already settled as no gain.
2. **Decoration is now more than half of pregen** (about 3.0 s against 2.4-2.9 s for
   terrain at radius 16). Pregen at ~189 cps is about 42 ms of core time per chunk, without
   lighting; Pumpkin's full chunk is 28.3 ms.
3. **Bench box disk at 96%.** Pipeline's first two PGO builds failed with ENOSPC. About 50
   scratch dirs under `~/oxide-perf` hold 65 GB; the rest of the 182 GB used is not ours. The
   auditor removed its own five dirs (12 GB free after).
4. Noise's kept experiments since d2433fb have no tsv rows, so the progress page misses them.
5. Noise repeated the load-average inference datapath retracted.
6. Datapath is at 85% of its five-hour limit.

### Messages sent

- all: delete your own finished scratch dirs on the box, and each new one once logged.
- noise: experiment 12 reads exact; append tsv rows for kept heads; load average is not evidence.
- datapath: merge `perf/noise` again for experiment 12; write the handover before the limit.
- pipeline: close PGO unless a 1-thread alternating hold shows a gain; decoration is the lever,
  go ahead with interned block states.

Stopped reading at: noise 0c83b38, datapath 741d96f, pipeline 59928b8.

## Pass 11, 11:34 UTC

Read only; no auditor re-measurement this pass (auditor account at 87% of its five-hour limit).

- **noise 8f3738f** (docs only since 0c83b38). Experiment 13 (SIMD within one sample) rejected:
  9.72 against 9.52 ms, 1 thread, 5 rounds. Three vectorisations of `ImprovedNoise` have now
  lost on Zen 1. tsv rows backfilled for d2433fb and ff02449 (timestamps approximate, stated).
  Best: 9.77 ms on 1 thread at ff02449 (its own hold). Limit 80%.
- **datapath 4b152db** (code 0823fd8 = 20d218d + noise merge with experiment 12; only
  `oxide-noise` files changed, all from the merge). Tests passed. 8 threads: 701.5 cps at
  741d96f, one row. Handover written. Four-way 1-thread hold: head 10.41, noise merge 9.97,
  E27 biomes 9.76, E22 carvers 9.97 ms; datapath rightly does not trust it, since E27 gains
  more than all of `fill_biomes` costs. Suspected cause: separately built binaries of one
  source land about 4% apart. A two-head-build hold is running. If confirmed, every A/B under
  5% this session (E21 at 4.2% included) needs repeating against two base builds.
  E22 and E27 are unmerged; E27's y-independence check parses `dump_program` text and should
  become a predicate in `oxide-noise`. Not reviewed for exactness yet (not on the branch).
  Limit reset (1%).
- **pipeline 5b67e37**. PGO, alternating in one hold on the noise merge a1f3a32: 1 thread
  9.23 against 10.50 ms (-12.1%, 5 of 5); 8 threads 723.7 against 682.4 cps (+6.1%, 3 of 5).
  Closed as an opt-in script. My pass-10 reading (no gain) came from separate 8-thread holds
  and was wrong for 1 thread. No decoration result pushed; interning sub-agent has run 2 h.
- Box: disk 70% (60 GB free), 13 dirs, 16 GB. All running jobs seen under the lock. No
  OmniRoute calls since 03:50. Sub-agents: noise 13, datapath 18, pipeline 4 in total.
- Digests: every row `a977961a58122047`. No ownership breach.

Messages: noise (4% build floor, add a second base build; provide the y-independence predicate
for E27; write handover), pipeline (PGO accepted; push a decoration profile, second sub-agent,
push the interning result). None to datapath.

Open for the user: PGO as a release step (12% on 1 thread, about 7 min of build).

Stopped reading at: noise 8f3738f, datapath 4b152db, pipeline 5b67e37

## Pass 12, 12:34 UTC

Read only; no auditor re-measurement this pass.

- **noise 65ba9fc**. b1e4a5c adds `NoiseRouterEvaluator::is_y_independent(slot)` (an API for
  datapath's E27, with a test over 8 shapes; no speed change). Experiment 14 (column-batched
  corner grid with lattice-hash reuse) is inconclusive: 10.667 against 10.963 and 11.208 s per
  run on 1 thread, under the 5% bar; a 7-round hold with two bases is running. Experiment 13
  relabelled inconclusive. Handover written. Best still 9.77 ms at ff02449. Limit reset (7%).
- **datapath b55a3b2** (code 841a929). Two-head-build hold: the two builds of one source differ
  by 2.3% (10.49 and 10.25 ms), so the 4% build floor suspected in pass 11 is not confirmed.
  Kept on that hold, 1 thread, 4 rounds: E27 per-column climate with hinted biome search, 9.56 ms
  (ahead in 8 of 8 comparisons); E22 carver LCG jump-ahead and ellipsoid tables, 9.85 ms (7 of
  8); noise merge 9.65 ms. The combined head has no hold of its own yet. 8 threads: 701.8 cps,
  one row. Stayed in `oxide-biome` and `oxide-chunkgen` (non-decoration files).
  Exactness read by the auditor, holds:
  - hinted search: the bound is the hint leaf's distance plus one, so only subtrees that could
    never win are pruned and the first minimum leaf in traversal order is unchanged;
  - column climate: the five slots are sampled once per quart column only where the program is
    proven y-independent (text parser, fails closed; to be replaced by the noise predicate);
  - carver: `WalkRandom` is the same LCG with precomputed jumps (tested against `LegacyRandom`
    bit for bit); the reach skip uses a bound that is never below `carve_ellipsoid`'s own
    `16 + 2r`, so it skips only calls that returned at once; `zd`/`yd` tables hold the same
    expressions, and the sum order is unchanged.
  Idle from 11:53 to 12:36: four supervisor nudges sat typed but unsubmitted in its prompt.
- **pipeline 0c767f6** (docs only; decoration work is still in sub-agent worktrees).
  1-thread decoration profile: 17.5 ms per finished chunk; sculk patch 24.4%, the five stone
  ores 28.1%. Interned region states: neutral alone (3439 against 3504 ms), needs the pregen
  switch, under test. Sculk read memo: sculk CPU 3678 against 5173 ms in one profile each, not
  yet held. Pregen digest `df701ee858e2bdd3` on every row. Limit 4%.
- Box: disk 68%, 12 dirs; every job seen was under the shared lock at nice 10. No OmniRoute
  calls. Sub-agents in total: noise 13, datapath 18, pipeline 5.

Messages: datapath (woken; merge the noise predicate and drop the text parser; hold the combined
head; re-profile with two sub-agents), pipeline (decide decoration changes on 1-thread holds
and land them; next the ore `height` reads), noise (predicate accepted; decide experiment 14 on
the 7-round hold; two sub-agents on APIs for datapath's surface and aquifer).

Open for the user: the supervisor's "Continue the ... track" nudges are typed into the panes
but not submitted; PGO as a release step.

Stopped reading at: noise 65ba9fc, datapath b55a3b2, pipeline 0c767f6

## Pass 13, 13:34 UTC

Read only; no auditor re-measurement this pass.

- **noise b258649** (docs only). Two sub-agents on APIs for datapath: `noise-9` (aquifer noise
  share, one narrow API) and `noise-10` (exact 2D fast path in `ImprovedNoise`). No result from
  them or from the experiment 14 confirmation: all three waited 45-55 min on the bench lock.
  Best still 9.77 ms at ff02449. Limit 30%.
- **datapath 2127bd7** (code b1f77a6). Merged noise at 65ba9fc; `biome_grid.rs` now calls
  `is_y_independent` and the text parser is gone (-200 lines). Tests passed. Profile of the
  combined head: corner grid 34.8%, surface 19.4%, aquifer 11.9%, per-block density 8.4%,
  carvers 8.2%. E29 (preliminary surface per quart) parked: ceiling 1.3%. The hold of the
  combined head against 0823fd8 is still running, so there is no new number. Only owned files
  changed outside the merge. Limit 25%.
- **pipeline a1d75b1**. Interned region states plus the pregen switch landed (10d7220, 78de067,
  639be4e): decorate 16689 against 21702 ms on 1 thread at radius 16, ahead in 3 of 3, digest
  `df701ee858e2bdd3` on all nine. Sculk read memo rejected (22737 ms, slower). Changes are in
  `decorator/`, `region.rs` and `oxide-pregen` only. Its branch still has the old terrain
  (about 70 s of each 95 s run), so its 9-run hold held the exclusive lock about 15 min.
- Box: disk 72%, 22 dirs; two builds waited 10 and 20 min on the shared lock behind exclusive
  holds. Nothing ran outside the lock. No OmniRoute calls. Sub-agents in total: noise 15,
  datapath 20, pipeline 5.

Messages: pipeline (interning accepted; run decoration holds on a tree with merged terrain or
radius 8, holds under 5 min), noise (`is_quart_invariant` only if trivial; holds under 5 min).
None to datapath.

Stopped reading at: noise b258649, datapath 2127bd7, pipeline a1d75b1

## Pass 14, 14:35 UTC

Read only; no auditor re-measurement this pass.

- **noise 3362440** (code 01528de). Experiment 14 kept: corner columns built lane-batched with
  lattice hashes reused across lanes of one cell. 7-round 1-thread hold against two base
  builds: 9.15 against 9.77 and 10.01 ms (ahead of both in 6 of 7), digest right on all 21.
  Read by the auditor: the memo is keyed by the full cell `(i, j, k)` and holds only the eight
  permutation bytes; the float path (`lerp_corners`) keeps the same operands and order. Exact.
  Experiment 15 (2D fast path) inconclusive, not landed. `noise-9` (aquifer API) still in its
  hold. Only `oxide-noise` touched. Limit 47%.
- **datapath bf577ba** (docs only; code still b1f77a6). Combined head against two builds of
  0823fd8: 9.37 against 9.55 and 9.33 ms, no measurable gain, so the speed claims for E22 and
  E27 (pass 12) are withdrawn; both stay as exact and not slower. E30 (aquifer slab and column
  run) not landed: 9.44 against 9.68 and 9.47. Floor between two builds of one source: 2.2-2.3%.
  Track asked which of three directions to take and sat idle from 14:24 for the answer.
- **pipeline 1732883** (docs only). Hold-length rule adopted; decoration holds now run on a
  tree merged with datapath's terrain. Ore floor scan skip (`height_bound`) in test.
- Best now: about 9.2 ms per chunk on 1 thread (noise head; datapath head 9.4-9.5 without
  experiment 14). 8 threads has stayed near 700 cps since pass 9: about 11.4 ms of core time
  per chunk against 9.4 on 1 thread. M2 (1,000 cps) needs that gap closed as well.
- Box: disk 67%, 12 dirs, load 1.1, all jobs under the lock. No OmniRoute calls. Sub-agents
  in total: noise 15, datapath 20, pipeline 6.

Messages: datapath (merge noise 3362440; fused fill-plus-surface pass with a design sub-agent;
a second sub-agent on the 8-thread scaling loss; 7 rounds at radius 16, not radius 24). None
to noise or pipeline.

Stopped reading at: noise 3362440, datapath bf577ba, pipeline 1732883

### Note from the user, 15:05 UTC

- s5 was reserved for a live server test; bench builds were SIGSTOPped and resume at about
  15:55 UTC (16:55 CET, systemd timer `resume-bench`). **Rows timed between 15:00 and 15:55 UTC
  do not count.**
- Live Chunky with the datapath head: 85 cps at first, 115 cps with the box free and lz4 region
  files. With `native-decoration=true`: about 10 cps (pipeline has been told).

## Pass 15, 15:35 UTC

Read only. The box is still reserved (builds stopped, live server running); resume at 15:55 UTC.

- **Gate hole found by datapath.** The terrain digest `a977961a58122047` hashes
  `block.to_string()` per position (`oxide-harness/src/merkle.rs`). It does not see palette
  order, biomes or heightmaps. Datapath changed all three (E1, E12, E27 biomes; E1 heightmaps;
  E1, E2, E9-E11, E16, E22 palettes). Its reading review found no violation. Interim evidence:
  the pregen digest is over chunk NBT, and pipeline's merged trees (up to datapath 2127bd7)
  gave `df701ee858e2bdd3`. Interim rule sent: each kept terrain change also passes a pregen
  run on a tree merged with pipeline. Widening the harness digest is for the user to assign.
- **noise 33fc192** (code fd4bfe2). `is_quart_invariant(slot)` added for datapath's E29 (API,
  test, no speed change). Aquifer point memo (`noise-9`) rejected. Column-uniform x/z
  (`noise-11`) designed, not built. Best still 9.15 ms on 1 thread at 01528de; that commit has
  no 8-thread measurement. Limit 76%.
- **datapath a95c316** (code 0f15cef = b1f77a6 + noise 3362440). Fused fill-plus-surface pass
  stopped at design: ceiling 1-1.5%. One 8-thread row of 392.9 cps at 14:56 UTC, minutes before
  the reservation: not counted; the hold against b1f77a6 runs after the box is back.
- **pipeline d7d5ea8**. Kept, each on 1-thread holds on merged terrain, digest
  `df701ee858e2bdd3` on every row: ore floor scan skip 14617 against 15382 ms (ahead in 4 of 6
  pairs, 5%; thin); sculk 3x3x3 neighbourhood 16370 against 17971 ms (3 of 3). Files: `sculk.rs`,
  `ore.rs`, `region.rs`. Branch-head tests wait for the box.
  Live 10 cps with `native-decoration=true`, by reading: (1) the Vertex hook does not take the
  FEATURES stage, so vanilla features run on top of native ones (cost and a wrong world);
  (2) one finished chunk needs 25 terrain chunks and 9 feature runs unless cached, and the
  cache is 256 terrain chunks, FIFO. Fixes are in the plugin and `oxide-ffi`, owned by no track.
- Box: disk 68%; stopped builds are waiting as the user said. No OmniRoute calls. Sub-agents
  in total: noise 15, datapath 22, pipeline 6.

Messages: datapath (interim pregen gate; 392.9 row suspect, hold first), noise (01528de needs
an 8-thread hold, datapath runs it; handover), pipeline (plugin and oxide-ffi are not its
files; log the merged datapath commit in each row).

Open for the user: who widens the harness digest and at which commit the new value is set;
who fixes the Vertex stage registration and the decoration cache in `oxide-ffi`.

Stopped reading at: noise 33fc192, datapath a95c316, pipeline d7d5ea8

## Pass 16, 16:35 UTC

Read only; no auditor re-measurement this pass.

- **Gate hole closed empirically for datapath up to 0f15cef.** Throwaway harness at the
  session start (3fd5cf6) and the head: block and biome palettes in order plus packed longs
  `04da69623d0ddc8f` at both; six heightmaps `16f8026ca33ac948` at both; block digest
  `a977961a58122047` at both. Pregen NBT digest on the head merged with pipeline d7d5ea8:
  `df701ee858e2bdd3`. The harness itself is still unchanged; widening it waits for the user.
- **noise a297e03** (docs only): handover at 76%. Experiment 17 (`noise-11`) in its hold. Best
  still 9.15 ms on 1 thread at 01528de. Limit 86%.
- **datapath d137014** (docs only; code 0f15cef). One 8-thread run of the head under the shared
  lock gave 712.0 cps, so the 392.9 row looks disturbed; the exclusive 8-thread hold from the
  scaling sub-agent (running 1 h 48 min) has not reported. Limit 3% (reset).
- **pipeline 36688fe** (docs only): table of which datapath commit each merged tree carried.
  Its counting test for the decoration cache is queued on the box. Limit 28%.
- Box: the live Java server is still running at 16:35 UTC (18-30% of one core, 7 GB), 40 min
  past the stated end of the reservation. Holds taken now are disturbed by it, 8-thread ones
  most. Disk 70%. No OmniRoute calls. Sub-agents in total: noise 15, datapath 22, pipeline 6.
- Datapath's pane had a `/usage` screen open, which blocked input; closed with Escape.

Messages: datapath (checks accepted; log the Java server at each hold; get the 8-thread
result; then E29 plus E30 together). None to noise (near its limit) or pipeline.

Stopped reading at: noise a297e03, datapath d137014, pipeline 36688fe

## Pass 17, 17:35 UTC

Read only; no auditor re-measurement this pass.

- **noise af380e3** (docs and a wip patch). Experiment 17 (column-uniform x/z) inconclusive, not
  landed: 1 thread 100.1 against 101.4 and 97.5 cps; 8 threads 687.7 against 601.9 and 596.9,
  not trusted (each base has one 455-cps round). Best still 9.15 ms on 1 thread at 01528de.
- **datapath 00b456d** (code bd442e7 = 0f15cef + noise fd4bfe2). 8-thread exclusive hold, 6
  rounds: 0f15cef 750.5 cps (627-834) against b1f77a6 728.5 (707-747). The 392.9 row was the
  Java server (362% CPU at the time) and is struck. Scaling findings: fill costs 5.72 ms per
  chunk on 1 thread and 6.46-6.50 on 8 (+13%); about 11.4k allocations and 7 MB per chunk,
  most in `run_cell`/`run_lanes` scratch (noise's crate); E18's per-thread memo repeats work
  across threads (1.37% of CPU on 1 thread, 4.10% on 8). A shared lock-free table for it gave
  739.9 against 684.8 cps in one hold, not trusted yet. E29 plus E30 together rejected: 9.89
  against 9.76 and 9.56 ms, 7 rounds. A merge was pushed before its tests finished; they passed.
- **pipeline cd0a1f1**. Stone-ore match cache kept (9480668; `ore.rs`, `region.rs`): decorate
  13563 against 15941 ms on 1 thread, 3 of 3, digest `df701ee858e2bdd3` on every row, merged
  with datapath d137014. Decoration CPU at radius 16 is now 11.4 s against 21.5 s in the first
  profile. Spiral counting test (81e6458, `decorate.rs`): with the live cache sizes a finished
  chunk costs 2.77 terrain generations and 1.06 feature runs; with a terrain cache of 1024 it
  is 1.13 and 1.06, the floor. So the cache explains 2-3x of the live 11x drop, not all of it.
- Box: the Java server is still running (33% of a core at 17:35 UTC, 98% sampled by datapath
  during one hold). Disk 67%, 9 dirs. No OmniRoute calls. Sub-agents: noise 15, datapath 22,
  pipeline 8.

Messages: noise (per-thread scratch for `run_cell`/`run_lanes`, held at 1 and 8 threads),
datapath (the shared table must verify the full key or prove the packing injective; then the
holds), pipeline (carry on with sculk and lush caves).

Stopped reading at: noise af380e3, datapath 00b456d, pipeline cd0a1f1

## Pass 18, 18:35 UTC

Read only; no auditor re-measurement this pass.

- **Box: the Java server is the blocker.** It was restarted after the reservation (pid 1742511)
  and uses 1-6 cores: noise measured 64 CPU-seconds of Java per 10.5 s run on average during its
  1-thread hold; datapath's 8-thread hold has waited an hour for it to drop under 20% of a
  core. Not ours to stop. 8-thread numbers are not trustworthy while it runs.
- **noise 13cbfb2** (docs and a wip patch). Experiment 18, per-thread scratch pool: 1 thread
  101.5 against 98.0 and 97.1 cps (ahead in 4 of 5 against both); 8 threads 586.0 against 584.2
  and 623.9. Disturbed, not landed. Best still 9.15 ms on 1 thread at 01528de. Limit 11%.
- **datapath c38344f** (docs only). Conditions for the E18 shared table logged; its hold has not
  run. No new number. Limit 15%.
- **pipeline 1b749a0**. Sculk per-state cache kept (2ac77fe): decorate 12464 against 13523 ms,
  1 thread, 3 of 3, digest `df701ee858e2bdd3` on every row, merged with datapath c38344f; Java
  at 1.1-2.1 cores during the hold, equal for both trees by alternation. Lush-cave scan change
  rejected (13054 against 13149 ms). **Outside its files:** 8 additive lines in
  `crates/oxide-datapack/src/block_facts.rs` (`BlockFacts::bits`, plus an import reorder),
  declared in its log; `oxide-datapack` belongs to no track. Left standing. Limit 20%.
- No OmniRoute calls. Disk 70%, 13 dirs. Sub-agents in total: noise 15, datapath 22, pipeline 8.

Messages (all three): log user+sys CPU seconds per run and Java CPU per round, drop disturbed
rounds; noise also to log allocations per chunk for the scratch pool; datapath to stop waiting
for an idle Java; pipeline told the out-of-crate rule.

Stopped reading at: noise 13cbfb2, datapath c38344f, pipeline 1b749a0

## Pass 19, 19:35 UTC

Read only; no auditor re-measurement this pass.

- **noise**: no commit since 13cbfb2. Its session compacted while idle; a 7-round 8-thread hold
  of the scratch pool (two bases) is running. Best still 9.15 ms on 1 thread. Limit 17%.
- **datapath aa8ffab** (docs only; code bd442e7). Scaling report closed: in quiet rounds CPU per
  chunk at 8 threads matches 1 thread (about 9.6 ms); real extra CPU at 8 threads is 8-12%
  (fill +13%, E18 rescans, corner cache hit rate 54% to 50%); most of the rest was the Java
  server. E32 (shared surface table) parked: 8 threads 731 against 717 cps on the 5 of 16
  rounds that survived the Java filter; 1 thread 9.63 against 9.78 s median. Its key argument
  (24-bit quart coordinates, bijective inside the world border, full key compared on read,
  out-of-range columns skip the table) reads sound. The track stopped at 19:03 saying nothing
  left is measurable. Head: about 9.6 ms on 1 thread, 690-750 cps on 8. Limit 23%.
- **pipeline cb76f56**. Lush biome gate on reachable quarts kept (b54e352, 7bb1272; files in
  `decorator/` and `region.rs`): decorate 9520 against 10591 ms, 1 thread, 3 of 3; user CPU
  26.58 against 27.61 s; Java quiet in every round; digest `df701ee858e2bdd3` on every row,
  merged with datapath c38344f. Decoration at radius 16 on 1 thread: 21.7 s at the start of the
  day, 9.5 s now. Hold script now logs CPU seconds and Java CPU per run. Limit 28%.
- Box: Java at 8% of a core at 19:35 UTC. Disk 68%, 11 dirs. All jobs under the lock. No
  OmniRoute calls. Sub-agents in total: noise 15, datapath 22, pipeline 9.
- Unowned code named by the tracks as costs: harness `carvers_at` clones the carver list 289
  times per chunk; `SurfaceSystem::new` runs per chunk (about 2%).

Messages: datapath (stack E32, a per-thread SurfaceSystem cache and noise's scratch pool into
one hold; read-only analysis of the live path, 115 cps live against about 700 in the harness).
None to noise or pipeline.

Stopped reading at: noise 13cbfb2, datapath aa8ffab, pipeline cb76f56

## Pass 20, 20:35 UTC

Read only; no auditor re-measurement this pass. No messages sent: every track is working on
what it was last told.

- **noise 9fabbb3** (docs and tsv rows). Scratch pool (18) re-held, 7 rounds, no round dropped
  for Java: 1 thread 35.40 against 36.21 and 36.33 CPU-s (106.0 against 102.8 and 103.8 cps);
  8 threads 36.68 against 37.59 and 37.48 CPU-s (717.6 against 690.9 and 687.5 cps).
  Allocations fall by 7,607 calls and 6.07 MB per chunk (deterministic count). Inconclusive by
  its own bar, not landed; being re-held on generation-only CPU. Finding: with `--time-only`
  the harness still builds the merkle digest single-threaded after the timed part, about 25 s
  of a 36 s 1-thread run, so whole-process CPU dilutes a generation effect about 3.5x and each
  hold round holds the lock 3.5x longer than the timed part needs. Best landed: 9.15 ms on 1
  thread at 01528de. Limit 24%.
- **datapath 4ddd92c** (docs and a parked diff; code bd442e7). E33: `SurfaceSystem`'s program
  cached per thread, keyed by router id, `min_y`, `height` and a field-by-field comparison of
  the surface rule; no `unsafe`; test with the rule swapped under one router. Stack (E32 + E33
  + noise's scratch pool) built in a scratch tree; its hold is queued. Live-path reading: at 115
  cps a chunk costs about 70 ms of core time, Oxide about 15 ms of it, the FFI boundary under
  1 ms, Java the rest. Unverified: whether Vertex still runs vanilla NOISE fill after the hook
  (`NOISE` is not in the claimed stage set). `oxide-ffi`'s `carvers_at` runs an uncached climate
  sample for each of 289 source chunks per chunk (2-5 ms per chunk by estimate). Limit 40%.
- **pipeline 3c2fafd** (docs only). Ore z-run with a written-cell bitmap rejected (10168 against
  9880 ms); z-run alone rejected (9085 against 9150 ms). Digest `df701ee858e2bdd3` on every
  row. Decoration stays at about 9.5 s on 1 thread at radius 16. Limit 35%.
- Box: Java present but quiet (0.2-0.3 CPU-s per 10 s in every logged round). Disk 69%, 12 dirs.
  All jobs under the lock. No OmniRoute calls. Sub-agents: noise 15, datapath 24, pipeline 10.

Open for the user, added this pass: a harness option to skip the digest on hold rounds (digest
checked once per binary) would cut hold time about 3.5x on 1 thread; the harness has no owner.

Stopped reading at: noise 9fabbb3, datapath 4ddd92c, pipeline 3c2fafd

## Pass 21 (2026-11-15 21:40 UTC)

No code landed on any track. Read: noise 3534f6a..e0ba0b6, datapath ef0764b..5c47191 (logs only);
pipeline has no commit since 3c2fafd (20:16).

- **noise** (code fd4bfe2, 9.15 ms 1T unchanged): exp 18 scratch pool decided on generation-only
  CPU: 10.93 s against bases 10.91/11.10 at 1T, between the bases at 8T; not landed. Now holding
  exp 17 (column x/z) on gen_cpu_s; that hold is 5 rounds at 1T, about 9 min exclusive.
- **datapath** (code bd442e7, unchanged): stack E32+E33+noise-12 on aa8ffab +9.2% at 1T (7/8
  rounds), +9.3% at 8T (5/8), 48 digests ok. E32+E33 alone: -2.6% at 1T (1/6), +8.5% at 8T (4/6);
  not landed. It attributes the 1T gain to noise-12 by subtraction.
- **Conflict:** the two tracks disagree on noise-12 (same noise crate in both trees: merge-base
  of aa8ffab and fd4bfe2 is fd4bfe2). Neither claim is landed. Asked datapath for one hold with
  four binaries (two bases, base+noise-12, stack) judged on generation-only CPU.
- **pipeline**: working, 4 sub-agents; a per-region write-table change is built on `pipeline-lg`
  and waiting for the lock. Nothing to check yet.
- Box: disk 69%, all builds under the shared lock, Java at 33% of a core. Limits: noise 28%,
  datapath 41%, pipeline 1% (window reset). No OmniRoute. All three sessions have unsubmitted
  supervisor nudges stacked in the input line.

Messages: noise (do not close 18, datapath's contrary result, 4-round holds); datapath (four-binary
hold on generation-only CPU).

Stopped reading at: noise e0ba0b6, datapath 5c47191, pipeline 3c2fafd

## Pass 22 (2026-11-15 22:40 UTC)

No code landed. Read: noise e0ba0b6..8a84a52, datapath 5c47191..26f512f (logs only); pipeline
still at 3c2fafd.

- **noise** (code fd4bfe2, 9.15 ms 1T): exp 17 column x/z rejected on gen_cpu_s (10.59 s between
  bases 10.79/10.49; hold cut to 4 rounds). 18 reopened; re-hold of the scratch pool on aa8ffab
  code running (4 rounds at 1T, then 8T).
- **datapath** (code bd442e7): withdrew its subtraction. Four-way hold on generation-only CPU,
  44 digests ok, Java <= 0.30 CPU-s per run: noise-12 alone 9.31 s against bases 9.90/9.85 at 1T
  (-5.7%, below both in 4/5), 10.47 against 10.81/11.24 at 8T (-5.1%, 4/6). E32+E33 on top:
  -1.3%, not established, parked. Under 10% and one hold, so noise's own re-hold is the repeat.
  Why noise saw nothing on fd4bfe2 is unexplained (its bases ran 10.5-11.1 s, datapath's 9.9).
- **pipeline**: idle since 21:07 with no job on the box; nudged.
- Datapath had stopped ("next gain depends on noise-12 landing"); redirected to a fresh profile
  on base+noise-12 and sub-agents. M2 arithmetic: ~780-810 cps now, 1,000 needs ~7.3 ms/chunk.
- Box: disk 68%, Java ~27% of a core, noise's hold is the only job. No OmniRoute.

Messages: pipeline (idle, log the write-table result); datapath (profile base+noise-12, do not wait).

Stopped reading at: noise 8a84a52, datapath 26f512f, pipeline 3c2fafd

## Pass 23 (2026-11-15 23:40 UTC)

Read: noise 8a84a52..ebd7526, datapath 26f512f..80add17, pipeline 3c2fafd..b84621e.

- **noise** landed b889079 (per-thread scratch buffers). Reviewed: `Scratch::filled` = clear +
  resize, same contents as `vec![v; n]`; IntervalSelect now indexes lanes then runs each function
  over its lanes ascending, same calls in the same order; no arithmetic touched. Evidence: own
  hold over aa8ffab code, 1T gen_cpu_s 9.62 against 10.38/10.09 (-6.0% on the mean, 3/4 rounds
  each), 8T 11.02 against 11.09/13.18; datapath's independent hold -5.7% 1T, -5.1% 8T. 24+44
  digests ok; tests noise 40, chunkgen 190+3. Accepted as a repeated sub-10% gain. Rows on
  merged code: 8.35-8.81 ms/chunk at 1T, 715-821 cps at 8T. On perf/noise alone no gain
  (10.93 against 10.91/11.10); the cause in datapath's call pattern is not traced. No merged
  pregen digest logged for it (cannot change NBT by construction, but the rule stands).
- **datapath** (code unchanged): profile of base+noise-12: noise 42% (corner grid 29%), surface
  20%, carvers 12.6%, aquifer 9.3%, fill loop 6.3%, biomes 4.6%, ore veins 3.7%. Round 8: three
  sub-agents (surface, carvers, fill+ore veins) and E30 retry; holds queued, 6 rounds x 3
  binaries at 1T (~11 min exclusive each).
- **pipeline**: dense per-section write overlay in `region.rs`: decorate 8925 against 9815 ms
  (-9.1%, 3/3) on one base, pregen digest ok on all 6 rows; second base pending, not landed.
  Dark forest, glow lichen and geode experiments built, waiting on the lock.
- Box: disk 73% (16 bench dirs), Java ~22% of a core, four exclusive holds queued. No OmniRoute.

Messages: noise (idle; work over the merged tree, three profile targets, sub-agents); datapath
(4-round holds, do not stack holds on the lock).

Stopped reading at: noise ebd7526, datapath 80add17, pipeline b84621e

## Pass 24 (2026-11-16 00:40 UTC)

Read: noise ebd7526..c09255b, datapath 80add17..f52ccea, pipeline b84621e..8ab747b.

- **pipeline** landed 40bf0b6 (dense per-section write overlay, `region.rs` only): decorate
  -9.1% (3/3) on base 1 and -8.4% (2/3) on base 2, 5 of 6 rounds, pregen digest on all 12 rows,
  chunkgen 194. Reviewed: per-cell id+1 and write index replace the HashMap; write order and
  rewrite-in-place unchanged. Decoration now ~8.9 s at r16 1T. Pending: tree state cache -12.3%
  (3/3) on base 1, second base queued; lichen/geode +6.8% slower on base 1, heading for reject.
- **datapath** merged noise b889079 (365ebeb; chunkgen 192, noise 40). No code landed. Round 8
  diffs kept as `pending-E34/E35/E36-*.diff`: fill loop + ore veins, surface bulk sections
  (new palette-order test on 7 chunks), tunnel walk record/replay (bit-for-bit test, 300 tunnels
  x 361 chunks). Only single profiles so far; holds run one at a time, 4 rounds at 1T. Not
  reviewed until a hold decides.
- **noise** (code b889079): now measures over the shipping tree (ef9a35f), holds at radius 12
  (~4 min), digest at r16 separately. Profile: noise 45.7% of generation. Three sub-agents:
  20a lane-vectorised Perlin (must keep per-lane IEEE expression; review on landing), 20b
  run_cell loops, 20c ore-vein sampling. Finding for datapath/oxide-core: `SIN_TABLE` LazyLock
  check on every `mth::sin`/`cos` is 2.6% of generation self time (`oxide-core/src/mth.rs:18`);
  oxide-core has no track owner.
- **Lock starvation:** datapath's E30 hold waited ~45 min for the exclusive lock, pipeline's
  has waited 29 min, three noise holds behind it. Likely cause: ~10 sub-agents keep the shared
  lock held with builds and tests, and flock gives waiting exclusive holders no priority.
  Needs a turnstile in `scripts/bench-vps.sh` (user's script); not a track fix.
- Box: disk 77% (28 bench dirs, ~25 GB), Java ~20% of a core. No OmniRoute. All tracks have
  sub-agents working.

Messages: none.

Stopped reading at: noise c09255b, datapath f52ccea, pipeline 8ab747b

## Pass 25 (2026-11-16 01:40 UTC)

Read: noise c09255b..43def89, pipeline 8ab747b..b793819; datapath no commit since f52ccea (00:16).

- **noise** landed 07a6727 (block kernel for Perlin and Blended octaves, up to 64 lanes in
  three phases). Reviewed against `noise_with_scale_reusing`: same scale/wrap/offset/floor,
  same smear expression with the mode fixed per block, `u = smoothstep(dy)` on the un-smeared
  dy, `lerp_corners_faded` = same 8 dots and 7 lerps; Blended limit stacks compacted by the same
  `!(f >= 1)` / `!(f <= 0)` tests, sums from 0.0 in octave order (old accumulators also started
  at 0.0); Perlin `get_values` passes `0.0 * freq` so no smear, as before. No reassociation, no
  FMA (its disassembly check). Evidence over ef9a35f at radius 12, 1T gen_cpu_s 5.17 against
  5.46/5.42 (-5.3%/-4.5%, below both bases in 4/4 rounds); 8T inside the spread (7.47 against
  6.87/7.82). r16 digest ok; tests noise 42, chunkgen 190+3. One hold only; datapath's rebuild
  on it serves as the repeat. Rows: 7.64-8.04 ms/chunk 1T at r12.
  Not landed: 20b cell lerp + register reuse (-5.9%/-3.0%), 20c vein memo hit path
  (-2.0%/-3.8%); bundled with 2D-cache pooling as exp 21, hold queued. No merged pregen digest
  for b889079 or 07a6727 yet.
- **pipeline** landed 95ae585 (tree per-state info cache + interned writes): -12.3% (3/3) on
  base 1, -3.8% (3/3) on base 2 (which already has the overlay); pregen digest on all rows,
  tests 194. Lichen/geode rejected on two bases (+6.8%, +1.2%). Pending: lush patch bodies
  -8.4% (3/3) on base 1. Decoration ~8.9 s at r16 1T on the latest base. All inside decorator/.
- **datapath**: no result logged for E30 although its hold started ~00:30; E34 hold waiting
  19 min; session idle between waits.
- Box: disk 73% (17 dirs, tracks cleaned up), Java ~17% of a core. No OmniRoute.

Messages: datapath (log E30, push per hold, merge 07a6727 after this sequence); noise (review
result, merged pregen digest owed for both landings).

Stopped reading at: noise 43def89, datapath f52ccea, pipeline b793819

## Pass 26 (2026-11-16 02:40 UTC)

Read: noise 43def89..c17f4ed, datapath f52ccea..c34e27b, pipeline b793819..cec45c2.

- **noise** landed ac68e26 (bundle 21: fixed-width cell lerp, unzeroed pooled registers in
  `run_cell`, borrow-free vein memo, pooled 2D column cache). Reviewed: every `run_cell` arm
  (const fill, unary/binary over full slices, `fill_cell_interpolated`, `fill_cell_by_y`,
  range_choice with at least one branch) writes all lanes of its register before any read, so
  `Scratch::overwritten` never exposes stale values; `lerp_cell_fixed` has the same lerps,
  operands and order, `dz = d[lz]` is the same product; memo tags updated after the fill as
  before. Evidence over ef9a35f at r12: 1T gen_cpu_s 5.21 against 5.53/5.43 (-5.8%/-4.1%,
  below both in 4/4); 8T 6.58 against 6.36/6.88, inside the spread. r16 digest ok on both
  trees; tests noise 45, chunkgen 190+3. Held without 07a6727 (disjoint files); the two together
  are not yet measured. Pregen digest df701ee858e2bdd3 logged for b889079+07a6727 on a
  pipeline-merged tree (e4fb8f3); owed for ac68e26.
- **datapath** (code = 365ebeb merge): E30 rejected for good (below both bases 1/4 at 1T, 1/6
  at 8T). E34 fill loop + ore veins: 1T 8.75 against 9.13/9.37 (-5.5%, 2/4), 8T 10.25 against
  11.10/11.46 (-9.1%, 3/6); candidate, to be re-held on bases rebuilt after merging noise.
  E35 hold running, E36 next. Bases now ~9.2-9.4 s gen CPU at r16 1T (8.5 ms/chunk) before
  07a6727 and ac68e26.
- **pipeline**: lush patch bodies rejected (base 1 -8.4%, base 2 +1.4% and -2.5%; nothing on
  the current base). Profile of the head: decorate 7.3 s CPU at r16 1T, ores ~35% (66M box
  cells, 36.8M in sphere, 8.0M writes); ore z-run being re-held on the dense-overlay base.
  Digest on every row. No code landed this hour.
- Box: disk 72% (15 dirs), Java ~16% of a core, one hold running, queue short. No OmniRoute.

Messages: noise (review result; build its own shipping tree, do not wait for datapath's merge).

Stopped reading at: noise c17f4ed, datapath c34e27b, pipeline cec45c2

## Pass 27 (2026-11-16 03:40 UTC)

Read: noise c17f4ed..a13718d, datapath c34e27b..667bdbb, pipeline cec45c2..80f76d3. No code
landed on any track this hour.

- **noise** (code ac68e26): pregen digest df701ee858e2bdd3 on a pipeline-merged tree for
  ac68e26; all three session landings now checked. New bases on c34e27b crates + ac68e26.
  Profile: noise 41.9% of generation (45.7% before), corner grid 24.3%, `grad_dot` 5.2% the
  largest self-time item. In flight: 22a packed dots/lerps (a gradient-coefficient table: check
  on landing that each lane keeps the scalar operand order and signed zeros), 22b vein corner
  layers.
- **datapath** (code 365ebeb merge): three candidates on gen_cpu_s against two bases, all
  digests ok: E35 surface bulk sections 1T 8.45 against 8.94/9.21 (-6.8%, 4/4), 8T flat
  (-1.6%, 2/6); E36 tunnel replay 1T 8.43 against 8.74/9.32 (-6.7%, 3/4; bases 6.6% apart),
  8T hold waiting; E34 from last pass (-5.5%, 2/4). None landed.
- **pipeline**: ore z-run rejected a second time (-2.1%, 2/3). Ore one-lookup cells + pooled
  overlay sections (`decorator/ore.rs`, `region.rs`): decorate 8033 against 8837 ms (-9.1%,
  3/3) on base 1, pregen digest on all rows, tests 195; second base next.
- Box: disk 74% (21 dirs), lock queue long again (datapath's 8T hold waited 25 min behind
  shared builds). Transient ssh resets to the box around 02:20-02:45, reported by all three
  tracks; no runs lost. No OmniRoute.

Messages: datapath (hold E34+E35+E36 as one stack on bases rebuilt with ac68e26).

Stopped reading at: noise a13718d, datapath 667bdbb, pipeline 80f76d3
