//! `monster_room`: a dungeon, a small cobblestone room with a spawner and up to two chests.
//!
//! The room is a box two or three blocks out from the origin in each horizontal direction, five
//! tall, with a floor and ceiling that must be solid and one to five holes in its wall at floor
//! level. It is hollowed, floored in cobblestone (mostly mossy), then up to two chests are placed
//! against a single wall and a spawner at the origin.
//!
//! The chests' loot table (`minecraft:chests/simple_dungeon` with the seed vanilla draws) and the
//! spawner's mob (skeleton, zombie, zombie or spider) go to the region as block-entity SNBT.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::block_entity;
use super::super::context::{block_in, FeatureContext};
use super::{default_state, in_tag, is_air, is_block, is_block_at, offset};

/// `MonsterRoomFeature.MOBS`.
const MOBS: [&str; 4] = [
    "minecraft:skeleton",
    "minecraft:zombie",
    "minecraft:zombie",
    "minecraft:spider",
];

/// `Direction.Plane.HORIZONTAL`: north, east, south, west.
const HORIZONTAL: [(&str, [i32; 3]); 4] = [
    ("north", [0, 0, -1]),
    ("east", [1, 0, 0]),
    ("south", [0, 0, 1]),
    ("west", [-1, 0, 0]),
];

#[derive(Debug)]
pub struct MonsterRoomFeature {
    cave_air: BlockState,
    cobblestone: BlockState,
    mossy_cobblestone: BlockState,
    chest: BlockState,
    spawner: BlockState,
}

impl MonsterRoomFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            cave_air: default_state(facts, "cave_air")?,
            cobblestone: default_state(facts, "cobblestone")?,
            mossy_cobblestone: default_state(facts, "mossy_cobblestone")?,
            chest: default_state(facts, "chest")?,
            spawner: default_state(facts, "spawner")?,
        })
    }

    fn solid(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        cx.region.is(pos, BlockFact::Solid)
    }

    /// `Feature.safeSetBlock`: writes `state` unless the block there is one features may not
    /// replace.
    fn safe_set(cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, state: BlockState) {
        if !in_tag(cx, "features_cannot_replace", block_in(cx.region, pos)) {
            cx.region.set_block(pos, state);
        }
    }

    /// `MonsterRoomFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let xr = cx.random.next_int_bounded(2) + 2;
        let (min_x, max_x) = (-xr - 1, xr + 1);
        let zr = cx.random.next_int_bounded(2) + 2;
        let (min_z, max_z) = (-zr - 1, zr + 1);
        let mut holes = 0;
        for dx in min_x..=max_x {
            for dy in -1..=4 {
                for dz in min_z..=max_z {
                    let pos = offset(origin, dx, dy, dz);
                    if (dy == -1 || dy == 4) && !Self::solid(cx, pos) {
                        return false;
                    }
                    if (dx == min_x || dx == max_x || dz == min_z || dz == max_z)
                        && dy == 0
                        && is_air(cx, pos)
                        && is_air(cx, offset(pos, 0, 1, 0))
                    {
                        holes += 1;
                    }
                }
            }
        }
        if !(1..=5).contains(&holes) {
            return false;
        }
        let min_y = cx.region.min_y();
        for dx in min_x..=max_x {
            for dy in (-1..=3).rev() {
                for dz in min_z..=max_z {
                    let pos = offset(origin, dx, dy, dz);
                    let wall = block_in(cx.region, pos).clone();
                    if dx == min_x
                        || dy == -1
                        || dz == min_z
                        || dx == max_x
                        || dy == 4
                        || dz == max_z
                    {
                        if pos.y >= min_y && !Self::solid(cx, offset(pos, 0, -1, 0)) {
                            cx.region.set_block(pos, self.cave_air.clone());
                        } else if cx.env.facts.is(&wall, BlockFact::Solid)
                            && !is_block(&wall, "chest")
                        {
                            let value = if dy == -1 && cx.random.next_int_bounded(4) != 0 {
                                self.mossy_cobblestone.clone()
                            } else {
                                self.cobblestone.clone()
                            };
                            Self::safe_set(cx, pos, value);
                        }
                    } else if !(is_block(&wall, "chest") || is_block(&wall, "spawner")) {
                        Self::safe_set(cx, pos, self.cave_air.clone());
                    }
                }
            }
        }
        self.place_chests(cx, xr, zr);
        self.place_spawner(cx);
        true
    }

    /// Up to two chests, each tried three times at a random spot of the room's floor row where
    /// exactly one horizontal neighbour is solid.
    fn place_chests(&self, cx: &mut FeatureContext<'_, '_, '_>, xr: i32, zr: i32) {
        let origin = cx.origin;
        for _ in 0..2 {
            for _ in 0..3 {
                let x = origin.x + cx.random.next_int_bounded(xr * 2 + 1) - xr;
                let z = origin.z + cx.random.next_int_bounded(zr * 2 + 1) - zr;
                let pos = BlockPos::new(x, origin.y, z);
                if !is_air(cx, pos) {
                    continue;
                }
                let walls = HORIZONTAL
                    .iter()
                    .filter(|(_, d)| Self::solid(cx, offset(pos, d[0], d[1], d[2])))
                    .count();
                if walls == 1 {
                    let chest = self.reorient(cx, pos);
                    Self::safe_set(cx, pos, chest);
                    if is_block_at(cx, pos, "chest") {
                        // `RandomizableContainer.setBlockEntityLootTable`.
                        let seed = cx.random.next_long();
                        let snbt = block_entity::with_loot_table(
                            "minecraft:chest",
                            pos,
                            "minecraft:chests/simple_dungeon",
                            seed,
                        );
                        cx.region.put_block_entity(pos, snbt);
                    }
                    break;
                }
            }
        }
    }

    /// The spawner at the origin and its mob pick.
    fn place_spawner(&self, cx: &mut FeatureContext<'_, '_, '_>) {
        let origin = cx.origin;
        Self::safe_set(cx, origin, self.spawner.clone());
        if is_block_at(cx, origin, "spawner") {
            // `Util.getRandom(MOBS, random)`; a fresh spawner's empty `SpawnPotentials` draws
            // nothing more in `setEntityId`.
            let mob = MOBS[cx.random.next_int_bounded(4) as usize];
            cx.region
                .put_block_entity(origin, block_entity::spawner(origin, mob));
        }
    }

    /// `StructurePiece.reorient` for the chest's horizontal facing.
    ///
    /// PARITY-CHECK: "solid render" is the exported `SolidRender` fact, as `isSolidRender()`.
    fn reorient(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> BlockState {
        let solid_render = |at: BlockPos| cx.region.is(at, BlockFact::SolidRender);
        let side = |name: &str| HORIZONTAL.iter().position(|(n, _)| *n == name).unwrap_or(0);
        let neighbour = |index: usize| {
            let d = HORIZONTAL[index].1;
            offset(pos, d[0], d[1], d[2])
        };
        let facing = |index: usize| {
            let mut state = self.chest.clone();
            state
                .properties
                .insert("facing".to_string(), HORIZONTAL[index].0.to_string());
            state
        };
        let mut solid_side: Option<usize> = None;
        for index in 0..4 {
            let beside = block_in(cx.region, neighbour(index));
            if is_block(beside, "chest") {
                return self.chest.clone();
            }
            if cx.env.facts.is(beside, BlockFact::SolidRender) {
                if solid_side.is_some() {
                    solid_side = None;
                    break;
                }
                solid_side = Some(index);
            }
        }
        if let Some(index) = solid_side {
            return facing((index + 2) % 4);
        }
        let mut lock = side(
            self.chest
                .properties
                .get("facing")
                .map_or("north", String::as_str),
        );
        if solid_render(neighbour(lock)) {
            lock = (lock + 2) % 4;
        }
        if solid_render(neighbour(lock)) {
            lock = (lock + 1) % 4;
        }
        if solid_render(neighbour(lock)) {
            lock = (lock + 2) % 4;
        }
        facing(lock)
    }
}
