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
