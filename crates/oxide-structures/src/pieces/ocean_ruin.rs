//! `OceanRuinStructure` and `OceanRuinPieces`.

use oxide_chunkgen::decorator::{Processor, Rule, RuleTest};
use oxide_core::rotation::{transform, Mirror, Rotation};
use oxide_core::{BlockPos, HeightmapType, RandomSource};
use oxide_datapack::BlockFact;

use super::template_piece::{minecraft, offset, set_loot_table, TemplateAssets, TemplatePiece};
use super::{GeneratedStructure, GenerationContext, PieceWorld, StructureEntity, StructurePiece};
use crate::BoundingBox;

/// The `ocean_ruin` structure's settings.
#[derive(Debug, Clone, Copy)]
pub struct OceanRuinConfig {
    /// `biome_temp: cold`; otherwise warm.
    pub cold: bool,
    pub large_probability: f32,
    pub cluster_probability: f32,
}

/// `OceanRuinStructure.findGenerationPoint`.
pub fn generate(
    ctx: &GenerationContext<'_>,
    assets: &TemplateAssets<'_>,
    config: OceanRuinConfig,
) -> GeneratedStructure {
    let (x, z) = (ctx.chunk.x * 16 + 8, ctx.chunk.z * 16 + 8);
    let height = ctx.terrain.base_height(x, z, HeightmapType::OceanFloorWg);
    GeneratedStructure {
        position: BlockPos::new(x, height - 1, z),
        pieces: pieces(ctx, assets, config)
            .into_iter()
            .map(|piece| Box::new(piece) as Box<dyn StructurePiece>)
            .collect(),
        after_place: None,
    }
}

/// `Mth.nextInt(random, min, max)`.
fn next_int(random: &mut dyn RandomSource, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        random.next_int_bounded(max - min + 1) + min
    }
}

/// `BoundingBox.fromCorners`.
fn from_corners(a: BlockPos, b: BlockPos) -> BoundingBox {
    BoundingBox::new(
        a.x.min(b.x),
        a.y.min(b.y),
        a.z.min(b.z),
        a.x.max(b.x),
        a.y.max(b.y),
        a.z.max(b.z),
    )
}

fn turned(pos: BlockPos, rotation: Rotation, by: BlockPos) -> BlockPos {
    let moved = transform(pos, Mirror::None, rotation, BlockPos::new(0, 0, 0));
    offset(moved, by.x, by.y, by.z)
}

struct Builder<'a, 'c> {
    ctx: &'a GenerationContext<'c>,
    assets: &'a TemplateAssets<'a>,
    config: OceanRuinConfig,
    pieces: Vec<OceanRuinPiece>,
}

/// `OceanRuinStructure.generatePieces` and `OceanRuinPieces.addPieces`.
fn pieces(
    ctx: &GenerationContext<'_>,
    assets: &TemplateAssets<'_>,
    config: OceanRuinConfig,
) -> Vec<OceanRuinPiece> {
    let mut random = ctx.random();
    let position = BlockPos::new(ctx.chunk.x * 16, 90, ctx.chunk.z * 16);
    let rotation = Rotation::get_random(&mut random);
    let mut builder = Builder {
        ctx,
        assets,
        config,
        pieces: Vec::new(),
    };
    let large = random.next_float() <= config.large_probability;
    let integrity = if large { 0.9 } else { 0.8 };
    builder.add(&mut random, position, rotation, large, integrity);
    if large && random.next_float() <= config.cluster_probability {
        builder.add_cluster(&mut random, rotation, position);
    }
    builder.pieces
}

impl Builder<'_, '_> {
    /// `addClusterRuins`.
    fn add_cluster(&mut self, random: &mut dyn RandomSource, rotation: Rotation, p: BlockPos) {
        let parent = BlockPos::new(p.x, 90, p.z);
        let parent_corner = turned(BlockPos::new(15, 0, 15), rotation, parent);
        let parent_box = from_corners(parent, parent_corner);
        let origin = BlockPos::new(
            parent.x.min(parent_corner.x),
            parent.y,
            parent.z.min(parent_corner.z),
        );
        let mut positions = all_positions(random, origin);
        let ruins = next_int(random, 4, 8);
        for _ in 0..ruins {
            if positions.is_empty() {
                continue;
            }
            let index = random.next_int_bounded(positions.len() as i32) as usize;
            let pos = positions.remove(index);
            let next_rotation = Rotation::VALUES[random.next_int_bounded(4) as usize];
            let corner = turned(BlockPos::new(5, 0, 6), next_rotation, pos);
            if from_corners(pos, corner).intersects(&parent_box) {
                continue;
            }
            self.add(random, pos, next_rotation, false, 0.8);
        }
    }

    /// `addPiece`.
    fn add(
        &mut self,
        random: &mut dyn RandomSource,
        position: BlockPos,
        rotation: Rotation,
        large: bool,
        integrity: f32,
    ) {
        let mut push = |name: String, integrity: f32| {
            let template = TemplatePiece::new(
                self.assets,
                self.ctx.seed,
                minecraft(&format!("underwater_ruin/{name}")),
                position,
                rotation,
                processors(integrity, self.config.cold),
            );
            self.pieces.push(OceanRuinPiece {
                template,
                large,
                sea_level: self.ctx.terrain.sea_level(),
            });
        };
        if !self.config.cold {
            let name = if large {
                ["big_warm_4", "big_warm_5", "big_warm_6", "big_warm_7"]
                    [random.next_int_bounded(4) as usize]
                    .to_owned()
            } else {
                format!("warm_{}", random.next_int_bounded(8) + 1)
            };
            push(name, integrity);
            return;
        }
        let (prefix, number) = if large {
            ("big_", [1, 2, 3, 8][random.next_int_bounded(4) as usize])
        } else {
            ("", random.next_int_bounded(8) + 1)
        };
        push(format!("{prefix}brick_{number}"), integrity);
        push(format!("{prefix}cracked_{number}"), 0.7);
        push(format!("{prefix}mossy_{number}"), 0.5);
    }
}

/// `allPositions`, drawn in its order.
fn all_positions(random: &mut dyn RandomSource, origin: BlockPos) -> Vec<BlockPos> {
    let mut at = |x_min: i32, x: (i32, i32), z_min: i32, z: (i32, i32)| {
        let dx = x_min + next_int(random, x.0, x.1);
        let dz = z_min + next_int(random, z.0, z.1);
        offset(origin, dx, 0, dz)
    };
    vec![
        at(-16, (1, 8), 16, (1, 7)),
        at(-16, (1, 8), 0, (1, 7)),
        at(-16, (1, 8), -16, (4, 8)),
        at(0, (1, 7), 16, (1, 7)),
        at(0, (1, 7), -16, (4, 6)),
        at(16, (1, 7), 16, (3, 8)),
        at(16, (1, 7), 0, (1, 7)),
        at(16, (1, 7), -16, (4, 8)),
    ]
}

/// `OceanRuinPiece.makeSettings`'s processors.
fn processors(integrity: f32, cold: bool) -> Vec<Processor> {
    let (candidate, suspicious, table) = if cold {
        (
            "gravel",
            "suspicious_gravel",
            "minecraft:archaeology/ocean_ruin_cold",
        )
    } else {
        (
            "sand",
            "suspicious_sand",
            "minecraft:archaeology/ocean_ruin_warm",
        )
    };
    let mut output = oxide_core::BlockState::new(minecraft(suspicious));
    output = output.with_property("dusted", "0");
    vec![
        Processor::BlockRot {
            rottable: None,
            integrity,
        },
        Processor::BlockIgnore(vec![minecraft("structure_block"), minecraft("air")]),
        Processor::Capped {
            delegate: Box::new(Processor::Rule(vec![Rule::new(
                RuleTest::BlockMatch(minecraft(candidate)),
                RuleTest::AlwaysTrue,
                output,
                Some(table.to_owned()),
            )])),
            limit: 5,
        },
    ]
}

#[derive(Debug)]
pub struct OceanRuinPiece {
    template: TemplatePiece,
    large: bool,
    sea_level: i32,
}

impl OceanRuinPiece {
    /// `OceanRuinPiece.getHeight`: sink the piece to the sea floor under its footprint when
    /// enough of it would hang over a drop.
    fn floor_height(&self, world: &PieceWorld<'_, '_>, pos: BlockPos, corner: BlockPos) -> i32 {
        let ice = minecraft("ice");
        let tags = self.template.block_tags();
        let open = |world: &PieceWorld<'_, '_>, at: BlockPos| {
            world.is_air(at)
                || world.is(at, BlockFact::Water)
                || world
                    .region
                    .block(at)
                    .is_some_and(|state| tags.contains(&ice, &state.name))
        };
        let mut new_y = pos.y;
        let mut min_y = 512;
        let top_y = new_y - 1;
        let mut area = 0;
        for x in pos.x.min(corner.x)..=pos.x.max(corner.x) {
            for z in pos.z.min(corner.z)..=pos.z.max(corner.z) {
                let mut floor_y = pos.y - 1;
                while open(world, BlockPos::new(x, floor_y, z)) && floor_y > world.min_y() + 1 {
                    floor_y -= 1;
                }
                min_y = min_y.min(floor_y);
                if floor_y < top_y - 2 {
                    area += 1;
                }
            }
        }
        let width = (pos.x - corner.x).abs();
        if top_y - min_y > 2 && area > width - 2 {
            new_y = min_y + 1;
        }
        new_y
    }
}

impl StructurePiece for OceanRuinPiece {
    fn bounding_box(&self) -> BoundingBox {
        self.template.bounding_box()
    }

    // Vanilla moves `templatePosition` here and keeps it, but recomputes it from the heightmap
    // and the blocks in place at the start of every call, so recomputing from scratch is exact.
    fn post_process(
        &self,
        world: &mut PieceWorld<'_, '_>,
        random: &mut dyn RandomSource,
        chunk_box: &BoundingBox,
        _reference: BlockPos,
    ) {
        let t = &self.template;
        let mut position = t.position;
        position.y = world.height(HeightmapType::OceanFloorWg, position.x, position.z);
        let [size_x, _, size_z] = t.size();
        let corner = turned(
            BlockPos::new(size_x - 1, 0, size_z - 1),
            t.rotation,
            position,
        );
        position.y = self.floor_height(world, position, corner);
        let table = if self.large {
            "minecraft:chests/underwater_ruin_big"
        } else {
            "minecraft:chests/underwater_ruin_small"
        };
        t.place(
            world,
            random,
            chunk_box,
            position,
            |world, random, marker, pos| {
                match marker {
                    "chest" => {
                        let waterlogged = if world.is(pos, BlockFact::Water) {
                            "true"
                        } else {
                            "false"
                        };
                        world.set_block(pos, world.state("chest", &[("waterlogged", waterlogged)]));
                        set_loot_table(world, random, pos, table);
                    }
                    "drowned" => {
                        // PARITY-CHECK: `finalizeSpawn` rolls the drowned's equipment and
                        // attributes from the level's random, which nothing here models; the
                        // entity is saved bare, persistent, where vanilla spawns it.
                        world.add_entity(StructureEntity {
                            pos: [pos.x as f64 + 0.5, pos.y as f64, pos.z as f64 + 0.5],
                            snbt: "{id:\"minecraft:drowned\",PersistenceRequired:1b}".to_owned(),
                        });
                        let block = if pos.y > self.sea_level {
                            "air"
                        } else {
                            "water"
                        };
                        world.set_block(pos, world.state(block, &[]));
                    }
                    _ => {}
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::template_piece::oracle;
    use super::OceanRuinConfig;

    #[test]
    fn layouts_match_vanilla_starts() {
        for (structure, cold) in [
            ("minecraft:ocean_ruin_cold", true),
            ("minecraft:ocean_ruin_warm", false),
        ] {
            let config = OceanRuinConfig {
                cold,
                large_probability: 0.3,
                cluster_probability: 0.9,
            };
            oracle::check_layouts(structure, |ctx, assets| {
                Some(
                    super::pieces(ctx, assets, config)
                        .into_iter()
                        .map(|piece| piece.template)
                        .collect(),
                )
            });
        }
    }
}
