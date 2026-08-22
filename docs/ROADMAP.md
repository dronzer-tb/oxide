# Roadmap

Maps PRD §6 milestones onto the crate graph. Waves are the parallelism boundary — everything in a
wave can be built concurrently; a wave starts when the previous one compiles and its tests pass.

| Wave | Crates | PRD milestone | Status |
|---|---|---|---|
| 0 | `oxide-core` | 1 | done — `9b1c332`, 43 tests |
| 1 | `oxide-datapack`, `oxide-anvil`, CI | 1, 2 | done — `d727fb0`, `f401a90`, `3789e86` |
| 2 | `oxide-noise` | 1 | done — `6974f82`, 17 tests, unverified vs Java (see PARITY-CHECK) |
| 3 | `oxide-biome`, `oxide-chunkgen` | 1 | done (scoped) — `b3f5671`, 9 tests. Terrain fill + heightmaps + biome grid only; aquifers/ore veins/carvers/surface rules deferred, see `fill.rs` |
| 4 | `oxide-structures`, `oxide-harness` | 3, 4 | done (scoped) — `0a3893c`, 17 tests. random_spread placement + weighted selection only; concentric_rings/frequency reduction/exclusion zones/piece layout deferred. Harness self-checks only — no vanilla reference data to diff against |
| 5 | `plugin/` router, `oxide-ffi` | 5, 6, 7 | not started |

## v0 smoke test (out of band)

Built ahead of wave 2 so the write path could be exercised before any real
generator exists. It proves plumbing, not generation quality.

| Piece | Commit | What it is |
|---|---|---|
| Provenance sidecar | `3789e86` | `r.X.Z.mca.oxide`, 136 bytes, magic `OXPV`, 1024-bit LSB-first bitmap indexed `local_z*32+local_x`. Marked inside the region write's `RegionGuard`. |
| `oxide-pregen` | `5ac8224` | CLI writing deliberately synthetic checkerboard/flat chunks into a world's region dir. |
| `plugin/` debug overlay | `b4fe0c8` | Folia action-bar readout of chunk provenance, `/oxide debug`, `/oxide here`. |

The F3 screen is client-side and cannot be extended by a server, so the readout
is the action bar. It works on vanilla Java clients and on Bedrock via Geyser.

Blocked on two things only the user can supply: a real `--data-version` from a
26.2 export, and a verified `foliaApiVersion` in `plugin/gradle.properties`
(currently a deliberate placeholder that will not resolve). `.github/workflows/
plugin.yml` is `workflow_dispatch` only until that version is real.

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

- **21 `PARITY-CHECK` markers** are open across the crates (`grep -rn "PARITY-CHECK" crates/`).
  The load-bearing ones: `oxide-core`'s Xoroshiro seed constants, bounded-int path and positional
  mix are reconstructed and unverified, while `LegacyRandom` **was** checked against a real JDK 25
  run. `oxide-anvil`'s `isLightOn` relight key and `ChunkStatus` NBT strings, and `oxide-pregen`'s
  world bounds and block-array index order, are conventions rather than confirmed 26.2 facts.
- **Minecraft 26.2 specifics are not assumed.** The data-driven design exists precisely so that no
  26.2 constant, DataVersion, or noise parameter is written from memory. Anything a crate could not
  confirm is marked with a `// PARITY-CHECK:` comment naming what needs verification against a real
  Java run or world save. Those comments are the punch list for wave gates — grep for them before
  declaring a wave done.
- Structure placement parity is a hard v1 requirement, which makes `oxide-structures` the highest-risk
  crate: it depends on RNG exactness, biome resolution, and chunkgen all being correct first.
