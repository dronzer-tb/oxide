# Roadmap

Maps PRD §6 milestones onto the crate graph. Waves are the parallelism boundary — everything in a
wave can be built concurrently; a wave starts when the previous one compiles and its tests pass.

| Wave | Crates | PRD milestone | Status |
|---|---|---|---|
| 0 | `oxide-core` | 1 | in progress |
| 1 | `oxide-datapack`, `oxide-anvil`, CI | 1, 2 | in progress |
| 2 | `oxide-noise` | 1 | blocked on wave 0–1 |
| 3 | `oxide-biome`, `oxide-chunkgen` | 1 | blocked on wave 2 |
| 4 | `oxide-structures`, `oxide-harness` | 3, 4 | blocked on wave 3 |
| 5 | `plugin/`, `oxide-ffi` | 5, 6, 7 | blocked on wave 4 |

## Gates between waves

- **0 → 1**: `oxide-core` RNG verified against recorded Java test vectors. Everything downstream
  inherits RNG bugs, so an unverified RNG makes later parity numbers meaningless.
- **1 → 2**: the vanilla worldgen registries load and reference-resolve cleanly from a locally
  extracted `reference/` tree (see `REFERENCE_DATA.md`).
- **2 → 3**: density-function interpreter reproduces sampled values from a vanilla reference dump.
- **3 → 4**: a generated chunk opens in a vanilla client without corruption (PRD milestone 2 exit).
- **4 → 5**: surface/heightmap pass rate is high enough to be worth wiring into a live server
  (PRD §7 targets >99% for non-structure chunks).

## Known unknowns

- **Minecraft 26.2 specifics are not assumed.** The data-driven design exists precisely so that no
  26.2 constant, DataVersion, or noise parameter is written from memory. Anything a crate could not
  confirm is marked with a `// PARITY-CHECK:` comment naming what needs verification against a real
  Java run or world save. Those comments are the punch list for wave gates — grep for them before
  declaring a wave done.
- Structure placement parity is a hard v1 requirement, which makes `oxide-structures` the highest-risk
  crate: it depends on RNG exactness, biome resolution, and chunkgen all being correct first.
