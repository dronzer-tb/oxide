//! Deliberately fake, deliberately non-vanilla stub chunk generator.
//!
//! `oxide-noise`, `oxide-biome`, and `oxide-chunkgen` are still empty stubs (see
//! `docs/ARCHITECTURE.md`) — there is no real terrain generation in this repo yet. This module is
//! not a placeholder for that; it exists to give `oxide-pregen` something deterministic to write
//! so the *plumbing* downstream of generation (chunk NBT shape, `.mca` writing, provenance,
//! heightmaps, whether a real client loads the result) can be exercised end-to-end. The terrain
//! pattern is chosen specifically to be unmistakable from vanilla — a whole-chunk checkerboard —
//! so nobody mistakes this output for parity work. Do not add noise, biome climate logic, or
//! anything that reaches for realism here.

use std::collections::HashMap;

use clap::ValueEnum;

use oxide_core::{
    BlockState, ChunkData, ChunkPos, ChunkSection, ChunkStatus, Heightmap, HeightmapType,
    ResourceLocation,
};

/// World vertical bounds this stub assumes.
///
/// // PARITY-CHECK: not derived from any datapack — `oxide-datapack` doesn't exist yet, so there
/// // is nothing to read a real `noise_settings` min_y/height from. `-64`/`384` is the modern
/// // default Overworld shape and matches the placeholder bounds `oxide-anvil`'s own tests use;
/// // replace with the real value read from the loaded noise settings once that path exists, per
/// // `docs/ARCHITECTURE.md`'s "never hardcoded" rule for 26.2 specifics.
pub const WORLD_MIN_Y: i32 = -64;
pub const WORLD_HEIGHT: i32 = 384;

/// Bedrock: exactly one layer, at the world floor.
const BEDROCK_Y: i32 = WORLD_MIN_Y;
/// Solid stone fill from just above bedrock up to (and including) this Y — "modest height",
/// arbitrary but deterministic.
const STONE_TOP_Y: i32 = WORLD_MIN_Y + 63;
/// One surface layer, then air above.
const SURFACE_Y: i32 = STONE_TOP_Y + 1;

/// Stub terrain pattern, selectable via `--pattern`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Pattern {
    /// Whole-chunk stone/quartz checkerboard keyed on `(chunk_x + chunk_z) % 2` — adjacent
    /// chunks alternate surface material, unmistakable from the air.
    Checkerboard,
    /// Uniform grass surface, no per-chunk variation. Still obviously stub terrain (flat, no
    /// biome-appropriate features), just without the checkerboard's chunk-boundary tells.
    Flat,
}

fn bedrock() -> BlockState {
    BlockState::new(ResourceLocation::minecraft("bedrock"))
}
fn stone() -> BlockState {
    BlockState::new(ResourceLocation::minecraft("stone"))
}
fn quartz_block() -> BlockState {
    BlockState::new(ResourceLocation::minecraft("quartz_block"))
}
fn grass_block() -> BlockState {
    BlockState::new(ResourceLocation::minecraft("grass_block"))
}
fn air() -> BlockState {
    BlockState::new(ResourceLocation::minecraft("air"))
}

/// Single valid biome id this stub fills every section with. Any real biome id is "valid" for
/// the smoke test's purposes (the point under test is that the biome palette round-trips
/// through NBT correctly, not which biome it is).
fn stub_biome() -> ResourceLocation {
    ResourceLocation::minecraft("plains")
}

fn surface_block(pattern: Pattern, pos: ChunkPos) -> BlockState {
    match pattern {
        Pattern::Flat => grass_block(),
        Pattern::Checkerboard => {
            if (pos.x + pos.z).rem_euclid(2) == 0 {
                stone()
            } else {
                quartz_block()
            }
        }
    }
}

/// The block at absolute Y in every column of this chunk. This stub is horizontally uniform
/// within a chunk — the synthetic signal is the whole-chunk pattern (see `surface_block`), not
/// per-block noise. `None` means air, which is every section's default value, so callers can
/// skip writing it.
fn block_at(y: i32, pattern: Pattern, pos: ChunkPos) -> Option<BlockState> {
    if y == BEDROCK_Y {
        Some(bedrock())
    } else if y <= STONE_TOP_Y {
        Some(stone())
    } else if y == SURFACE_Y {
        Some(surface_block(pattern, pos))
    } else {
        None
    }
}

/// Local-index convention for a 16x16x16 section's flattened block/biome arrays: `(y*16+z)*16+x`
/// (Y outer, Z middle, X inner) — vanilla's own flattened block-array order since the palette
/// format was introduced.
///
/// // PARITY-CHECK: this ordering is not asserted anywhere else in this repo (`PalettedContainer`
/// // is index-order-agnostic; it's the caller's job to pick one and be consistent). Verify
/// // against a real 26.2 chunk NBT dump before trusting a client to render this correctly.
fn local_index(local_x: usize, local_y: usize, local_z: usize) -> usize {
    (local_y * 16 + local_z) * 16 + local_x
}

/// True if the block at this index should count toward heightmap type `ty`. This stub's palette
/// (bedrock/stone/quartz/grass/air) never includes fluid or leaves, so every type currently
/// reduces to "not air" — kept as distinct match arms (rather than one shared check) so a future
/// stub block palette makes each heightmap type diverge correctly instead of silently staying
/// wrong.
fn counts_for_heightmap(ty: HeightmapType, block: &BlockState) -> bool {
    let path = block.name.path();
    let is_air = path == "air";
    let is_fluid = path == "water" || path == "lava";
    let is_leaves = path.contains("leaves");
    match ty {
        HeightmapType::WorldSurface | HeightmapType::WorldSurfaceWg => !is_air,
        HeightmapType::OceanFloor | HeightmapType::OceanFloorWg => !is_air && !is_fluid,
        HeightmapType::MotionBlocking => !is_air && !is_fluid,
        HeightmapType::MotionBlockingNoLeaves => !is_air && !is_fluid && !is_leaves,
    }
}

/// Highest absolute Y in column `(x, z)` whose block counts for heightmap type `ty`, scanning
/// the chunk's actual placed sections top-down. `None` if no block in the column counts.
fn column_top_y(chunk: &ChunkData, x: usize, z: usize, ty: HeightmapType) -> Option<i32> {
    for section in chunk.sections.iter().rev() {
        for local_y in (0..16usize).rev() {
            let block = section.block_states.get(local_index(x, local_y, z));
            if counts_for_heightmap(ty, block) {
                let section_min_y = (section.y as i32) * 16;
                return Some(section_min_y + local_y as i32);
            }
        }
    }
    None
}

/// Computes all six vanilla heightmap types from the blocks actually placed in `chunk` — never
/// hardcoded. Value stored is one above the highest counting block (vanilla's convention: the
/// heightmap gives the Y you'd stand on), `0` (chunk floor) if a column has no counting block.
fn compute_heightmaps(chunk: &ChunkData) -> HashMap<HeightmapType, Heightmap> {
    const TYPES: [HeightmapType; 6] = [
        HeightmapType::WorldSurface,
        HeightmapType::WorldSurfaceWg,
        HeightmapType::OceanFloor,
        HeightmapType::OceanFloorWg,
        HeightmapType::MotionBlocking,
        HeightmapType::MotionBlockingNoLeaves,
    ];

    let mut out = HashMap::new();
    for ty in TYPES {
        let mut hm = Heightmap::new(chunk.height);
        for x in 0..16usize {
            for z in 0..16usize {
                let relative = match column_top_y(chunk, x, z, ty) {
                    Some(y) => y + 1 - chunk.min_y,
                    None => 0,
                };
                hm.set(x, z, relative);
            }
        }
        out.insert(ty, hm);
    }
    out
}

/// Builds one stub-generated chunk column at `pos`: bedrock floor, solid stone fill, one surface
/// layer patterned per `pattern`, air above. Deterministic, no noise library involved. Heightmaps
/// are computed from the blocks actually placed. The chunk is marked `needs_relight = true` — per
/// `docs/ARCHITECTURE.md`'s "Chunk output contract", Rust-generated chunks never carry
/// self-computed lighting.
pub fn build_chunk(pos: ChunkPos, pattern: Pattern) -> ChunkData {
    let mut chunk = ChunkData::new(pos, WORLD_MIN_Y, WORLD_HEIGHT);
    let biome = stub_biome();

    for i in 0..chunk.section_count() {
        let section_y = WORLD_MIN_Y / 16 + i as i32;
        let mut section = ChunkSection::new(section_y as i8, air(), biome.clone());
        let section_min_y = section_y * 16;

        for local_y in 0..16i32 {
            let Some(block) = block_at(section_min_y + local_y, pattern, pos) else {
                continue; // air is the section's default value already.
            };
            for local_z in 0..16usize {
                for local_x in 0..16usize {
                    section.block_states.set(
                        local_index(local_x, local_y as usize, local_z),
                        block.clone(),
                    );
                }
            }
        }
        chunk.sections.push(section);
    }

    chunk.status = ChunkStatus::Full;
    chunk.needs_relight = true;
    chunk.heightmaps = compute_heightmaps(&chunk);
    chunk
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bedrock_at_world_floor() {
        let chunk = build_chunk(ChunkPos::new(0, 0), Pattern::Flat);
        let bottom = chunk.sections.first().unwrap();
        let block = bottom.block_states.get(local_index(0, 0, 0));
        assert_eq!(block.name.path(), "bedrock");
    }

    #[test]
    fn top_of_world_is_air() {
        let chunk = build_chunk(ChunkPos::new(0, 0), Pattern::Flat);
        let top = chunk.sections.last().unwrap();
        let block = top.block_states.get(local_index(0, 15, 0));
        assert_eq!(block.name.path(), "air");
    }

    #[test]
    fn checkerboard_alternates_on_chunk_parity() {
        assert_eq!(
            surface_block(Pattern::Checkerboard, ChunkPos::new(0, 0))
                .name
                .path(),
            "stone"
        );
        assert_eq!(
            surface_block(Pattern::Checkerboard, ChunkPos::new(1, 0))
                .name
                .path(),
            "quartz_block"
        );
        assert_eq!(
            surface_block(Pattern::Checkerboard, ChunkPos::new(1, 1))
                .name
                .path(),
            "stone"
        );
        // Negative coordinates must use the same parity as their positive counterparts modulo 2.
        assert_eq!(
            surface_block(Pattern::Checkerboard, ChunkPos::new(-1, 0))
                .name
                .path(),
            "quartz_block"
        );
    }

    #[test]
    fn flat_pattern_has_no_chunk_variation() {
        let a = surface_block(Pattern::Flat, ChunkPos::new(0, 0));
        let b = surface_block(Pattern::Flat, ChunkPos::new(7, -3));
        assert_eq!(a, b);
        assert_eq!(a.name.path(), "grass_block");
    }

    #[test]
    fn heightmap_matches_surface_y() {
        let chunk = build_chunk(ChunkPos::new(2, 2), Pattern::Checkerboard);
        let hm = chunk.heightmaps.get(&HeightmapType::WorldSurface).unwrap();
        let expected_relative = SURFACE_Y + 1 - WORLD_MIN_Y;
        for x in 0..16 {
            for z in 0..16 {
                assert_eq!(hm.get(x, z), expected_relative);
            }
        }
    }

    #[test]
    fn all_six_heightmap_types_present_and_agree_for_this_palette() {
        let chunk = build_chunk(ChunkPos::new(0, 0), Pattern::Checkerboard);
        assert_eq!(chunk.heightmaps.len(), 6);
        let world_surface = chunk.heightmaps.get(&HeightmapType::WorldSurface).unwrap();
        for ty in [
            HeightmapType::WorldSurfaceWg,
            HeightmapType::OceanFloor,
            HeightmapType::OceanFloorWg,
            HeightmapType::MotionBlocking,
            HeightmapType::MotionBlockingNoLeaves,
        ] {
            let other = chunk.heightmaps.get(&ty).unwrap();
            for x in 0..16 {
                for z in 0..16 {
                    assert_eq!(other.get(x, z), world_surface.get(x, z));
                }
            }
        }
    }

    #[test]
    fn needs_relight_is_always_set() {
        let chunk = build_chunk(ChunkPos::new(0, 0), Pattern::Flat);
        assert!(chunk.needs_relight);
        assert_eq!(chunk.status, ChunkStatus::Full);
    }
}
