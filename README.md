# Oxide

Hybrid Rust/Java chunk generator for Folia, targeting Minecraft `26.2`. Rust does the terrain
generation; a thin Folia plugin routes and validates against it.

## What it does

Vanilla worldgen is data-driven, not hardcoded: Rust loads the vanilla-exported worldgen
registries (noise settings, density functions, biomes, structure sets) as JSON and interprets
them at runtime. A version bump is a data re-export, not a code port. Every downstream property —
surface rules, carvers, ore placement, structure placement — depends on RNG being bit-exact with
Java's `Xoroshiro128PlusPlus` and legacy `java.util.Random`, so RNG parity is the first thing
built and the most heavily tested.

Chunks Rust writes are marked needs-relight, not self-lit. A Merkle-tree validation harness diffs
generated chunks against a vanilla reference, bucketed by biome and structure proximity, to catch
parity regressions early.

See `docs/ARCHITECTURE.md` for the crate graph and design calls, `docs/ROADMAP.md` for wave
status, `docs/REFERENCE_DATA.md` for extracting vanilla reference data.

## Status

Waves 0–1 done (`oxide-core`, `oxide-datapack`, `oxide-anvil`). No real terrain generation yet —
`oxide-noise`, `oxide-biome`, `oxide-chunkgen` are wave 2–3, not started. A v0 smoke test proves
the write path only: `oxide-pregen` writes synthetic checkerboard/flat chunks, and the `plugin/`
Folia debug overlay reads back per-chunk provenance (`/oxide debug`, `/oxide here`) to confirm
which generator wrote what.

## Repo layout

```
crates/          oxide-core, oxide-datapack, oxide-noise, oxide-biome, oxide-chunkgen,
                  oxide-structures, oxide-anvil, oxide-harness, oxide-ffi, oxide-pregen
plugin/           Folia plugin: chunk provenance debug overlay
docs/             ARCHITECTURE.md, ROADMAP.md, REFERENCE_DATA.md
```

## Building

Heavy/release builds run in GitHub Actions, not locally — see `.github/workflows/`.

- `plugin/` builds against `dev.folia:folia-api` — version is a Gradle property in
  `plugin/gradle.properties`, confirmed against `repo.papermc.io`'s `maven-metadata.xml` for
  MC 26.2. `.github/workflows/plugin.yml` is `workflow_dispatch`-only until that version is stable
  upstream (currently a `26.2.build.*-beta`).
- Rust crates build via `cargo build --release` in `.github/workflows/ci.yml`.

## Non-negotiables

- No Minecraft 26.2 constant, DataVersion, or noise parameter is written from memory — everything
  is read from exported datapack metadata or extracted reference data at load time.
- `anyhow` at binaries/boundaries, `thiserror` for library error enums, no `unwrap()` in
  generation-reachable library code.
- `rayon` for parallelism; no async — generation is CPU-bound.
- No `unsafe` outside `oxide-ffi`.
