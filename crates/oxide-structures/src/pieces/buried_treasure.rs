//! Buried treasure: 26.2's `BuriedTreasureStructure` and `BuriedTreasurePieces`.

use oxide_core::rotation::Direction;
use oxide_core::{BlockPos, BlockState, HeightmapType, RandomSource};

use super::swamp_hut::chunk_center_on;
use super::{create_chest, GeneratedStructure, GenerationContext, PieceWorld, StructurePiece};
use crate::BoundingBox;

/// The blocks the chest may rest on.
const BASES: [&str; 5] = ["sandstone", "stone", "andesite", "granite", "diorite"];

#[derive(Debug, Clone)]
pub struct BuriedTreasurePiece {
    column: BlockPos,
}

/// `BuriedTreasureStructure.findGenerationPoint`: block (9, 90, 9) of the start chunk.
pub fn generate(ctx: &GenerationContext<'_>) -> GeneratedStructure {
    let column = BlockPos::new(ctx.chunk.min_block_x() + 9, 90, ctx.chunk.min_block_z() + 9);
    GeneratedStructure {
        position: chunk_center_on(ctx, HeightmapType::OceanFloorWg),
        pieces: vec![Box::new(BuriedTreasurePiece { column })],
        after_place: None,
    }
}

/// `isAir() || isLiquid` as the piece tests it: water or lava blocks, not waterlogging.
fn is_open(world: &PieceWorld<'_, '_>, pos: BlockPos) -> bool {
    world.is_air(pos) || world.is_block(pos, "water") || world.is_block(pos, "lava")
}

fn relative(pos: BlockPos, direction: Direction) -> BlockPos {
    BlockPos::new(
        pos.x + direction.step_x(),
        pos.y + direction.step_y(),
        pos.z + direction.step_z(),
    )
}

impl StructurePiece for BuriedTreasurePiece {
    fn bounding_box(&self) -> BoundingBox {
        let c = self.column;
        BoundingBox::new(c.x, c.y, c.z, c.x, c.y, c.z)
    }

    // The column and every neighbour it touches lie in the start chunk, so this runs once, there.
    fn post_process(
        &self,
        world: &mut PieceWorld<'_, '_>,
        random: &mut dyn RandomSource,
        chunk_box: &BoundingBox,
        _reference: BlockPos,
    ) {
        let (x, z) = (self.column.x, self.column.z);
        let mut pos = BlockPos::new(x, world.height(HeightmapType::OceanFloorWg, x, z), z);
        while pos.y > world.min_y() {
            let below = BlockPos::new(x, pos.y - 1, z);
            if !BASES.iter().any(|base| world.is_block(below, base)) {
                pos.y -= 1;
                continue;
            }
            let below_state = world.region.block(below).cloned();
            let soft = match world.region.block(pos) {
                Some(state) if !is_open(world, pos) => state.clone(),
                _ => world.state("sand", &[]),
            };
            for direction in Direction::VALUES {
                let side = relative(pos, direction);
                if !is_open(world, side) {
                    continue;
                }
                let under_open = is_open(world, BlockPos::new(side.x, side.y - 1, side.z));
                let state: BlockState = match &below_state {
                    Some(base) if under_open && direction != Direction::Up => base.clone(),
                    _ => soft.clone(),
                };
                world.set_block(side, state);
            }
            create_chest(
                world,
                chunk_box,
                random,
                pos,
                "minecraft:chests/buried_treasure",
                None,
            );
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use oxide_core::{ChunkPos, WorldgenRandom};

    use super::super::swamp_hut::tests::{block_at, facts, place};
    use super::*;

    #[test]
    fn chest_settles_on_the_first_stone_under_the_sand() {
        let Some(facts) = facts() else { return };
        let structure = GeneratedStructure {
            position: BlockPos::new(8, 40, 8),
            pieces: vec![Box::new(BuriedTreasurePiece {
                column: BlockPos::new(9, 90, 9),
            })],
            after_place: None,
        };
        let (writes, extras, _) = place(
            facts,
            &structure,
            ChunkPos::new(0, 0),
            42,
            (40, "sand", "water"),
        );
        // Sand fills y 36..=39 over stone: the chest replaces the lowest sand, at y 36.
        let chest = BlockPos::new(9, 36, 9);
        assert_eq!(
            block_at(&writes, chest).map(|s| s.name.path()),
            Some("chest")
        );
        let seed = WorldgenRandom::new(42).next_long();
        let entity = extras.block_entities.iter().find(|e| e.pos == chest);
        assert_eq!(
            entity.map(|e| e.snbt.clone()),
            Some(oxide_chunkgen::decorator::block_entity::with_loot_table(
                "minecraft:chest",
                chest,
                "minecraft:chests/buried_treasure",
                seed
            ))
        );
    }
}
