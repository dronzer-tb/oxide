# Oxide — Architecture

Hybrid Rust/Java chunk generation for Folia. See `prd.txt` for product scope.

## Pinned decisions (v1)

- **Minecraft version:** `26.2`. The DataVersion integer is **never hardcoded from memory** — it is
  read from the exported datapack metadata (`version.json` / `pack.mcmeta`) at load time and carried
  through to the Anvil writer.
- **Structure placement parity:** required. Structure placement must reproduce vanilla's RNG chain
  exactly for the pinned version.
- **Fallback:** Java is the permanent generator for the cold radius. No rule-based production
  failover system is in scope (PRD §4, §9).

## Core design call: data-driven worldgen

Rust does **not** hardcode noise constants, density-function trees, surface rules, or biome climate
parameters. It loads the vanilla-exported worldgen registries as JSON and interprets them:

- `worldgen/noise_settings/*.json` — noise router, surface rule tree, sea level, default block/fluid
- `worldgen/density_function/*.json` — named density functions referenced by the router
- `worldgen/noise/*.json` — normal-noise parameter sets (firstOctave, amplitudes)
- `worldgen/biome/*.json` — biome definitions
- `dimension/*.json`, `dimension_type/*.json`
- `worldgen/structure*/**` — structure sets, placement, spread

Consequences:
- A Minecraft version bump is a **data re-export**, not a code port (mitigates PRD §8 version churn).
- Nobody has to guess 26.2's internals; correctness is checkable against the same JSON the server reads.
- Reference data extraction is a local, manual step. Extracted vanilla data and reference chunk NBT
  are **gitignored and never committed or redistributed** (PRD §4, §8).

Put extracted data under `reference/` (gitignored). Document the extraction command in
`docs/REFERENCE_DATA.md` rather than committing outputs.

## Crate graph

```
oxide-core ──┬── oxide-datapack ──┬── oxide-noise ──┬── oxide-biome ──┐
             │                    │                 │                 ├── oxide-chunkgen ──┬── oxide-structures
             │                    └─────────────────┴─────────────────┘                    │
             └── oxide-anvil ─────────────────────────────────────────────────────────────┴── oxide-harness (bin)
                                                                                            └── oxide-ffi (cdylib)
```

| Crate | Responsibility |
|---|---|
| `oxide-core` | Positions, block/biome identifiers, paletted containers, heightmap types, chunk data model, and **all RNG primitives** |
| `oxide-datapack` | Deserialize + validate the worldgen JSON registries; resolve inter-registry references; expose typed IR |
| `oxide-noise` | Perlin / improved-noise / normal-noise / blended-noise primitives + the density-function interpreter and noise router |
| `oxide-biome` | Climate parameter sampling and the multi-noise (n-dimensional KD) biome search tree |
| `oxide-chunkgen` | Noise-based terrain fill, aquifers, ore veins, surface rules, carvers, heightmap computation |
| `oxide-structures` | Structure set placement RNG chain, start selection, piece layout |
| `oxide-anvil` | NBT chunk serialization, section/biome palettes, `.mca` region writer, region-file locking |
| `oxide-harness` | Dev-only: Merkle diff vs vanilla reference, invariant checks, bucketed pass-rate report |
| `oxide-ffi` | C ABI surface consumed by the Folia plugin over Panama (milestone 5) |

## RNG is load-bearing

Every downstream parity property — surface rules, carvers, ore placement, structure placement — is
built on RNG behaviour. `oxide-core` RNG must be bit-exact with the Java implementation, including:

- `Xoroshiro128PlusPlus` and its seed upgrade path
- Legacy `java.util.Random` (LCG) for legacy-seeded code paths
- Positional random factories (`at(x, y, z)`, `fromHashOf(name)`), including the MD5 name hashing
- `nextInt(bound)` rejection-sampling behaviour, `nextGaussian` pair caching, `nextDouble` bit layout

Bit-exactness here is verified with recorded Java test vectors before any dependent crate is trusted.
A silent RNG bug is indistinguishable from an algorithm bug three crates downstream, which is why
this is the first thing built and the most heavily tested.

## Chunk output contract

Chunks written by Rust are marked **needs relight** rather than carrying self-computed lighting
(PRD §5.2, §8). Heightmaps written: `WORLD_SURFACE`, `WORLD_SURFACE_WG`, `OCEAN_FLOOR`,
`OCEAN_FLOOR_WG`, `MOTION_BLOCKING`, `MOTION_BLOCKING_NO_LEAVES`.

## Float parity is not free

Java has been strict-FP everywhere since 17, so `+ - * /` and `sqrt` match IEEE-754 in Rust
exactly. Three things do not, and each has already produced or nearly produced a divergence:

**Vanilla often isn't doing the maths you think.** `Mth.sin`/`Mth.cos` are a 65536-entry table of
`f32` built as `(float) Math.sin(i / 10430.378…)`, not trigonometry. Carvers, structure placement
and several features are shaped by that table. Calling a real `sin()` gives a visibly different
cave, not a last-ulp difference. Ported in `oxide_core::mth`; use it wherever vanilla writes
`Mth.`.

**Real transcendentals differ between Java and Rust.** Measured, not assumed: over 400 sampled
angles, `f64::cos` and `f64::sin` each differ from Java's `Math.cos`/`Math.sin` by 1 ulp on about
0.25% of inputs. Anywhere vanilla calls `Math.sin`, `cos`, `pow`, `exp` or `log` and the result
reaches an `int` — a `Math.round`, a floor, an array index — that 1 ulp can change a block.
Concentric-ring stronghold placement is the known case: it feeds `Math.cos` into `Math.round`.
Being bit-exact there needs the fdlibm algorithm ported, not the platform's.

`oxide_biome::search::bucket_size` uses `powf`/`ln` and is *safe* because a `floor()` and a cast
to `usize` collapse the difference, and because R-tree bucketing changes the tree's shape rather
than which biome is nearest. Safe by argument, not by luck — check the same way before adding
another.

**Narrowing order and operation order matter.** `Mth.sin((float)(Math.PI / 2))` narrows to `f32`
*before* the table lookup and can select a different index than the `f64` would. The cave carver
computes `Mth.PI * currentStep / distance` while the canyon computes
`currentStep * Mth.PI / distance`; float multiplication is not associative, so those are
different numbers. Transcribe the expression, not its meaning.

## Structures: three layers, and the one the API blocks

Structure generation splits into placement (which chunk starts one), assembly (what the pieces
are), and registration (telling the server a structure exists there). The three have very
different states, and merging them in a status report is how "structures work" gets claimed for a
world that has none.

**Placement** is `oxide-structures`' verified core: `random_spread` start chunks and frequency
reduction are bit-exact against real 26.2. `concentric_rings` is present but skips vanilla's
`findBiomeHorizontal` search, so stronghold positions are approximate, not parity.

**Start bookkeeping** (`starts.rs`) is the layer that makes per-chunk generation able to emit
multi-chunk structures. Its one invariant: a start is a pure function of `(world_seed, start
chunk, structure set)`. Nothing may consult a neighbour, a previous chunk, or the cache's
contents. That is what lets Folia's region threads generate in any order and in parallel and
still agree, and it is why the start cache is a pure memoisation — eviction and duplicated work
under a race are allowed to cost time but cannot change output. `StructureStartCache` therefore
assembles *outside* its lock rather than serialising region threads behind one mutex.

**Assembly** (`jigsaw.rs`) is a port of `JigsawPlacement`: four rotations, `canAttach` connector
matching, child-origin offsets, free-space collision, `terrain_matching` projection, the village
expansion hack, and priority-ordered expansion. It assembles 60-100-piece plains villages,
pillager outposts and ancient cities from Mojang's own pools and `.nbt` templates. Its primitives
(`StructureTemplate.transform`, `Util.shuffle`, `Rotation.getShuffled`/`getRandom`) are bit-exact
against the game; the piece *layout* is not yet diffed against a real generated village, so it is
correct by construction rather than verified. Processors, `list`/`feature` pool elements, pool
aliases and dimension padding are not ported.

The free-space region deserves a note, because it is the one place the port deliberately does not
mirror vanilla's data structure. Vanilla carries a `VoxelShape` and asks
`Shapes.joinIsNotEmpty(free, box.deflate(0.25), ONLY_SECOND)`. That shape is only ever built as
one box with boxes subtracted from it, so the question it answers is exactly "inside the limit and
touching none of the taken boxes" — which `FreeRegion` answers directly with integer box tests, no
voxel grid. The `deflate(0.25)` is what lets two pieces share a face while rejecting a one-block
overlap, and that is what an inclusive integer intersection already says.

**Registration is not available on the Bukkit path at all.** Verified against
`dev.folia:folia-api:26.2.build.5-beta`: `ChunkGenerator` exposes only `generateNoise` /
`generateSurface` / `generateCaves` / `generateBedrock` plus the `shouldGenerate*` gates;
`GeneratedStructure` and `Chunk.getStructures()` are read-only getters, and `LimitedRegion` has no
structure API. There is no method anywhere in the API to attach a `StructureStart` or a structure
reference to a chunk. So a plugin-path native jigsaw could place the *blocks* of a village and
still leave `/locate` blind, structure-gated mob spawning dead (outpost pillagers, fortress
blazes, monument guardians), and structure-conditioned loot and advancements unarmed. Native
structures therefore require the Vertex Engine module path, which sits below the API. The target
method exists: 26.2's `ChunkGenerator.tryGenerateStructure` calls
`StructureManager.setStartForStructure(SectionPos, Structure, StructureStart, StructureAccess)`
once `StructureStart.isValid()`, so registration is a real internal call rather than a hoped-for
one. What is still unverified is whether the Vertex module path can reach it. Until that is
answered, Java owns structures and `shouldGenerateStructures()` stays `true`.

Placement's RNG derivations are checked against the game itself rather than against a
transcription: `crates/oxide-structures/tests/data/FeatureSeedOracle.java` runs 26.2's own
`WorldgenRandom` out of a Mojang-mapped jar, and the vectors it prints are what the Rust test
diffs. That is the standard a `PARITY-CHECK` marker has to be cleared by — a test that recomputes
the formula it is testing clears nothing.

## Validation harness

Per chunk: Merkle tree of `chunk → section → sub-region → leaf`, leaf granularity configurable.
Root mismatch triggers a descent to localize the diverging sub-region. Results are bucketed by biome
and by structure proximity, not aggregated into one number. Invariant checks (heightmap agrees with
actual surface, no bedrock breach, valid biome ids, no floating trees) run without a Java reference.
Only pass/fail results and code are committable.

## Non-negotiables for contributors and agents

- Errors: `anyhow` at binary/boundary level, `thiserror` for library error enums. No `unwrap()` in
  library code paths that can be reached from generation.
- Parallelism: `rayon`. Async is not used — generation is CPU-bound.
- `unsafe` lives only in the two crates that own a C ABI: `oxide-ffi` and `pacside`. In both, a
  `pub extern "C"` function that dereferences a caller's pointer is declared `unsafe fn` with a
  `# Safety` section stating the contract — the compiler cannot check what Java passes, so the
  signature has to carry it. `cargo clippy --workspace --all-targets` denies the alternative
  (`clippy::not_unsafe_ptr_arg_deref`) and is expected to pass clean.
- Every parity-critical algorithm carries unit tests with recorded expected values.
- Heavy release builds run in GitHub Actions, not locally.
