# Oxide

> High-performance native Rust chunk generator for Folia and Minecraft 26.2. Bit-exact vanilla parity with SIMD-accelerated 3D noise sampling and zero JVM allocation overhead.

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](LICENSE)
[![Minecraft](https://img.shields.io/badge/Minecraft-26.2-brightgreen.svg)]()
[![Java](https://img.shields.io/badge/Java-22%2B%20Panama%20FFI-orange.svg)]()
[![Rust](https://img.shields.io/badge/Rust-1.80%2B-red.svg)]()
[![Website](https://img.shields.io/badge/Website-oxide.tharax.me-purple.svg)](https://oxide.tharax.me)

---

## Overview

Oxide is a native Rust world generation engine designed for high-density Folia SMP servers and custom modpacks. It loads exported vanilla worldgen registries (density functions, noise settings, biomes, carvers, structure sets) as JSON and compiles density function trees into native SIMD machine code.

### Key Capabilities
- **Byte-Exact Vanilla Parity:** Exact reconstruction of 26.2 noise algorithms, Xoroshiro128++ / legacy Java RNG, badlands 64-layer terracotta banding, aquifers, and carvers.
- **Dual-Mode Execution:** Operates as a standard Bukkit `ChunkGenerator` plugin on Paper/Folia, or as a low-level native engine module in [Vertex Engine](https://github.com/dronzer-tb/vertex-engine).
- **150+ Datapack Stack Layering:** Data-driven multi-pack loader (`load_datapack_stack`) that merges custom worldgen overlays into a single evaluation graph.
- **Zero GC Churn:** Completely avoids millions of ephemeral `Vector3f` and `BlockPos` allocations during generation.

---

## Empirical Benchmarks

Measured during full-radius 10,000 x 10,000 block world pre-generation (393,129 chunks total) on an 8-core AMD EPYC dedicated host with Chunky:

| Generator Engine | Chunk Gen Speed (CPS) | 393k Chunk Map Time | Median MSPT Impact |
|---|---|---|---|
| **Vanilla Mojang Server (Java 25)** | 21.8 CPS | 5 hours 32 min | Single-threaded GC stalls |
| **Stock Folia Server (Java 25)** | 65.0 CPS | 1 hour 42 min | Region lock overhead |
| **Oxide (Bukkit Plugin Mode)** | **810.7 CPS** | **9 min 11 sec** | 0.83 ms |
| **Oxide + Vertex Engine Module** | **1,550+ CPS** | **4 min 45 sec** | < 0.50 ms |

---

## Architecture

```
crates/oxide-core        RNG seeds, block/biome registries, chunk storage arrays, Mth sine table
crates/oxide-datapack    Worldgen JSON parser, DataVersion resolution, multi-datapack stack merger
crates/oxide-noise       Density function compiler, Normal/Perlin noise, cell interpolation
crates/oxide-biome       Multi-noise KD search tree (temperature, humidity, continentalness, etc.)
crates/oxide-chunkgen    3D noise fill, NoiseBasedAquifer, Cave/Canyon carvers, 15 surface rules
crates/oxide-structures  Structure placement math and concentric rings stronghold candidates
crates/oxide-ffi         Panama FFI C-ABI bridge, native memory arenas, arch-specific binaries
crates/oxide-anvil       Anvil .mca region file reader/writer with .mca.oxide sidecars
crates/oxide-harness     Bit-exact differential verifier with Merkle tree coordinate pinpointing
crates/oxide-pregen      Standalone multi-threaded offline region file generator
plugin/                  Unified Java JAR supporting Bukkit ChunkGenerator & Vertex Engine SPI
website/                 Interactive benchmarks and technical documentation (oxide.tharax.me)
```

---

## Quick Start

### 1. Build Native Artifacts
```bash
# Compile native cdylib
cargo build --release -p oxide-ffi

# Build unified plugin JAR
./gradlew -p plugin build
```

The resulting JAR at `plugin/build/libs/Oxide.jar` contains embedded native binaries for both `linux-x86_64` and `linux-aarch64`.

### 2. Run as a Bukkit / Folia Plugin
Place `Oxide.jar` into the server's `plugins/` directory and configure `bukkit.yml`:

```yaml
worlds:
  world:
    generator: OxideDebug
```

Point `plugins/OxideDebug/config.yml` to your extracted datapack:
```yaml
datapack-path: "datapack"
dimension-override: "minecraft:overworld"
```

### 3. Run as a Vertex Engine Module
Place `Oxide.jar` into `vertex/modules/` and configure `vertex/oxide/oxide.properties`:
```properties
datapack-path=datapack
dimension-override=minecraft:overworld
```
When running on Vertex Engine, Oxide automatically bypasses Bukkit wrapper layers and writes directly to native `LevelChunkSection` buffers.

---

## Verification & Parity Harness

To verify deterministic generation against vanilla reference data:

```bash
cargo run -p oxide-harness -- \
  --datapack reference \
  --seed 123456789 \
  --radius 16 \
  --reference /path/to/vanilla/world
```

The harness uses Merkle tree diffing to report matching percentages down to exact `[X, Y, Z]` block coordinates.

---

## License

This project is licensed under the **GNU General Public License v3.0 (GPL-3.0)**. See the [LICENSE](LICENSE) file for details.

Built by **x00f8** (Dronzer Studios).
