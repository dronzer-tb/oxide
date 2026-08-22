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
- No `unsafe` outside `oxide-ffi`.
- Every parity-critical algorithm carries unit tests with recorded expected values.
- Heavy release builds run in GitHub Actions, not locally.
