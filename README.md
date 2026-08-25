# Oxide

> A Rust chunk generator for Folia, aiming to be bit-for-bit identical to vanilla Minecraft `26.2`
> and faster than it.

## What it does

Rust generates the terrain; Java places it. Oxide loads the vanilla-exported worldgen registries —
noise settings, density functions, biomes, structure sets — as JSON and interprets them at
runtime, so a version bump is a data re-export rather than a code port.

The goal is not "close enough". It is the same blocks in the same places for the same seed, which
is why RNG parity is the foundation: `Xoroshiro128PlusPlus` and legacy `java.util.Random` are
verified against real Java output, not against a second Rust implementation.

Runs two ways from one jar:

- **as a Bukkit plugin** on any Paper or Folia server, through `ChunkGenerator`;
- **as a [Vertex Engine](https://github.com/dronzer-tb/vertex-engine) module**, one stage lower,
  where it writes blocks *and* biomes from a single generation call. A `ChunkGenerator` cannot
  write the biome grid at all, so the plugin path has to answer biomes again per position.

## Status

Implemented and running: noise terrain, aquifers, ore veins, surface rules, carvers, multi-noise
biomes, and heightmaps. Overworld and nether.

Not implemented: **features and decoration** (no trees, no vegetation, no ore *features*),
**structures** beyond placement selection, and **the End**. Those still come from vanilla, which
means a world generated today is Oxide terrain with vanilla decoration on top.

Parity is **not yet verified**. The harness can diff against a real vanilla world (see below), but
no reference world has been run through it yet, so no claim of bit-exactness is currently
supported by evidence. Known divergences are tracked in `docs/ARCHITECTURE.md`.

Speed, measured on an idle machine, single-threaded, 289 chunks, versus the same three stages in
Folia — treat as provisional until re-run on quiet hardware:

| | ms/chunk |
| --- | --- |
| Folia, noise + surface + carvers | 42.1 |
| Oxide, same three stages, in-process Rust only | ~25 |

In-server the marshalling boundary currently costs more than the generation does. That is the
open problem, not the generator.

## Quick start

Build the native library and the jar:

```bash
cargo build --release -p oxide-ffi
gradle -p plugin build
```

Drop `plugin/build/libs/oxide-debug-plugin-*.jar` into `plugins/`, then point it at an extracted
vanilla datapack in `plugins/OxideDebug/config.yml`:

```yaml
datapack-path: /path/to/extracted/datapack
```

See `docs/REFERENCE_DATA.md` for producing that datapack. It is Mojang-derived and must stay
local — it is gitignored and must never be committed or redistributed.

To run it as a Vertex Engine module instead, put the same jar in `vertex/modules/` and configure
`vertex/oxide/oxide.properties`:

```properties
datapack-path=/path/to/extracted/datapack
```

Installed in both places, it still generates once: the plugin half stands down when the engine
reports that terrain is already hooked.

## Checking parity against vanilla

`oxide-harness` diffs generated chunks against a world a real server produced, and reports the
difference down to world block coordinates:

```bash
cargo run -p oxide-harness -- \
  --datapack reference \
  --seed <the world's seed> \
  --radius 8 \
  --reference /path/to/vanilla/world
```

```
chunk -1,-1: 33564 block(s) differ from vanilla in section(s) [-4, -3, -2, -1]
  -16 -63 -16: oxide minecraft:deepslate[axis=y] / vanilla minecraft:stone
```

A Merkle tree narrows a mismatch from the chunk to a section, the section to a leaf, and the leaf
to individual blocks, so the output names a place to teleport to. Without `--reference` the
harness reports self-consistency only — it re-generates every chunk and compares, which catches
non-determinism without needing Java.

## Repo layout

```
crates/oxide-core        RNG, block/biome identity, chunk storage, Mth's sine table
crates/oxide-datapack    worldgen registry parsing
crates/oxide-noise       density functions, Perlin, the compiled evaluator
crates/oxide-biome       multi-noise biome search
crates/oxide-chunkgen    fill, aquifers, ore veins, surface rules, carvers
crates/oxide-structures  structure set placement
crates/oxide-ffi         the cdylib Java calls
crates/oxide-anvil       region file read/write
crates/oxide-harness     vanilla-parity differ and invariant checks
crates/oxide-pregen      offline region writer
plugin/                  Bukkit plugin and Vertex Engine module, one jar
docs/                    ARCHITECTURE.md, ROADMAP.md, REFERENCE_DATA.md
```

## Rules this repo holds itself to

- **No Minecraft constant is written from memory.** Every DataVersion, noise parameter and block
  id is read from exported datapack metadata at load time.
- **Parity claims are measured, not asserted.** Where a formula is ported, it is checked against
  real Java output — see the `Oracle.java` files under `tests/data/`, which are standalone
  transcriptions of decompiled sources and run on a plain JDK.
- **A missing algorithm says so.** Code that cannot answer returns "unsupported" rather than a
  plausible guess, because a silently different world is worse than a visibly incomplete one.
- `anyhow` at binaries and boundaries, `thiserror` for library errors, no `unwrap()` in
  generation-reachable code, `rayon` for parallelism, no `unsafe` outside `oxide-ffi`.

## Licence

MIT OR Apache-2.0.
