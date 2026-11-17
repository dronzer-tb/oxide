//! Desert pyramids: 26.2's `DesertPyramidStructure` and `DesertPyramidPiece`.
//!
//! The only piece of C3's set that spans chunks (21x21 from the start chunk's corner, so up to
//! four), which is what makes it awkward: vanilla settles its height on the first chunk it is
//! placed in and keeps that on the shared piece. Here every chunk works the same answer out for
//! itself from inputs they all see alike (see [`DesertPyramidPiece::floor`]).

use oxide_chunkgen::decoration_seed::{set_decoration_seed, set_feature_seed};
use oxide_chunkgen::decorator::block_entity;
use oxide_core::rotation::Direction;
use oxide_core::{
    BlockPos, BlockState, ChunkPos, HeightmapType, LegacyPositionalRandomFactory, LegacyRandom,
    PositionalRandomFactory, RandomSource, WorldgenRandom, Xoroshiro128PlusPlus,
    XoroshiroPositionalRandomFactory,
};

use super::jungle_temple::single_piece;
use super::swamp_hut::settled_frame;
use super::{
    center, make_bounding_box, random_horizontal, GeneratedStructure, GenerationContext,
    PieceFrame, PieceWorld, StructurePiece, HORIZONTAL,
};
use crate::BoundingBox;

const SIZE: i32 = 21;
const CHEST: &str = "minecraft:chests/desert_pyramid";
const ARCHAEOLOGY: &str = "minecraft:archaeology/desert_pyramid";

#[derive(Debug, Clone)]
pub struct DesertPyramidPiece {
    bounding_box: BoundingBox,
    facing: Direction,
    world_seed: i64,
    start: ChunkPos,
    /// The structure's `setFeatureSeed` index and decoration step.
    feature: (i32, i32),
}

/// `DesertPyramidStructure.findGenerationPoint`. `index` and `step` are the structure's
/// `setFeatureSeed` arguments: the ones the caller seeds [`super::place_in_chunk`]'s random with.
pub fn generate(ctx: &GenerationContext<'_>, index: i32, step: i32) -> Option<GeneratedStructure> {
    single_piece(ctx, SIZE, SIZE, || {
        let facing = random_horizontal(&mut ctx.random());
        let (west, north) = (ctx.chunk.min_block_x(), ctx.chunk.min_block_z());
        Box::new(DesertPyramidPiece {
            bounding_box: make_bounding_box(west, 64, north, facing, SIZE, 15, SIZE),
            facing,
            world_seed: ctx.seed,
            start: ctx.chunk,
            feature: (index, step),
        })
    })
}

/// `WorldGenRegion.getRandom()` of the region decorating `chunk`:
/// `getOrCreateRandomFactory("minecraft:worldgen_region_random").at(chunk.getWorldPosition())`
/// over the overworld's Xoroshiro source.
///
/// PARITY-CHECK: it starts fresh here, as if the pyramid were the region random's first user in
/// the chunk. Before surface structures only template `updateShape` calls draw from it, and a
/// desert village sharing the chunk is the one way that happens.
fn region_random(world_seed: i64, chunk: ChunkPos) -> Xoroshiro128PlusPlus {
    let mut source = Xoroshiro128PlusPlus::new(world_seed);
    let (lo, hi) = (source.next_long() as u64, source.next_long() as u64);
    let mut named = XoroshiroPositionalRandomFactory::new(lo, hi)
        .from_hash_of("minecraft:worldgen_region_random");
    let (lo, hi) = (named.next_long() as u64, named.next_long() as u64);
    XoroshiroPositionalRandomFactory::new(lo, hi).at(chunk.min_block_x(), 0, chunk.min_block_z())
}

/// `RandomSource.createThreadLocalInstance(seed).forkPositional().at(pos)`.
fn positional(world_seed: i64, pos: BlockPos) -> LegacyRandom {
    let fork = LegacyRandom::new(world_seed).next_long();
    LegacyPositionalRandomFactory::new(fork).at(pos.x, pos.y, pos.z)
}

/// `nextIntBetweenInclusive`.
fn between(random: &mut dyn RandomSource, min: i32, max: i32) -> i32 {
    random.next_int_bounded(max - min + 1) + min
}

/// `DesertPyramidStructure.placeSuspiciousSand`.
fn place_suspicious_sand(world: &mut PieceWorld<'_, '_>, chunk_box: &BoundingBox, pos: BlockPos) {
    if chunk_box.contains(pos) {
        let sand = world.state("suspicious_sand", &[]);
        world.set_block(pos, sand);
        let snbt = block_entity::with_loot_table(
            "minecraft:brushable_block",
            pos,
            ARCHAEOLOGY,
            block_entity::pos_as_long(pos),
        );
        world.put_block_entity(pos, snbt);
    }
}

impl DesertPyramidPiece {
    /// The settled floor: `updateHeightPositionToLowestGroundHeight(level, -random.nextInt(3))`.
    ///
    /// PARITY-CHECK: vanilla settles on whichever chunk places the pyramid first, reading the
    /// live heightmaps and that chunk's `nextInt(3)`. Every chunk here takes the start chunk as
    /// that first one: the lowest motion-blocking-no-leaves height over the box from the
    /// heightmaps as terrain left them, and the start chunk's own draw. A chunk placed first in
    /// vanilla, or a feature an earlier step put inside the box, can move the result.
    fn floor(&self, world: &PieceWorld<'_, '_>) -> i32 {
        let b = &self.bounding_box;
        let mut lowest = world.max_y() + 1;
        for z in b.min_z..=b.max_z {
            for x in b.min_x..=b.max_x {
                let height = world
                    .region
                    .terrain_height(HeightmapType::MotionBlockingNoLeaves, x, z)
                    .unwrap_or_else(|| world.min_y());
                lowest = lowest.min(height);
            }
        }
        let mut start_random = WorldgenRandom::new(0);
        let (x, z) = (self.start.min_block_x(), self.start.min_block_z());
        let decoration_seed = set_decoration_seed(&mut start_random, self.world_seed, x, z);
        let (index, step) = self.feature;
        set_feature_seed(&mut start_random, decoration_seed, index, step);
        lowest - start_random.next_int_bounded(3)
    }

    /// `addCellarStairs`.
    fn cellar_stairs(
        &self,
        f: &PieceFrame,
        world: &mut PieceWorld<'_, '_>,
        cb: &BoundingBox,
        region_random: &mut Xoroshiro128PlusPlus,
        (x, y, z): (i32, i32, i32),
    ) {
        // `SANDSTONE_STAIRS.defaultBlockState().rotate(COUNTERCLOCKWISE_90)`: north turns west.
        let stairs = world.state("sandstone_stairs", &[("facing", "west")]);
        f.place(world, &stairs, 13, -1, 17, cb);
        f.place(world, &stairs, 14, -2, 17, cb);
        f.place(world, &stairs, 15, -3, 17, cb);
        let sand = world.state("sand", &[]);
        let sandstone = world.state("sandstone", &[]);
        let variant = region_random.next_boolean();
        for dx in -4..=0 {
            f.place(world, &sand, x + dx, y + 4, z + 4, cb);
        }
        f.place(world, &sand, x - 2, y + 3, z + 4, cb);
        let (a, b) = if variant {
            (&sand, &sandstone)
        } else {
            (&sandstone, &sand)
        };
        f.place(world, a, x - 1, y + 3, z + 4, cb);
        f.place(world, b, x, y + 3, z + 4, cb);
        f.place(world, &sand, x - 1, y + 2, z + 4, cb);
        f.place(world, &sandstone, x, y + 2, z + 4, cb);
        f.place(world, &sand, x, y + 1, z + 4, cb);
    }

    /// `addCellarRoom`: the room's blocks, then its potential suspicious sand (world positions)
    /// and the collapsed roof's random position.
    fn cellar_room(
        &self,
        f: &PieceFrame,
        world: &mut PieceWorld<'_, '_>,
        cb: &BoundingBox,
        region_random: &mut Xoroshiro128PlusPlus,
        (x, y, z): (i32, i32, i32),
    ) -> (Vec<BlockPos>, BlockPos) {
        let cut = world.state("cut_sandstone", &[]);
        let chiseled = world.state("chiseled_sandstone", &[]);
        for (state, ly) in [(&cut, y + 1), (&chiseled, y + 2), (&cut, -1)] {
            for (min, max) in [
                ((x - 3, ly, z - 3), (x - 3, ly, z + 2)),
                ((x + 3, ly, z - 3), (x + 3, ly, z + 2)),
                ((x - 3, ly, z - 3), (x + 3, ly, z - 2)),
                ((x - 3, ly, z + 3), (x + 3, ly, z + 3)),
            ] {
                f.generate_box(world, cb, min, max, state, state, true);
            }
        }
        let mut sand_spots = Vec::new();
        for sy in y + 1..=y + 3 {
            for sx in x - 2..=x + 2 {
                for sz in z - 2..=z + 2 {
                    sand_spots.push(f.world_pos(sx, sy, sz));
                }
            }
        }
        // `placeCollapsedRoof(x - 2, y + 4, z - 2, x + 2, z + 2)`.
        let (x0, y0, z0, x1, z1) = (x - 2, y + 4, z - 2, x + 2, z + 2);
        let sand = world.state("sand", &[]);
        let sandstone = world.state("sandstone", &[]);
        for rx in x0..=x1 {
            for rz in z0..=z1 {
                let state = if region_random.next_float() < 0.33 {
                    &sandstone
                } else {
                    &sand
                };
                f.place(world, state, rx, y0, rz, cb);
            }
        }
        let mut roof_random = positional(self.world_seed, f.world_pos(x0, y0, z0));
        let roof_x = between(&mut roof_random, x0, x1);
        let roof_z = between(&mut roof_random, z0, z1);
        let roof = f.world_pos(roof_x, y0, roof_z);

        let orange = world.state("orange_terracotta", &[]);
        let blue = world.state("blue_terracotta", &[]);
        f.place(world, &blue, x, y, z, cb);
        for (dx, dz) in [
            (1, -1),
            (1, 1),
            (-1, -1),
            (-1, 1),
            (2, 0),
            (-2, 0),
            (0, 2),
            (0, -2),
        ] {
            f.place(world, &orange, x + dx, y, z + dz, cb);
        }
        f.place(world, &orange, x + 3, y, z, cb);
        sand_spots.push(f.world_pos(x + 3, y + 1, z));
        sand_spots.push(f.world_pos(x + 3, y + 2, z));
        f.place(world, &cut, x + 4, y + 1, z, cb);
        f.place(world, &chiseled, x + 4, y + 2, z, cb);
        f.place(world, &orange, x - 3, y, z, cb);
        sand_spots.push(f.world_pos(x - 3, y + 1, z));
        sand_spots.push(f.world_pos(x - 3, y + 2, z));
        f.place(world, &cut, x - 4, y + 1, z, cb);
        f.place(world, &chiseled, x - 4, y + 2, z, cb);
        f.place(world, &orange, x, y, z + 3, cb);
        sand_spots.push(f.world_pos(x, y + 1, z + 3));
        sand_spots.push(f.world_pos(x, y + 2, z + 3));
        f.place(world, &orange, x, y, z - 3, cb);
        sand_spots.push(f.world_pos(x, y + 1, z - 3));
        sand_spots.push(f.world_pos(x, y + 2, z - 3));
        f.place(world, &cut, x, y + 1, z - 4, cb);
        f.place(world, &chiseled, x, -2, z - 4, cb);
        (sand_spots, roof)
    }

    /// `DesertPyramidStructure.afterPlace`, for this one piece: the roof's suspicious sand, then
    /// 5 to 7 of the shuffled sand spots suspicious and the rest plain sand.
    fn after_place(
        &self,
        world: &mut PieceWorld<'_, '_>,
        cb: &BoundingBox,
        settled: &BoundingBox,
        mut spots: Vec<BlockPos>,
        roof: BlockPos,
    ) {
        // `SortedArraySet` on `Vec3i.compareTo`: y, then z, then x.
        spots.sort_unstable_by_key(|p| (p.y, p.z, p.x));
        spots.dedup();
        place_suspicious_sand(world, cb, roof);
        let mut random = positional(self.world_seed, center(settled));
        for i in (2..=spots.len()).rev() {
            let swap = random.next_int_bounded(i as i32) as usize;
            spots.swap(i - 1, swap);
        }
        let suspicious = spots.len().min((5 + random.next_int_bounded(3)) as usize);
        let sand = world.state("sand", &[]);
        for (i, pos) in spots.into_iter().enumerate() {
            if i < suspicious {
                place_suspicious_sand(world, cb, pos);
            } else if cb.contains(pos) {
                world.set_block(pos, sand.clone());
            }
        }
    }
}

/// The four-wide tower faces: one column of the pattern per `(dx, dz)` across the face.
const TOWER_FACE: [[&str; 3]; 7] = [
    ["cut_sandstone", "orange_terracotta", "cut_sandstone"],
    ["cut_sandstone", "orange_terracotta", "cut_sandstone"],
    [
        "orange_terracotta",
        "chiseled_sandstone",
        "orange_terracotta",
    ],
    ["cut_sandstone", "orange_terracotta", "cut_sandstone"],
    [
        "orange_terracotta",
        "chiseled_sandstone",
        "orange_terracotta",
    ],
    [
        "orange_terracotta",
        "orange_terracotta",
        "orange_terracotta",
    ],
    ["cut_sandstone", "cut_sandstone", "cut_sandstone"],
];

impl StructurePiece for DesertPyramidPiece {
    fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    fn post_process(
        &self,
        world: &mut PieceWorld<'_, '_>,
        random: &mut dyn RandomSource,
        cb: &BoundingBox,
        _reference: BlockPos,
    ) {
        // Drawn by every chunk, though only the first chunk's counts (see `floor`).
        random.next_int_bounded(3);
        let f = settled_frame(&self.bounding_box, self.facing, self.floor(world));
        let (w, d) = (SIZE, SIZE);
        let state = |world: &PieceWorld<'_, '_>, path: &str| world.state(path, &[]);
        let sandstone = state(world, "sandstone");
        let cut = state(world, "cut_sandstone");
        let chiseled = state(world, "chiseled_sandstone");
        let orange = state(world, "orange_terracotta");
        let air = state(world, "air");
        let stairs = |world: &PieceWorld<'_, '_>, facing: &str| {
            world.state("sandstone_stairs", &[("facing", facing)])
        };
        let north = stairs(world, "north");
        let south = stairs(world, "south");
        let east = stairs(world, "east");
        let west = stairs(world, "west");

        f.generate_box(
            world,
            cb,
            (0, -4, 0),
            (w - 1, 0, d - 1),
            &sandstone,
            &sandstone,
            false,
        );
        for i in 1..=9 {
            f.generate_box(
                world,
                cb,
                (i, i, i),
                (w - 1 - i, i, d - 1 - i),
                &sandstone,
                &sandstone,
                false,
            );
            f.generate_box(
                world,
                cb,
                (i + 1, i, i + 1),
                (w - 2 - i, i, d - 2 - i),
                &air,
                &air,
                false,
            );
        }
        for x in 0..w {
            for z in 0..d {
                f.fill_column_down(world, &sandstone, x, -5, z, cb);
            }
        }
        // The two front towers.
        for x0 in [0, w - 5] {
            f.generate_box(
                world,
                cb,
                (x0, 0, 0),
                (x0 + 4, 9, 4),
                &sandstone,
                &air,
                false,
            );
            f.generate_box(
                world,
                cb,
                (x0 + 1, 10, 1),
                (x0 + 3, 10, 3),
                &sandstone,
                &sandstone,
                false,
            );
            f.place(world, &north, x0 + 2, 10, 0, cb);
            f.place(world, &south, x0 + 2, 10, 4, cb);
            f.place(world, &east, x0, 10, 2, cb);
            f.place(world, &west, x0 + 4, 10, 2, cb);
        }
        f.generate_box(world, cb, (8, 0, 0), (12, 4, 4), &sandstone, &air, false);
        f.generate_box(world, cb, (9, 1, 0), (11, 3, 4), &air, &air, false);
        for (x, y) in [(9, 1), (9, 2), (9, 3), (10, 3), (11, 3), (11, 2), (11, 1)] {
            f.place(world, &cut, x, y, 1, cb);
        }
        f.generate_box(world, cb, (4, 1, 1), (8, 3, 3), &sandstone, &air, false);
        f.generate_box(world, cb, (4, 1, 2), (8, 2, 2), &air, &air, false);
        f.generate_box(world, cb, (12, 1, 1), (16, 3, 3), &sandstone, &air, false);
        f.generate_box(world, cb, (12, 1, 2), (16, 2, 2), &air, &air, false);
        f.generate_box(
            world,
            cb,
            (5, 4, 5),
            (w - 6, 4, d - 6),
            &sandstone,
            &sandstone,
            false,
        );
        f.generate_box(world, cb, (9, 4, 9), (11, 4, 11), &air, &air, false);
        for (x, z) in [(8, 8), (12, 8), (8, 12), (12, 12)] {
            f.generate_box(world, cb, (x, 1, z), (x, 3, z), &cut, &cut, false);
        }
        f.generate_box(
            world,
            cb,
            (1, 1, 5),
            (4, 4, 11),
            &sandstone,
            &sandstone,
            false,
        );
        f.generate_box(
            world,
            cb,
            (w - 5, 1, 5),
            (w - 2, 4, 11),
            &sandstone,
            &sandstone,
            false,
        );
        f.generate_box(
            world,
            cb,
            (6, 7, 9),
            (6, 7, 11),
            &sandstone,
            &sandstone,
            false,
        );
        f.generate_box(
            world,
            cb,
            (w - 7, 7, 9),
            (w - 7, 7, 11),
            &sandstone,
            &sandstone,
            false,
        );
        f.generate_box(world, cb, (5, 5, 9), (5, 7, 11), &cut, &cut, false);
        f.generate_box(world, cb, (w - 6, 5, 9), (w - 6, 7, 11), &cut, &cut, false);
        for (x, y) in [(5, 5), (5, 6), (6, 6), (w - 6, 5), (w - 6, 6), (w - 7, 6)] {
            f.place(world, &air, x, y, 10, cb);
        }
        f.generate_box(world, cb, (2, 4, 4), (2, 6, 4), &air, &air, false);
        f.generate_box(world, cb, (w - 3, 4, 4), (w - 3, 6, 4), &air, &air, false);
        for (x, y, z) in [(2, 4, 5), (2, 3, 4), (w - 3, 4, 5), (w - 3, 3, 4)] {
            f.place(world, &north, x, y, z, cb);
        }
        f.generate_box(
            world,
            cb,
            (1, 1, 3),
            (2, 2, 3),
            &sandstone,
            &sandstone,
            false,
        );
        f.generate_box(
            world,
            cb,
            (w - 3, 1, 3),
            (w - 2, 2, 3),
            &sandstone,
            &sandstone,
            false,
        );
        f.place(world, &sandstone, 1, 1, 2, cb);
        f.place(world, &sandstone, w - 2, 1, 2, cb);
        let slab = state(world, "sandstone_slab");
        f.place(world, &slab, 1, 2, 2, cb);
        f.place(world, &slab, w - 2, 2, 2, cb);
        f.place(world, &west, 2, 1, 2, cb);
        f.place(world, &east, w - 3, 1, 2, cb);
        f.generate_box(
            world,
            cb,
            (4, 3, 5),
            (4, 3, 17),
            &sandstone,
            &sandstone,
            false,
        );
        f.generate_box(
            world,
            cb,
            (w - 5, 3, 5),
            (w - 5, 3, 17),
            &sandstone,
            &sandstone,
            false,
        );
        f.generate_box(world, cb, (3, 1, 5), (4, 2, 16), &air, &air, false);
        f.generate_box(world, cb, (w - 6, 1, 5), (w - 5, 2, 16), &air, &air, false);
        for z in (5..=17).step_by(2) {
            f.place(world, &cut, 4, 1, z, cb);
            f.place(world, &chiseled, 4, 2, z, cb);
            f.place(world, &cut, w - 5, 1, z, cb);
            f.place(world, &chiseled, w - 5, 2, z, cb);
        }
        for (x, z) in [
            (10, 7),
            (10, 8),
            (9, 9),
            (11, 9),
            (8, 10),
            (12, 10),
            (7, 10),
            (13, 10),
            (9, 11),
            (11, 11),
            (10, 12),
            (10, 13),
        ] {
            f.place(world, &orange, x, 0, z, cb);
        }
        let blue = state(world, "blue_terracotta");
        f.place(world, &blue, 10, 0, 10, cb);
        let face: Vec<[BlockState; 3]> = TOWER_FACE
            .iter()
            .map(|row| row.map(|path| state(world, path)))
            .collect();
        for x in [0, w - 1] {
            for (row, y) in face.iter().zip(2..) {
                for (block, z) in row.iter().zip(1..) {
                    f.place(world, block, x, y, z, cb);
                }
            }
        }
        for x in [2, w - 3] {
            for (row, y) in face.iter().zip(2..) {
                for (block, dx) in row.iter().zip(-1..) {
                    f.place(world, block, x + dx, y, 0, cb);
                }
            }
        }
        f.generate_box(world, cb, (8, 4, 0), (12, 6, 0), &cut, &cut, false);
        f.place(world, &air, 8, 6, 0, cb);
        f.place(world, &air, 12, 6, 0, cb);
        f.place(world, &orange, 9, 5, 0, cb);
        f.place(world, &chiseled, 10, 5, 0, cb);
        f.place(world, &orange, 11, 5, 0, cb);
        // The treasure room.
        f.generate_box(world, cb, (8, -14, 8), (12, -11, 12), &cut, &cut, false);
        f.generate_box(
            world,
            cb,
            (8, -10, 8),
            (12, -10, 12),
            &chiseled,
            &chiseled,
            false,
        );
        f.generate_box(world, cb, (8, -9, 8), (12, -9, 12), &cut, &cut, false);
        f.generate_box(
            world,
            cb,
            (8, -8, 8),
            (12, -1, 12),
            &sandstone,
            &sandstone,
            false,
        );
        f.generate_box(world, cb, (9, -11, 9), (11, -1, 11), &air, &air, false);
        let plate = state(world, "stone_pressure_plate");
        f.place(world, &plate, 10, -11, 10, cb);
        let tnt = state(world, "tnt");
        f.generate_box(world, cb, (9, -13, 9), (11, -13, 11), &tnt, &air, false);
        for (ax, az, bx, bz) in [
            (8, 10, 7, 10),
            (12, 10, 13, 10),
            (10, 8, 10, 7),
            (10, 12, 10, 13),
        ] {
            f.place(world, &air, ax, -11, az, cb);
            f.place(world, &air, ax, -10, az, cb);
            f.place(world, &chiseled, bx, -10, bz, cb);
            f.place(world, &cut, bx, -11, bz, cb);
        }
        for direction in HORIZONTAL {
            let (dx, dz) = (direction.step_x() * 2, direction.step_z() * 2);
            f.create_chest(world, cb, random, 10 + dx, -11, 10 + dz, CHEST);
        }
        // `addCellar`, around the room centre (16, -4, 13).
        let mut region_random = region_random(self.world_seed, world.region.center());
        let room = (16, -4, 13);
        self.cellar_stairs(&f, world, cb, &mut region_random, room);
        let (spots, roof) = self.cellar_room(&f, world, cb, &mut region_random, room);
        // The structure's one piece: `afterPlace` follows it directly.
        self.after_place(world, cb, &f.bounding_box, spots, roof);
    }
}

#[cfg(test)]
mod tests {
    use super::super::swamp_hut::tests::{block_at, facts, place};
    use super::*;

    #[test]
    fn every_chunk_of_a_pyramid_agrees_on_its_floor_and_sand() {
        let Some(facts) = facts() else { return };
        let (index, step) = (1, 4);
        let piece = DesertPyramidPiece {
            bounding_box: make_bounding_box(0, 64, 0, Direction::South, SIZE, 15, SIZE),
            facing: Direction::South,
            world_seed: 883502277,
            start: ChunkPos::new(0, 0),
            feature: (index, step),
        };
        // The start chunk's own `nextInt(3)`, below the flat ground's height of 70.
        let floor = {
            let mut random = WorldgenRandom::new(0);
            let seed = set_decoration_seed(&mut random, 883502277, 0, 0);
            set_feature_seed(&mut random, seed, index, step);
            70 - random.next_int_bounded(3)
        };
        let structure = GeneratedStructure {
            position: BlockPos::new(8, 69, 8),
            pieces: vec![Box::new(piece)],
            after_place: None,
        };
        let mut suspicious = 0;
        for (i, chunk) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
            let chunk = ChunkPos::new(chunk.0, chunk.1);
            // A different decoration random per chunk, as vanilla would hand each.
            let (writes, extras, _) =
                place(facts, &structure, chunk, i as i64, (70, "sand", "air"));
            // South keeps x and z: the slabs at local (1, 2, 2) and (19, 2, 2) fall in two
            // chunks and must share one floor.
            let slab = match (chunk.x, chunk.z) {
                (0, 0) => Some(BlockPos::new(1, floor + 2, 2)),
                (1, 0) => Some(BlockPos::new(19, floor + 2, 2)),
                _ => None,
            };
            if let Some(slab) = slab {
                assert_eq!(
                    block_at(&writes, slab).map(|s| s.name.path()),
                    Some("sandstone_slab")
                );
            }
            suspicious += extras
                .block_entities
                .iter()
                .filter(|e| e.snbt.contains(ARCHAEOLOGY))
                .count();
        }
        // The roof's one, plus 5 to 7 of the cellar's spots, across all four chunks.
        assert!((6..=8).contains(&suspicious), "{suspicious}");
    }
}
