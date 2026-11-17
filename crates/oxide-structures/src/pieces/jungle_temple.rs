//! Jungle temples: 26.2's `JungleTempleStructure` and `JungleTemplePiece`.

use oxide_core::rotation::Direction;
use oxide_core::{BlockPos, BlockState, HeightmapType, RandomSource};

use super::swamp_hut::{average_ground_height, chunk_center_on, settled_frame};
use super::{
    make_bounding_box, random_horizontal, GeneratedStructure, GenerationContext, PieceFrame,
    PieceWorld, StructurePiece,
};
use crate::BoundingBox;

const WIDTH: i32 = 12;
const DEPTH: i32 = 15;
const CHEST: &str = "minecraft:chests/jungle_temple";
const DISPENSER: &str = "minecraft:chests/jungle_temple_dispenser";

/// `SinglePieceStructure.findGenerationPoint`: nothing when a corner of the `width` x `depth`
/// area from the chunk's corner is below sea level, else `piece` on the chunk's middle column.
pub(super) fn single_piece(
    ctx: &GenerationContext<'_>,
    width: i32,
    depth: i32,
    piece: impl FnOnce() -> Box<dyn StructurePiece>,
) -> Option<GeneratedStructure> {
    let (x, z) = (ctx.chunk.min_block_x(), ctx.chunk.min_block_z());
    // `Structure.getLowestY`: the first occupied world-surface block at the four corners.
    let lowest = [
        (x, z),
        (x, z + depth),
        (x + width, z),
        (x + width, z + depth),
    ]
    .into_iter()
    .map(|(x, z)| ctx.terrain.base_height(x, z, HeightmapType::WorldSurfaceWg) - 1)
    .min()
    .expect("four corners");
    (lowest >= ctx.terrain.sea_level()).then(|| GeneratedStructure {
        position: chunk_center_on(ctx, HeightmapType::WorldSurfaceWg),
        pieces: vec![piece()],
        after_place: None,
    })
}

#[derive(Debug, Clone)]
pub struct JungleTemplePiece {
    bounding_box: BoundingBox,
    facing: Direction,
}

/// `JungleTempleStructure.findGenerationPoint`.
pub fn generate(ctx: &GenerationContext<'_>) -> Option<GeneratedStructure> {
    single_piece(ctx, WIDTH, DEPTH, || {
        let facing = random_horizontal(&mut ctx.random());
        let (west, north) = (ctx.chunk.min_block_x(), ctx.chunk.min_block_z());
        Box::new(JungleTemplePiece {
            bounding_box: make_bounding_box(west, 64, north, facing, WIDTH, 10, DEPTH),
            facing,
        })
    })
}

/// `JungleTemplePiece.MossStoneSelector` over a box: one `nextFloat` per block, chunk or not.
fn stone_box(
    f: &PieceFrame,
    world: &mut PieceWorld<'_, '_>,
    chunk_box: &BoundingBox,
    random: &mut dyn RandomSource,
    (min, max): ((i32, i32, i32), (i32, i32, i32)),
    stones: &(BlockState, BlockState),
) {
    f.generate_box_with(
        world,
        chunk_box,
        min,
        max,
        false,
        random,
        &mut |random, _, _, _, _| {
            if random.next_float() < 0.4 {
                stones.0.clone()
            } else {
                stones.1.clone()
            }
        },
    );
}

impl StructurePiece for JungleTemplePiece {
    fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    // At most 15 blocks from the start chunk's corner, the temple lies in that chunk alone: the
    // one `postProcess` vanilla runs is this one, so its once-only chest and trap flags never
    // matter across chunks.
    fn post_process(
        &self,
        world: &mut PieceWorld<'_, '_>,
        random: &mut dyn RandomSource,
        chunk_box: &BoundingBox,
        _reference: BlockPos,
    ) {
        let Some(floor) = average_ground_height(world, &self.bounding_box, chunk_box) else {
            return;
        };
        let f = settled_frame(&self.bounding_box, self.facing, floor);
        let cb = chunk_box;
        let stones = (
            world.state("cobblestone", &[]),
            world.state("mossy_cobblestone", &[]),
        );
        let stone = |world: &mut PieceWorld<'_, '_>,
                     random: &mut dyn RandomSource,
                     min: (i32, i32, i32),
                     max: (i32, i32, i32)| {
            stone_box(&f, world, cb, random, (min, max), &stones)
        };
        let air = world.state("air", &[]);
        let mossy = world.state("mossy_cobblestone", &[]);

        stone(world, random, (0, -4, 0), (WIDTH - 1, 0, DEPTH - 1));
        for (min, max) in [
            ((2, 1, 2), (9, 2, 2)),
            ((2, 1, 12), (9, 2, 12)),
            ((2, 1, 3), (2, 2, 11)),
            ((9, 1, 3), (9, 2, 11)),
            ((1, 3, 1), (10, 6, 1)),
            ((1, 3, 13), (10, 6, 13)),
            ((1, 3, 2), (1, 6, 12)),
            ((10, 3, 2), (10, 6, 12)),
            ((2, 3, 2), (9, 3, 12)),
            ((2, 6, 2), (9, 6, 12)),
            ((3, 7, 3), (8, 7, 11)),
            ((4, 8, 4), (7, 8, 10)),
        ] {
            stone(world, random, min, max);
        }
        for (min, max) in [
            ((3, 1, 3), (8, 2, 11)),
            ((4, 3, 6), (7, 3, 9)),
            ((2, 4, 2), (9, 5, 12)),
            ((4, 6, 5), (7, 6, 9)),
            ((5, 7, 6), (6, 7, 8)),
            ((5, 1, 2), (6, 2, 2)),
            ((5, 2, 12), (6, 2, 12)),
            ((5, 5, 1), (6, 5, 1)),
            ((5, 5, 13), (6, 5, 13)),
        ] {
            f.generate_air_box(world, cb, min, max);
        }
        for (x, z) in [(1, 5), (10, 5), (1, 9), (10, 9)] {
            f.place(world, &air, x, 5, z, cb);
        }
        for z in [0, 14] {
            for x in [2, 4, 7, 9] {
                stone(world, random, (x, 4, z), (x, 5, z));
            }
        }
        stone(world, random, (5, 6, 0), (6, 6, 0));
        for x in [0, 11] {
            for z in (2..=12).step_by(2) {
                stone(world, random, (x, 4, z), (x, 5, z));
            }
            stone(world, random, (x, 6, 5), (x, 6, 5));
            stone(world, random, (x, 6, 9), (x, 6, 9));
        }
        for (min, max) in [
            ((2, 7, 2), (2, 9, 2)),
            ((9, 7, 2), (9, 9, 2)),
            ((2, 7, 12), (2, 9, 12)),
            ((9, 7, 12), (9, 9, 12)),
            ((4, 9, 4), (4, 9, 4)),
            ((7, 9, 4), (7, 9, 4)),
            ((4, 9, 10), (4, 9, 10)),
            ((7, 9, 10), (7, 9, 10)),
            ((5, 9, 7), (6, 9, 7)),
        ] {
            stone(world, random, min, max);
        }

        let stairs = |world: &PieceWorld<'_, '_>, facing: &str| {
            world.state("cobblestone_stairs", &[("facing", facing)])
        };
        let east = stairs(world, "east");
        let west = stairs(world, "west");
        let south = stairs(world, "south");
        let north = stairs(world, "north");
        for (state, x, y, z) in [
            (&north, 5, 9, 6),
            (&north, 6, 9, 6),
            (&south, 5, 9, 8),
            (&south, 6, 9, 8),
            (&north, 4, 0, 0),
            (&north, 5, 0, 0),
            (&north, 6, 0, 0),
            (&north, 7, 0, 0),
            (&north, 4, 1, 8),
            (&north, 4, 2, 9),
            (&north, 4, 3, 10),
            (&north, 7, 1, 8),
            (&north, 7, 2, 9),
            (&north, 7, 3, 10),
        ] {
            f.place(world, state, x, y, z, cb);
        }
        stone(world, random, (4, 1, 9), (4, 1, 9));
        stone(world, random, (7, 1, 9), (7, 1, 9));
        stone(world, random, (4, 1, 10), (7, 2, 10));
        stone(world, random, (5, 4, 5), (6, 4, 5));
        f.place(world, &east, 4, 4, 5, cb);
        f.place(world, &west, 7, 4, 5, cb);
        for i in 0..4 {
            f.place(world, &south, 5, -i, 6 + i, cb);
            f.place(world, &south, 6, -i, 6 + i, cb);
            f.generate_air_box(world, cb, (5, -i, 7 + i), (6, -i, 9 + i));
        }
        f.generate_air_box(world, cb, (1, -3, 12), (10, -1, 13));
        f.generate_air_box(world, cb, (1, -3, 1), (3, -1, 13));
        f.generate_air_box(world, cb, (1, -3, 1), (9, -1, 5));
        for z in (1..=13).step_by(2) {
            stone(world, random, (1, -3, z), (1, -2, z));
        }
        for z in (2..=12).step_by(2) {
            stone(world, random, (1, -1, z), (3, -1, z));
        }
        stone(world, random, (2, -2, 1), (5, -2, 1));
        stone(world, random, (7, -2, 1), (9, -2, 1));
        stone(world, random, (6, -3, 1), (6, -3, 1));
        stone(world, random, (6, -1, 1), (6, -1, 1));

        let hook = |world: &PieceWorld<'_, '_>, facing: &str| {
            world.state("tripwire_hook", &[("facing", facing), ("attached", "true")])
        };
        let wire = |world: &PieceWorld<'_, '_>, sides: &[&str]| {
            let props: Vec<(&str, &str)> = sides
                .iter()
                .map(|side| (*side, "true"))
                .chain([("attached", "true")])
                .collect();
            world.state("tripwire", &props)
        };
        let redstone = |world: &PieceWorld<'_, '_>, sides: &[(&str, &str)]| {
            world.state("redstone_wire", sides)
        };
        let hook_east = hook(world, "east");
        f.place(world, &hook_east, 1, -3, 8, cb);
        let hook_west = hook(world, "west");
        f.place(world, &hook_west, 4, -3, 8, cb);
        let wire_ew = wire(world, &["east", "west"]);
        f.place(world, &wire_ew, 2, -3, 8, cb);
        f.place(world, &wire_ew, 3, -3, 8, cb);
        let redstone_ns = redstone(world, &[("north", "side"), ("south", "side")]);
        for z in 2..=7 {
            f.place(world, &redstone_ns, 5, -3, z, cb);
        }
        let redstone_nw = redstone(world, &[("north", "side"), ("west", "side")]);
        f.place(world, &redstone_nw, 5, -3, 1, cb);
        let redstone_ew = redstone(world, &[("east", "side"), ("west", "side")]);
        f.place(world, &redstone_ew, 4, -3, 1, cb);
        f.place(world, &mossy, 3, -3, 1, cb);
        f.create_dispenser(world, cb, random, 3, -2, 1, Direction::North, DISPENSER);
        let vine_south = world.state("vine", &[("south", "true")]);
        f.place(world, &vine_south, 3, -2, 2, cb);
        let hook_north = hook(world, "north");
        f.place(world, &hook_north, 7, -3, 1, cb);
        let hook_south = hook(world, "south");
        f.place(world, &hook_south, 7, -3, 5, cb);
        let wire_ns = wire(world, &["north", "south"]);
        for z in 2..=4 {
            f.place(world, &wire_ns, 7, -3, z, cb);
        }
        f.place(world, &redstone_ew, 8, -3, 6, cb);
        let redstone_ws = redstone(world, &[("west", "side"), ("south", "side")]);
        f.place(world, &redstone_ws, 9, -3, 6, cb);
        let redstone_n_up_s = redstone(world, &[("north", "side"), ("south", "up")]);
        f.place(world, &redstone_n_up_s, 9, -3, 5, cb);
        f.place(world, &mossy, 9, -3, 4, cb);
        f.place(world, &redstone_ns, 9, -2, 4, cb);
        f.create_dispenser(world, cb, random, 9, -2, 3, Direction::West, DISPENSER);
        let vine_east = world.state("vine", &[("east", "true")]);
        f.place(world, &vine_east, 8, -1, 3, cb);
        f.place(world, &vine_east, 8, -2, 3, cb);
        f.create_chest(world, cb, random, 8, -3, 3, CHEST);
        for (x, y, z) in [
            (9, -3, 2),
            (8, -3, 1),
            (4, -3, 5),
            (5, -2, 5),
            (5, -1, 5),
            (6, -3, 5),
            (7, -2, 5),
            (7, -1, 5),
            (8, -3, 5),
        ] {
            f.place(world, &mossy, x, y, z, cb);
        }
        stone(world, random, (9, -1, 1), (9, -1, 5));
        f.generate_air_box(world, cb, (8, -3, 8), (10, -1, 10));
        let chiseled = world.state("chiseled_stone_bricks", &[]);
        let lever = world.state("lever", &[("facing", "north"), ("face", "wall")]);
        for x in 8..=10 {
            f.place(world, &chiseled, x, -2, 11, cb);
        }
        for x in 8..=10 {
            f.place(world, &lever, x, -2, 12, cb);
        }
        stone(world, random, (8, -3, 8), (8, -3, 10));
        stone(world, random, (10, -3, 8), (10, -3, 10));
        f.place(world, &mossy, 10, -2, 9, cb);
        f.place(world, &redstone_ns, 8, -2, 9, cb);
        f.place(world, &redstone_ns, 8, -2, 10, cb);
        let redstone_all = redstone(
            world,
            &[
                ("north", "side"),
                ("south", "side"),
                ("east", "side"),
                ("west", "side"),
            ],
        );
        f.place(world, &redstone_all, 10, -1, 9, cb);
        let piston_up = world.state("sticky_piston", &[("facing", "up")]);
        f.place(world, &piston_up, 9, -2, 8, cb);
        let piston_west = world.state("sticky_piston", &[("facing", "west")]);
        f.place(world, &piston_west, 10, -2, 8, cb);
        f.place(world, &piston_west, 10, -1, 8, cb);
        let repeater = world.state("repeater", &[("facing", "north")]);
        f.place(world, &repeater, 10, -2, 10, cb);
        f.create_chest(world, cb, random, 9, -3, 10, CHEST);
    }
}

#[cfg(test)]
mod tests {
    use oxide_core::{ChunkPos, WorldgenRandom};

    use super::super::swamp_hut::tests::{block_at, facts, place};
    use super::*;

    #[test]
    fn temple_draws_its_moss_and_loot_in_vanilla_order() {
        let Some(facts) = facts() else { return };
        let temple = JungleTemplePiece {
            bounding_box: make_bounding_box(0, 64, 0, Direction::East, WIDTH, 10, DEPTH),
            facing: Direction::East,
        };
        let structure = GeneratedStructure {
            position: BlockPos::new(8, 70, 8),
            pieces: vec![Box::new(temple)],
            after_place: None,
        };
        let (writes, extras, _) = place(
            facts,
            &structure,
            ChunkPos::new(0, 0),
            7,
            (70, "grass_block", "air"),
        );
        // East maps local (x, z) to world (z, x): the floor's first stone at local (0, -4, 0) is
        // world (0, 66, 0), drawn by the decoration random's first float.
        let first = WorldgenRandom::new(7).next_float();
        let expected = if first < 0.4 {
            "cobblestone"
        } else {
            "mossy_cobblestone"
        };
        assert_eq!(
            block_at(&writes, BlockPos::new(0, 66, 0)).map(|s| s.name.path()),
            Some(expected)
        );
        // Two dispensers, then two chests, each with a loot seed.
        let mut tables: Vec<_> = extras
            .block_entities
            .iter()
            .filter(|e| e.snbt.contains("LootTable"))
            .map(|e| e.block.path().to_owned())
            .collect();
        tables.sort();
        assert_eq!(tables, ["chest", "chest", "dispenser", "dispenser"]);
    }
}
