//! `NetherFossilStructure` and `NetherFossilPieces`.

use oxide_chunkgen::decorator::Processor;
use oxide_core::rotation::{rotate_block_state, Rotation};
use oxide_core::{
    BlockPos, LegacyPositionalRandomFactory, LegacyRandom, PositionalRandomFactory, RandomSource,
};
use oxide_datapack::BlockFact;

use super::template_piece::{minecraft, BaseColumns, TemplateAssets, TemplatePiece};
use super::{GeneratedStructure, GenerationContext, PieceWorld, StructurePiece};
use crate::BoundingBox;

/// The structure's `height`, a `uniform` provider resolved against the generator's world
/// (`WorldGenerationContext`): 26.2's `nether_fossil` is 32 above bottom to 2 below the top.
#[derive(Debug, Clone, Copy)]
pub struct UniformHeight {
    pub min_inclusive: i32,
    pub max_inclusive: i32,
}

impl UniformHeight {
    /// `UniformHeight.sample`: an empty range yields its minimum without drawing.
    fn sample(self, random: &mut dyn RandomSource) -> i32 {
        if self.min_inclusive > self.max_inclusive {
            return self.min_inclusive;
        }
        random.next_int_bounded(self.max_inclusive - self.min_inclusive + 1) + self.min_inclusive
    }
}

/// `NetherFossilStructure.findGenerationPoint`: `None` where no air-over-floor spot lies above
/// the sea between the sampled height and the lava sea.
pub fn generate(
    ctx: &GenerationContext<'_>,
    assets: &TemplateAssets<'_>,
    columns: &dyn BaseColumns,
    height: UniformHeight,
) -> Option<GeneratedStructure> {
    let (position, piece) = locate(ctx, assets, columns, height)?;
    Some(GeneratedStructure {
        position,
        pieces: vec![Box::new(piece)],
        after_place: None,
    })
}

fn locate(
    ctx: &GenerationContext<'_>,
    assets: &TemplateAssets<'_>,
    columns: &dyn BaseColumns,
    height: UniformHeight,
) -> Option<(BlockPos, NetherFossilPiece)> {
    let mut random = ctx.random();
    let x = ctx.chunk.x * 16 + random.next_int_bounded(16);
    let z = ctx.chunk.z * 16 + random.next_int_bounded(16);
    let sea_level = ctx.terrain.sea_level();
    let mut y = height.sample(&mut random);
    let soul_sand = minecraft("soul_sand");
    while y > sea_level {
        let current = columns.block(x, y, z);
        y -= 1;
        let below = columns.block(x, y, z);
        if assets.facts.is(&current, BlockFact::Air)
            && (below.name == soul_sand || assets.facts.is(&below, BlockFact::SturdyTop))
        {
            break;
        }
    }
    if y <= sea_level {
        return None;
    }
    let position = BlockPos::new(x, y, z);
    let rotation = Rotation::get_random(&mut random);
    let fossil = random.next_int_bounded(14) + 1;
    let template = TemplatePiece::new(
        assets,
        ctx.seed,
        minecraft(&format!("nether_fossils/fossil_{fossil}")),
        position,
        rotation,
        vec![Processor::BlockIgnore(vec![
            minecraft("structure_block"),
            minecraft("air"),
        ])],
    );
    Some((position, NetherFossilPiece { template }))
}

#[derive(Debug)]
pub struct NetherFossilPiece {
    template: TemplatePiece,
}

impl StructurePiece for NetherFossilPiece {
    fn bounding_box(&self) -> BoundingBox {
        self.template.bounding_box()
    }

    /// Vanilla widens the chunk's box to the whole fossil, so every chunk the fossil touches
    /// writes all of it (as far as the region reaches).
    fn post_process(
        &self,
        world: &mut PieceWorld<'_, '_>,
        random: &mut dyn RandomSource,
        chunk_box: &BoundingBox,
        _reference: BlockPos,
    ) {
        let fossil = self.template.bounding_box();
        let widened = BoundingBox::new(
            chunk_box.min_x.min(fossil.min_x),
            chunk_box.min_y.min(fossil.min_y),
            chunk_box.min_z.min(fossil.min_z),
            chunk_box.max_x.max(fossil.max_x),
            chunk_box.max_y.max(fossil.max_y),
            chunk_box.max_z.max(fossil.max_z),
        );
        self.template.place(
            world,
            random,
            &widened,
            self.template.position,
            |_, _, _, _| {},
        );
        self.place_dried_ghast(world, &fossil);
    }
}

impl NetherFossilPiece {
    /// `placeDriedGhast`. The widened chunk box holds the whole fossil, so its inside check
    /// always passes.
    fn place_dried_ghast(&self, world: &mut PieceWorld<'_, '_>, fossil: &BoundingBox) {
        let center = BlockPos::new(
            fossil.min_x + (fossil.max_x - fossil.min_x + 1) / 2,
            fossil.min_y + (fossil.max_y - fossil.min_y + 1) / 2,
            fossil.min_z + (fossil.max_z - fossil.min_z + 1) / 2,
        );
        let factory =
            LegacyPositionalRandomFactory::new(LegacyRandom::new(self.template.seed()).next_long());
        let mut random = factory.at(center.x, center.y, center.z);
        if random.next_float() >= 0.5 {
            return;
        }
        let x = fossil.min_x + random.next_int_bounded(fossil.max_x - fossil.min_x + 1);
        let y = fossil.min_y;
        let z = fossil.min_z + random.next_int_bounded(fossil.max_z - fossil.min_z + 1);
        let pos = BlockPos::new(x, y, z);
        if !world.is_air(pos) {
            return;
        }
        let ghast = world.state("dried_ghast", &[]);
        let rotation = Rotation::get_random(&mut random);
        world.set_block(pos, rotate_block_state(&ghast, rotation));
    }
}

#[cfg(test)]
mod tests {
    use oxide_core::{BlockPos, BlockState, ResourceLocation};

    use super::super::template_piece::{oracle, BaseColumns};
    use super::UniformHeight;

    /// Solid up to the saved piece's Y, air above: the scan down from any height above it
    /// lands where the game's did, so the start's draws can be compared.
    struct Floor(i32);

    impl BaseColumns for Floor {
        fn block(&self, _x: i32, y: i32, _z: i32) -> BlockState {
            let name = if y <= self.0 { "netherrack" } else { "air" };
            BlockState::new(ResourceLocation::minecraft(name))
        }
        fn cold_enough_to_snow(&self, _pos: BlockPos) -> bool {
            false
        }
    }

    #[test]
    fn layouts_match_vanilla_starts() {
        let height = UniformHeight {
            min_inclusive: 32,
            max_inclusive: 125,
        };
        let terrain = oracle::Flat {
            height: 64,
            sea_level: 32,
        };
        oracle::check_layouts_at("minecraft:nether_fossil", terrain, |ctx, assets, saved| {
            let floor = Floor(saved[0].position[1]);
            let (_, piece) = super::locate(ctx, assets, &floor, height)?;
            Some(vec![piece.template])
        });
    }
}
