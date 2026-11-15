# Performance plan: terrain generation on the bench VPS

Written 2026-11-15. Three sessions work this in parallel, one track each. Read this file, then
your track file, then start.

## Target and where we are

Machine: bench VPS, ssh host `s5`, 8-core AMD EPYC (Zen 1, ~2 GHz), 23 GB.

| Measure | Value | How |
|---|---|---|
| Terrain only, 8 threads | **160.5 chunks/s** (49.8 ms core time per chunk) | `scripts/bench-vps.sh base 16 8` |
| Terrain only, 1 thread | 22.8 chunks/s (43.8 ms per chunk) | `scripts/bench-vps.sh base 6 1` |
| Live server, Chunky pregen | 60-66 chunks/s | Vertex Engine + Oxide module, 8 workers |

Goal: **5,000 chunks/s** on this machine, which is 1.6 ms of core time per chunk, a 31x cut.
That is a stretch goal, not a forecast. Report honestly against these gates:

| Gate | chunks/s (8 threads) | ms core per chunk |
|---|---|---|
| M1 | 400 | 20 |
| M2 | 1,000 | 8 |
| M3 | 2,500 | 3.2 |
| Goal | 5,000 | 1.6 |

The live server will never show these numbers: with Oxide at zero cost the JVM side (vanilla
features, lighting, saving) caps it near 240 chunks/s. The target is for offline generation.

## Where the time goes (perf, 25 s sample of the live server, 2026-11-15)

Share of all CPU: Oxide native 52%, JIT Java 25%, libc (malloc/free/memcpy) 17%, zlib 2%.
Inside Oxide: `oxide_noise::compiled::Program::run` 14%, `ImprovedNoise::noise_with_scale`
11%, surface `eval_rule` + `apply` 6%, biome search `Node::distance` 3%, aquifer
`compute_substance` 2%, `String::clone` and `BlockState` drop glue visible, the rest thin.

## Tracks

| Track | Worktree | Branch | Owns |
|---|---|---|---|
| noise | `~/oxide-wt/noise` | `perf/noise` | `crates/oxide-noise` |
| datapath | `~/oxide-wt/datapath` | `perf/datapath` | `crates/oxide-core`, `crates/oxide-biome`, `crates/oxide-chunkgen` except `decorator/` |
| pipeline | `~/oxide-wt/pipeline` | `perf/pipeline` | `crates/oxide-pregen`, `crates/oxide-anvil`, `scripts/` |

Stay inside what you own. If you need a change elsewhere, make the smallest additive one and
say so in your log; never rename or reshape another track's public items.

## Rules for every track

1. **Measure before and after every change**: `scripts/bench-vps.sh <track> 16 8`. It copies
   your tree to the VPS, builds there for that CPU, and prints one `BENCH` line. Runs queue on
   a lock, so wait your turn rather than working around it. Do not build `--release` locally.
2. **The digest is the correctness gate.** At radius 16 it is `a977961a58122047` today. If it
   changes, the generated world changed: revert, or stop and write down exactly why in your
   log. Run `cargo test -p <crates you touched>` before each commit.
3. **Push after every experiment.** The bench script appends each result to
   `docs/perf/bench-<track>.tsv`; commit it with your log and push, kept or reverted. The
   progress page at graph.tharax.me/lab/progress.html reads both files from GitHub, and it is
   the only way anyone sees what you are doing. Benchmark noise is about 8% run to run, so
   repeat a run before trusting a gain under 10%.
   **Log every experiment** in `docs/perf/LOG-<track>.md`: what you tried, the BENCH line
   before and after, kept or reverted. Failed experiments are worth a line; they stop the
   next session repeating them.
4. **Commit small, Conventional Commits, no attribution lines.** Push your own branch only.
   Open one PR into `feat/native-decoration` when you have a kept win; do not merge it.
5. **Never touch the live server** (panel server `oxide-bench`, the `e9097af6...` container)
   or anything on the VPS outside `/root/oxide-perf/`. No sudo anywhere.
6. Sub-agents: use Opus, give each a single narrow job and a hard file boundary, and have them
   query graphify (`graphify query "..."` in the repo) before reading source. Only the master
   commits and pushes.
6b. **Free workers:** `scripts/omni-agent.sh <model> "<task>"` runs a full Claude Code worker
   (files, shell, edits) on an OmniRoute model, at no Claude cost. Run it in its own scratch
   worktree (`git worktree add /tmp/omni-<name> HEAD`), never in yours, then review its diff.
   Good for: trying a variant you would not spend your own turns on, writing a micro-benchmark,
   reading a large file and reporting back. Use `gemini/gemini-3-flash-preview` (verified with tools);
   `mistral/codestral-latest` answers questions. If a model fails, log it and stop using it.
6a. **Cheap second opinions:** `scripts/omni-ask.sh <prompt-file> [model]` sends one prompt to a
   model on OmniRoute and prints the answer, at no Claude cost. Use it for review, for ideas
   when stuck, and for reading a profile. It cannot see the repo or run anything, so paste
   what it needs. Treat its code as a suggestion: nothing lands without the bench and the
   digest. If the key is missing the script says so; carry on without it.
7. `reference/` is game data and gitignored; each worktree has a symlink to the main
   checkout's copy. Never commit it, never copy it anywhere but the VPS bench directory.
8. Float parity rules in `docs/ARCHITECTURE.md` ("Float parity is not free") still apply:
   reordering float operations or using FMA changes results. The digest will catch it.
9. When you run out of ideas or hit your usage limit, finish by updating your log with the
   current BENCH line, what is in flight, and the next three things you would try.
