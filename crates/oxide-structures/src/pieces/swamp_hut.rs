//! Swamp huts: 26.2's `SwampHutStructure` and `SwampHutPiece`, plus the
//! `ScatteredFeaturePiece` height rule the jungle temple shares.

use oxide_core::rotation::Direction;
use oxide_core::{BlockPos, HeightmapType, RandomSource};

use super::{
    make_bounding_box, random_horizontal, GeneratedStructure, GenerationContext, PieceFrame,
    PieceWorld, StructureEntity, StructurePiece,
};
use crate::BoundingBox;

/// `Structure.onTopOfChunkCenter`'s position: the chunk's middle column, on `kind`'s first
/// occupied block.
pub(super) fn chunk_center_on(ctx: &GenerationContext<'_>, kind: HeightmapType) -> BlockPos {
    let (x, z) = (ctx.chunk.min_block_x() + 8, ctx.chunk.min_block_z() + 8);
    BlockPos::new(x, ctx.terrain.base_height(x, z, kind) - 1, z)
}

/// `ScatteredFeaturePiece.updateAverageGroundHeight` without the move: the mean
/// motion-blocking-no-leaves height of the box's columns inside `chunk_box`, or `None` when it
/// has none there.
pub(super) fn average_ground_height(
    world: &PieceWorld<'_, '_>,
    bounding_box: &BoundingBox,
    chunk_box: &BoundingBox,
) -> Option<i32> {
    let (mut total, mut count) = (0, 0);
    for z in bounding_box.min_z..=bounding_box.max_z {
        for x in bounding_box.min_x..=bounding_box.max_x {
            if chunk_box.contains_point(x, 64, z) {
                total += world.height(HeightmapType::MotionBlockingNoLeaves, x, z);
                count += 1;
            }
        }
    }
    (count > 0).then(|| total / count)
}

/// A scattered piece's frame once its height is settled at `floor`
/// (`boundingBox.move(0, heightPosition - minY + offset, 0)` with the offset folded in).
pub(super) fn settled_frame(
    bounding_box: &BoundingBox,
    facing: Direction,
    floor: i32,
) -> PieceFrame {
    PieceFrame::new(
        bounding_box.moved(0, floor - bounding_box.min_y, 0),
        Some(facing),
    )
}

#[derive(Debug, Clone)]
pub struct SwampHutPiece {
    bounding_box: BoundingBox,
    facing: Direction,
}

/// `SwampHutStructure.findGenerationPoint`: one hut at the start chunk's corner.
pub fn generate(ctx: &GenerationContext<'_>) -> GeneratedStructure {
    let mut random = ctx.random();
    let facing = random_horizontal(&mut random);
    let (west, north) = (ctx.chunk.min_block_x(), ctx.chunk.min_block_z());
    GeneratedStructure {
        position: chunk_center_on(ctx, HeightmapType::WorldSurfaceWg),
        pieces: vec![Box::new(SwampHutPiece {
            bounding_box: make_bounding_box(west, 64, north, facing, 7, 7, 9),
            facing,
        })],
        after_place: None,
    }
}

/// `SwampHutPiece.spawnedWitch`/`spawnedCat`'s mob, standing at the hut's (2, 2, 5).
///
/// PARITY-CHECK: vanilla saves the whole entity after `finalizeSpawn` (UUID, attributes, a cat's
/// biome-picked variant and collar, drawn from the level random). Only what the structure itself
/// decides is written; the loader fills the rest in as a fresh mob would.
fn mob(id: &str, pos: BlockPos) -> StructureEntity {
    let pos = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ];
    StructureEntity {
        snbt: format!(
            "{{id:\"minecraft:{id}\",Pos:[{:?}d,{:?}d,{:?}d],Rotation:[0.0f,0.0f],PersistenceRequired:1b}}",
            pos[0], pos[1], pos[2]
        ),
        pos,
    }
}

impl StructurePiece for SwampHutPiece {
    fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    // The hut (at most 9 blocks from the start chunk's corner) lies in that chunk alone, so the
    // one `postProcess` vanilla runs is this chunk's and no state carries across chunks.
    fn post_process(
        &self,
        world: &mut PieceWorld<'_, '_>,
        _random: &mut dyn RandomSource,
        chunk_box: &BoundingBox,
        _reference: BlockPos,
    ) {
        let Some(floor) = average_ground_height(world, &self.bounding_box, chunk_box) else {
            return;
        };
        let f = settled_frame(&self.bounding_box, self.facing, floor);
        let planks = world.state("spruce_planks", &[]);
        let log = world.state("oak_log", &[]);
        let fence = world.state("oak_fence", &[]);
        let air = world.state("air", &[]);
        for (min, max) in [
            ((1, 1, 1), (5, 1, 7)),
            ((1, 4, 2), (5, 4, 7)),
            ((2, 1, 0), (4, 1, 0)),
            ((2, 2, 2), (3, 3, 2)),
            ((1, 2, 3), (1, 3, 6)),
            ((5, 2, 3), (5, 3, 6)),
            ((2, 2, 7), (4, 3, 7)),
        ] {
            f.generate_box(world, chunk_box, min, max, &planks, &planks, false);
        }
        for (x, z) in [(1, 2), (5, 2), (1, 7), (5, 7)] {
            f.generate_box(world, chunk_box, (x, 0, z), (x, 3, z), &log, &log, false);
        }
        f.place(world, &fence, 2, 3, 2, chunk_box);
        f.place(world, &fence, 3, 3, 7, chunk_box);
        f.place(world, &air, 1, 3, 4, chunk_box);
        f.place(world, &air, 5, 3, 4, chunk_box);
        f.place(world, &air, 5, 3, 5, chunk_box);
        let mushroom = world.state("potted_red_mushroom", &[]);
        f.place(world, &mushroom, 1, 3, 5, chunk_box);
        let table = world.state("crafting_table", &[]);
        f.place(world, &table, 3, 2, 6, chunk_box);
        let cauldron = world.state("cauldron", &[]);
        f.place(world, &cauldron, 4, 2, 6, chunk_box);
        f.place(world, &fence, 1, 2, 1, chunk_box);
        f.place(world, &fence, 5, 2, 1, chunk_box);
        let stairs = |world: &PieceWorld<'_, '_>, facing: &str, shape: &str| {
            world.state("spruce_stairs", &[("facing", facing), ("shape", shape)])
        };
        let north = stairs(world, "north", "straight");
        let east = stairs(world, "east", "straight");
        let west = stairs(world, "west", "straight");
        let south = stairs(world, "south", "straight");
        f.generate_box(
            world,
            chunk_box,
            (0, 4, 1),
            (6, 4, 1),
            &north,
            &north,
            false,
        );
        f.generate_box(world, chunk_box, (0, 4, 2), (0, 4, 7), &east, &east, false);
        f.generate_box(world, chunk_box, (6, 4, 2), (6, 4, 7), &west, &west, false);
        f.generate_box(
            world,
            chunk_box,
            (0, 4, 8),
            (6, 4, 8),
            &south,
            &south,
            false,
        );
        for (facing, shape, x, z) in [
            ("north", "outer_right", 0, 1),
            ("north", "outer_left", 6, 1),
            ("south", "outer_left", 0, 8),
            ("south", "outer_right", 6, 8),
        ] {
            let corner = stairs(world, facing, shape);
            f.place(world, &corner, x, 4, z, chunk_box);
        }
        for z in [2, 7] {
            for x in [1, 5] {
                f.fill_column_down(world, &log, x, -1, z, chunk_box);
            }
        }
        let spot = f.world_pos(2, 2, 5);
        if chunk_box.contains(spot) {
            world.add_entity(mob("witch", spot));
            world.add_entity(mob("cat", spot));
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::path::Path;
    use std::sync::OnceLock;

    use oxide_chunkgen::{BlockWrite, ChunkExtras, DecorationRegion};
    use oxide_core::{
        BiomeId, BlockState, ChunkData, ChunkPos, ChunkSection, Heightmap, ResourceLocation,
        WorldgenRandom,
    };
    use oxide_datapack::BlockFacts;

    use super::*;
    use crate::pieces::{chunk_box, place_in_chunk};

    /// The game's exported block facts, or `None` (the test skips) without `reference/`.
    pub fn facts() -> Option<&'static BlockFacts> {
        static FACTS: OnceLock<Option<BlockFacts>> = OnceLock::new();
        FACTS
            .get_or_init(|| {
                let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference"));
                root.join("block_facts.tsv")
                    .is_file()
                    .then(|| BlockFacts::load(root).expect("block facts load"))
            })
            .as_ref()
    }

    /// `ground` for the 4 blocks under `surface` over stone, `above` higher up, in a -64..320
    /// world.
    pub fn flat_chunk(pos: ChunkPos, surface: i32, ground: &str, above: &str) -> ChunkData {
        let (min_y, height) = (-64, 384);
        let mut chunk = ChunkData::new(pos, min_y, height);
        let biome: BiomeId = ResourceLocation::minecraft("plains");
        let state = |path: &str| BlockState::new(ResourceLocation::minecraft(path));
        for index in 0..(height / 16) {
            let section_min = min_y + index * 16;
            let mut section =
                ChunkSection::new((section_min >> 4) as i8, state(above), biome.clone());
            for y in 0..16 {
                let world_y = section_min + y;
                if world_y >= surface {
                    continue;
                }
                let fill = state(if world_y >= surface - 4 {
                    ground
                } else {
                    "stone"
                });
                for i in 0..256 {
                    section
                        .block_states
                        .set((y as usize) * 256 + i, fill.clone());
                }
            }
            chunk.sections.push(section);
        }
        let mut map = Heightmap::new(height);
        for z in 0..16 {
            for x in 0..16 {
                map.set(x, z, surface - min_y);
            }
        }
        for kind in [
            HeightmapType::WorldSurface,
            HeightmapType::WorldSurfaceWg,
            HeightmapType::OceanFloor,
            HeightmapType::OceanFloorWg,
            HeightmapType::MotionBlocking,
            HeightmapType::MotionBlockingNoLeaves,
        ] {
            chunk.heightmaps.insert(kind, map.clone());
        }
        chunk
    }

    /// Places `structure` into the flat chunk `chunk`, its decoration random seeded `seed`.
    pub fn place(
        facts: &BlockFacts,
        structure: &GeneratedStructure,
        chunk: ChunkPos,
        seed: i64,
        terrain: (i32, &str, &str),
    ) -> (Vec<BlockWrite>, ChunkExtras, Vec<StructureEntity>) {
        let (surface, ground, above) = terrain;
        let chunks: Vec<ChunkData> = (-1..=1)
            .flat_map(|dz| (-1..=1).map(move |dx| ChunkPos::new(chunk.x + dx, chunk.z + dz)))
            .map(|pos| flat_chunk(pos, surface, ground, above))
            .collect();
        let refs: [&ChunkData; 9] = std::array::from_fn(|i| &chunks[i]);
        let mut region = DecorationRegion::new(chunk, refs, facts);
        let mut entities = Vec::new();
        let mut random = WorldgenRandom::new(seed);
        let area = chunk_box(chunk, region.min_y(), region.max_y());
        place_in_chunk(
            structure,
            &mut PieceWorld {
                region: &mut region,
                entities: &mut entities,
            },
            &mut random,
            &area,
        );
        let (writes, extras) = region.into_parts();
        (writes, extras, entities)
    }

    pub fn block_at(writes: &[BlockWrite], pos: BlockPos) -> Option<&BlockState> {
        writes
            .iter()
            .rev()
            .find(|(at, _)| *at == pos)
            .map(|(_, state)| state)
    }

    #[test]
    fn hut_sits_on_the_ground_and_houses_a_witch() {
        let Some(facts) = facts() else { return };
        let hut = SwampHutPiece {
            bounding_box: make_bounding_box(0, 64, 0, Direction::North, 7, 7, 9),
            facing: Direction::North,
        };
        let structure = GeneratedStructure {
            position: BlockPos::new(8, 62, 8),
            pieces: vec![Box::new(hut)],
            after_place: None,
        };
        let (writes, _, entities) = place(
            facts,
            &structure,
            ChunkPos::new(0, 0),
            0,
            (63, "grass_block", "air"),
        );
        // Facing north counts z from the box's far side: local (1, 1, 1) is world (1, 64, 7).
        assert_eq!(
            block_at(&writes, BlockPos::new(1, 64, 7)).map(|s| s.name.path()),
            Some("spruce_planks")
        );
        // The corner posts start on the ground (local y 0, the settled floor).
        assert_eq!(
            block_at(&writes, BlockPos::new(1, 63, 6)).map(|s| s.name.path()),
            Some("oak_log")
        );
        let ids: Vec<_> = entities.iter().map(|e| e.snbt.as_str()).collect();
        assert!(ids[0].starts_with("{id:\"minecraft:witch\",Pos:[2.5d,65.0d,3.5d]"));
        assert!(ids[1].starts_with("{id:\"minecraft:cat\""));
    }
}
