//! `IglooStructure` and `IglooPieces`.

use oxide_chunkgen::decorator::Processor;
use oxide_core::rotation::Rotation;
use oxide_core::{BlockPos, HeightmapType, RandomSource, ResourceLocation};

use super::template_piece::{minecraft, offset, set_loot_table, TemplateAssets, TemplatePiece};
use super::{GeneratedStructure, GenerationContext, PieceWorld, StructurePiece};
use crate::BoundingBox;

const GENERATION_HEIGHT: i32 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Top,
    Ladder,
    Laboratory,
}

impl Part {
    fn id(self) -> ResourceLocation {
        minecraft(match self {
            Part::Top => "igloo/top",
            Part::Ladder => "igloo/middle",
            Part::Laboratory => "igloo/bottom",
        })
    }

    fn pivot(self) -> BlockPos {
        match self {
            Part::Top => BlockPos::new(3, 5, 5),
            Part::Ladder => BlockPos::new(1, 3, 1),
            Part::Laboratory => BlockPos::new(3, 6, 7),
        }
    }

    fn offset(self) -> BlockPos {
        match self {
            Part::Top => BlockPos::new(0, 0, 0),
            Part::Ladder => BlockPos::new(2, -3, 4),
            Part::Laboratory => BlockPos::new(0, -3, -2),
        }
    }
}

/// `IglooStructure.findGenerationPoint`.
pub fn generate(ctx: &GenerationContext<'_>, assets: &TemplateAssets<'_>) -> GeneratedStructure {
    let (x, z) = (ctx.chunk.x * 16 + 8, ctx.chunk.z * 16 + 8);
    let height = ctx.terrain.base_height(x, z, HeightmapType::WorldSurfaceWg);
    GeneratedStructure {
        position: BlockPos::new(x, height - 1, z),
        pieces: pieces(ctx, assets)
            .into_iter()
            .map(|piece| Box::new(piece) as Box<dyn StructurePiece>)
            .collect(),
        after_place: None,
    }
}

/// `IglooStructure.generatePieces`.
fn pieces(ctx: &GenerationContext<'_>, assets: &TemplateAssets<'_>) -> Vec<IglooPiece> {
    let mut random = ctx.random();
    let (x, z) = (ctx.chunk.x * 16, ctx.chunk.z * 16);
    let start = BlockPos::new(x, GENERATION_HEIGHT, z);
    let rotation = Rotation::get_random(&mut random);
    let mut pieces = Vec::new();
    let mut add = |part: Part, depth: i32| {
        pieces.push(IglooPiece::new(
            ctx.seed, assets, part, start, rotation, depth,
        ));
    };
    if random.next_double() < 0.5 {
        let depth = random.next_int_bounded(8) + 4;
        add(Part::Laboratory, depth * 3);
        for i in 0..depth - 1 {
            add(Part::Ladder, i * 3);
        }
    }
    add(Part::Top, 0);
    pieces
}

#[derive(Debug)]
pub struct IglooPiece {
    part: Part,
    template: TemplatePiece,
}

impl IglooPiece {
    fn new(
        seed: i64,
        assets: &TemplateAssets<'_>,
        part: Part,
        start: BlockPos,
        rotation: Rotation,
        depth: i32,
    ) -> Self {
        let shift = part.offset();
        let position = offset(start, shift.x, shift.y - depth, shift.z);
        let mut template = TemplatePiece::new(
            assets,
            seed,
            part.id(),
            position,
            rotation,
            vec![Processor::BlockIgnore(vec![minecraft("structure_block")])],
        );
        template.pivot = part.pivot();
        template.apply_waterlogging = false;
        Self { part, template }
    }
}

impl StructurePiece for IglooPiece {
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
        let t = &self.template;
        let shift = self.part.offset();
        let entrance = t.relative(BlockPos::new(3 - shift.x, 0, -shift.z));
        let entrance = offset(t.position, entrance.x, entrance.y, entrance.z);
        let height = world.height(HeightmapType::WorldSurfaceWg, entrance.x, entrance.z);
        let position = offset(t.position, 0, height - GENERATION_HEIGHT - 1, 0);
        t.place(
            world,
            random,
            chunk_box,
            position,
            |world, random, marker, pos| {
                if marker != "chest" {
                    return;
                }
                world.set_block(pos, world.state("air", &[]));
                set_loot_table(
                    world,
                    random,
                    offset(pos, 0, -1, 0),
                    "minecraft:chests/igloo_chest",
                );
            },
        );
        if self.part != Part::Top {
            return;
        }
        let trap_door = t.relative(BlockPos::new(3, 0, 5));
        let trap_door = offset(position, trap_door.x, trap_door.y, trap_door.z);
        let below = offset(trap_door, 0, -1, 0);
        if !world.is_air(below) && !world.is_block(below, "ladder") {
            world.set_block(trap_door, world.state("snow_block", &[]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::template_piece::oracle;

    #[test]
    fn layouts_match_vanilla_starts() {
        oracle::check_layouts("minecraft:igloo", |ctx, assets, _| {
            Some(
                super::pieces(ctx, assets)
                    .into_iter()
                    .map(|piece| piece.template)
                    .collect(),
            )
        });
    }
}
