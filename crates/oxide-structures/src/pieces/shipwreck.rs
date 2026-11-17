//! `ShipwreckStructure` and `ShipwreckPieces`.

use std::sync::OnceLock;

use oxide_chunkgen::decorator::Processor;
use oxide_core::rotation::Rotation;
use oxide_core::{BlockPos, HeightmapType, RandomSource};

use super::template_piece::{
    lowest_y, mean_first_occupied_height, minecraft, offset, set_loot_table, TemplateAssets,
    TemplatePiece,
};
use super::{GeneratedStructure, GenerationContext, PieceWorld, StructurePiece};
use crate::BoundingBox;

const BEACHED: [&str; 11] = [
    "with_mast",
    "sideways_full",
    "sideways_fronthalf",
    "sideways_backhalf",
    "rightsideup_full",
    "rightsideup_fronthalf",
    "rightsideup_backhalf",
    "with_mast_degraded",
    "rightsideup_full_degraded",
    "rightsideup_fronthalf_degraded",
    "rightsideup_backhalf_degraded",
];

const OCEAN: [&str; 20] = [
    "with_mast",
    "upsidedown_full",
    "upsidedown_fronthalf",
    "upsidedown_backhalf",
    "sideways_full",
    "sideways_fronthalf",
    "sideways_backhalf",
    "rightsideup_full",
    "rightsideup_fronthalf",
    "rightsideup_backhalf",
    "with_mast_degraded",
    "upsidedown_full_degraded",
    "upsidedown_fronthalf_degraded",
    "upsidedown_backhalf_degraded",
    "sideways_full_degraded",
    "sideways_fronthalf_degraded",
    "sideways_backhalf_degraded",
    "rightsideup_full_degraded",
    "rightsideup_fronthalf_degraded",
    "rightsideup_backhalf_degraded",
];

fn heightmap(beached: bool) -> HeightmapType {
    if beached {
        HeightmapType::WorldSurfaceWg
    } else {
        HeightmapType::OceanFloorWg
    }
}

/// `ShipwreckStructure.findGenerationPoint`; `beached` is the structure's `is_beached`.
pub fn generate(
    ctx: &GenerationContext<'_>,
    assets: &TemplateAssets<'_>,
    beached: bool,
) -> GeneratedStructure {
    let (x, z) = (ctx.chunk.x * 16 + 8, ctx.chunk.z * 16 + 8);
    let height = ctx.terrain.base_height(x, z, heightmap(beached));
    GeneratedStructure {
        position: BlockPos::new(x, height - 1, z),
        pieces: vec![Box::new(piece(ctx, assets, beached))],
        after_place: None,
    }
}

/// `ShipwreckStructure.generatePieces`.
fn piece(
    ctx: &GenerationContext<'_>,
    assets: &TemplateAssets<'_>,
    beached: bool,
) -> ShipwreckPiece {
    let mut random = ctx.random();
    let rotation = Rotation::get_random(&mut random);
    let start = BlockPos::new(ctx.chunk.x * 16, 90, ctx.chunk.z * 16);
    let names: &[&str] = if beached { &BEACHED } else { &OCEAN };
    let name = names[random.next_int_bounded(names.len() as i32) as usize];
    let mut template = TemplatePiece::new(
        assets,
        ctx.seed,
        minecraft(&format!("shipwreck/{name}")),
        start,
        rotation,
        vec![Processor::BlockIgnore(vec![
            minecraft("structure_block"),
            minecraft("air"),
        ])],
    );
    template.pivot = BlockPos::new(4, 0, 15);
    let mut piece = ShipwreckPiece {
        template,
        beached,
        adjusted_height: OnceLock::new(),
    };
    if piece.too_big_for_region() {
        let b = piece.template.bounding_box();
        let (x_span, z_span) = (b.max_x - b.min_x + 1, b.max_z - b.min_z + 1);
        let height = if beached {
            // Vanilla passes (minX, xSpan, minZ, zSpan) to getLowestY(minX, minZ, sizeX, sizeZ).
            let min_y = lowest_y(ctx, b.min_x, x_span, b.min_z, z_span);
            piece.beached_height(min_y, &mut random)
        } else {
            mean_first_occupied_height(ctx, b.min_x, x_span, b.min_z, z_span)
        };
        piece.template.position.y = height;
        let _ = piece.adjusted_height.set(height);
    }
    piece
}

#[derive(Debug)]
pub struct ShipwreckPiece {
    template: TemplatePiece,
    beached: bool,
    /// `heightAdjusted` and the `templatePosition` Y it set. Vanilla settles it in the first
    /// chunk that places the piece, drawing the beached offset from that chunk's random; this
    /// keeps that "first chunk wins" for every later chunk.
    ///
    /// PARITY-CHECK: which chunk places a start's piece first follows vanilla's chunk order only
    /// as far as the caller's decoration order does; a beached wreck may settle one to two
    /// blocks differently (`nextInt(3)`) when another chunk goes first.
    adjusted_height: OnceLock<i32>,
}

impl ShipwreckPiece {
    /// `isTooBigToFitInWorldGenRegion`.
    fn too_big_for_region(&self) -> bool {
        let [x, y, _] = self.template.size();
        x > 32 || y > 32
    }

    /// `calculateBeachedPosition`.
    fn beached_height(&self, min_y: i32, random: &mut dyn RandomSource) -> i32 {
        min_y - self.template.size()[1] / 2 - random.next_int_bounded(3)
    }

    /// The footprint's lowest and mean heightmap value, as `postProcess` measures it.
    fn settle(&self, world: &PieceWorld<'_, '_>, random: &mut dyn RandomSource) -> i32 {
        let kind = heightmap(self.beached);
        let position = self.template.position;
        let [size_x, _, size_z] = self.template.size();
        let base = size_x * size_z;
        let mut min_y = world.max_y();
        let mut mean = 0;
        if base == 0 {
            mean = world.height(kind, position.x, position.z);
        } else {
            for x in position.x..position.x + size_x {
                for z in position.z..position.z + size_z {
                    let height = world.height(kind, x, z);
                    mean += height;
                    min_y = min_y.min(height);
                }
            }
            mean /= base;
        }
        if self.beached {
            self.beached_height(min_y, random)
        } else {
            mean
        }
    }
}

impl StructurePiece for ShipwreckPiece {
    fn bounding_box(&self) -> BoundingBox {
        self.template.bounding_box()
    }

    fn post_process(
        &self,
        world: &mut PieceWorld<'_, '_>,
        random: &mut dyn RandomSource,
        chunk_box: &BoundingBox,
        _reference: BlockPos,
    ) {
        let height = *self
            .adjusted_height
            .get_or_init(|| self.settle(world, random));
        let mut position = self.template.position;
        position.y = height;
        self.template.place(
            world,
            random,
            chunk_box,
            position,
            |world, random, marker, pos| {
                let table = match marker {
                    "map_chest" => "minecraft:chests/shipwreck_map",
                    "treasure_chest" => "minecraft:chests/shipwreck_treasure",
                    "supply_chest" => "minecraft:chests/shipwreck_supply",
                    _ => return,
                };
                set_loot_table(world, random, offset(pos, 0, -1, 0), table);
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::template_piece::oracle;

    #[test]
    fn layouts_match_vanilla_starts() {
        for (structure, beached) in [
            ("minecraft:shipwreck", false),
            ("minecraft:shipwreck_beached", true),
        ] {
            oracle::check_layouts(structure, |ctx, assets| {
                Some(vec![super::piece(ctx, assets, beached).template])
            });
        }
    }
}
