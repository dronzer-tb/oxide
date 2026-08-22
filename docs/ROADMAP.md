# Roadmap

Maps PRD §6 milestones onto the crate graph. Waves are the parallelism boundary — everything in a
wave can be built concurrently; a wave starts when the previous one compiles and its tests pass.

| Wave | Crates | PRD milestone | Status |
|---|---|---|---|
| 0 | `oxide-core` | 1 | done — `9b1c332`, 43 tests |
| 1 | `oxide-datapack`, `oxide-anvil`, CI | 1, 2 | done — `d727fb0`, `f401a90`, `3789e86` |
| 2 | `oxide-noise` | 1 | done — `6974f82`, 21 tests, **verified vs real Java 26.2** (2026-08-22, see below) |
| 3 | `oxide-biome`, `oxide-chunkgen` | 1 | done (scoped) — `b3f5671`, 9 tests. Terrain fill + heightmaps + biome grid only; aquifers/ore veins/carvers/surface rules deferred, see `fill.rs` |
| 4 | `oxide-structures`, `oxide-harness` | 3, 4 | done (scoped) — `0a3893c`, 21 tests, placement **verified vs real Java 26.2**. random_spread placement + weighted selection only; concentric_rings/frequency reduction/exclusion zones/piece layout deferred. Harness self-checks only — no vanilla reference data to diff against |
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

Was blocked on two things only the user could supply: a real `--data-version` from a 26.2
export, and a verified `foliaApiVersion` in `plugin/gradle.properties`. Both resolved: the real
server jar's `version.json` (`world_version`) gives DataVersion **4903**; `foliaApiVersion` was
set to a real, resolvable `dev.folia:folia-api` coordinate on the `plugin/confirm-folia-26.2-api`
branch (not yet merged to `main` as of this note).

## 2026-08-22: real Java verification (user-supplied `server.jar`)

The user supplied the real Mojang 26.2 server jar. Decompiled the relevant classes (CFR,
targeted extraction — `Xoroshiro128PlusPlus`, `XoroshiroRandomSource`, `LegacyRandomSource`,
`RandomSupport`, `WorldgenRandom`, `ImprovedNoise`, `PerlinNoise`, `NormalNoise`, `BlendedNoise`,
`DensityFunction(s)`, `CubicSpline`, `Heightmap`, `RandomState`, `Noises`,
`RandomSpreadStructurePlacement`, `StructurePlacement`, `Mth`) and cross-checked every
`PARITY-CHECK`-flagged constant/formula against it. Also confirmed the real `world_version`
(DataVersion) from `version.json` inside the jar: **4903** — resolves the "no confirmed
`--data-version`" blocker on `oxide-pregen` (see the v0 smoke test section above).

Where source alone wasn't proof enough, transcribed the confirmed algorithm standalone and ran
it on real OpenJDK 25 to capture exact-value test vectors (see `oxide-core`/`oxide-noise`/
`oxide-structures` test modules) — the same standard `LegacyRandom`'s tests already met.

**Bugs found and fixed** (all previously self-consistency-tested only, so nothing caught them):

- `Xoroshiro128PlusPlus::next_int_bounded` used a 31-bit Lemire scheme copied from
  `LegacyRandom`'s shape; vanilla's is a 32-bit scheme over `nextInt()`. This was the highest-impact
  bug — it's what `ImprovedNoise`'s permutation-table shuffle calls for every octave whenever
  `legacy_random_source` is `false` (the default), so every non-legacy noise instance in the
  entire tree was affected.
- `Xoroshiro128PlusPlus::next_double` reused `LegacyRandom`'s two-call 26+27-bit split; vanilla's
  is one call extracting the top 53 bits, times a Xoroshiro-specific `DOUBLE_UNIT` constant
  (`(double) 1.110223E-16f`, not the exact `2^-53` `LegacyRandom` uses).
- `Xoroshiro128PlusPlus::next_boolean` read the high bit of `next_bits(1)`; vanilla reads the low
  bit of a fresh `nextLong()`.
- `LegacyPositionalRandomFactory::from_hash_of` used MD5; vanilla uses Java's
  `String.hashCode()`. MD5 is Xoroshiro's `fromHashOf` only — that one was already correct.
- `NormalNoise`'s `valueFactor` formula was a guess (`(10/6)/span`); real formula is
  `(1/6) / (0.1 * (1 + 1/(span+1)))`.
- `oxide-noise`'s density-function interpreter was missing the `* 4.0` on `Shift`/`ShiftA`/
  `ShiftB` entirely.
- `PerlinNoise` had a fabricated `fix_y`/`-noise.yo()` branch that doesn't exist anywhere in
  vanilla — invented, not reconstructed; removed.
- `oxide-structures` only implemented `linear` spread; `triangular` (`(nextInt+nextInt)/2`) is
  now implemented too.

**Confirmed correct as originally written** (no change needed): the Xoroshiro seed-upgrade path
(`upgradeSeedTo128bit`, including the confusingly-named-but-numerically-right silver/golden
ratio constants), `stafford_mix13`'s constants, `Mth.getSeed`'s position-mix formula,
`XoroshiroPositionalRandomFactory`'s `at`/`from_hash_of`/`fork_positional`,
`LegacyPositionalRandomFactory`'s `at`/`fork_positional`, `ImprovedNoise`'s entire algorithm
(gradient table, permutation shuffle shape, trilinear interpolation), `PerlinNoise`'s
`lowestFreqInputFactor`/`lowestFreqValueFactor` formulas and octave-RNG derivation, the density
interpreter's arithmetic/clamp/spline/Y-clamped-gradient/squeeze nodes, `oxide-chunkgen`'s
heightmap predicates, and `oxide-biome`'s climate-axis-to-router-slot mapping.

**Confirmed out of scope, not a bug**: `minecraft:weird_scaled_sampler` doesn't exist in 26.2's
`DensityFunctions` at all (grepped the full decompiled class list) — that code path is dead
against a real datapack. `old_blended_noise`'s real algorithm (`BlendedNoise`) was read in full;
it's real and implementable (three `PerlinNoise` instances via the *legacy* init path + `Mth.
clampedLerp`) but stays a deliberate scope cut.

**Still unverified** (not checked this pass): `oxide-anvil`'s `isLightOn` relight key and
`ChunkStatus` NBT strings, `oxide-pregen`'s world bounds/block-array index order,
`oxide-datapack`'s JSON schema guesses (`Shift`/`ShiftA`/`ShiftB` field shapes turned out
right; others like `weird_scaled_sampler`'s shape are moot per above), `next_gaussian`'s
`ln`/`sqrt` platform-exactness, and everything in `oxide-structures`/`oxide-chunkgen` marked as
a deliberate scope cut rather than PARITY-CHECK.

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

- **26 `PARITY-CHECK` markers** are open across the crates (`grep -rn "PARITY-CHECK" crates/`) —
  down from the RNG core after 2026-08-22's verification pass (see above); both `LegacyRandom`
  and `Xoroshiro128PlusPlus` are now checked against real JDK 25 output, not just each other.
  `oxide-anvil`'s `isLightOn` relight key and `ChunkStatus` NBT strings, and `oxide-pregen`'s
  world bounds and block-array index order, remain conventions rather than confirmed 26.2 facts —
  the 2026-08-22 pass verified the noise/RNG/structure-placement core, not the NBT/anvil layer.
- **Minecraft 26.2 specifics are not assumed.** The data-driven design exists precisely so that no
  26.2 constant, DataVersion, or noise parameter is written from memory. Anything a crate could not
  confirm is marked with a `// PARITY-CHECK:` comment naming what needs verification against a real
  Java run or world save. Those comments are the punch list for wave gates — grep for them before
  declaring a wave done.
- Structure placement parity is a hard v1 requirement. `random_spread` placement-chunk selection
  is now verified bit-exact (2026-08-22), but the crate's higher-risk remaining surface —
  frequency reduction, exclusion zones, and especially jigsaw piece layout — is still unbuilt.
