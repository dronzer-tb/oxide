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
