//! Mineshafts, normal and mesa: 26.2's `MineshaftStructure` and `MineshaftPieces`.

use oxide_core::rotation::Direction;
use oxide_core::{BlockPos, BlockState, HeightmapType, RandomSource};
use oxide_datapack::BlockFact;

use super::{
    center, collides, encapsulate, is_replaceable_by_structures, x_span, z_span,
    GeneratedStructure, GenerationContext, PieceFrame, PieceWorld, StructureEntity, StructurePiece,
};
use crate::BoundingBox;

const MAX_DEPTH: i32 = 8;
const MAX_PILLAR_HEIGHT: i32 = 20;
const MAX_CHAIN_HEIGHT: i32 = 50;
const LOOT_TABLE: &str = "minecraft:chests/abandoned_mineshaft";

/// `MineshaftStructure.Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MineshaftType {
    Normal,
    Mesa,
}

impl MineshaftType {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "normal" => Some(Self::Normal),
            "mesa" => Some(Self::Mesa),
            _ => None,
        }
    }

    fn wood(self) -> &'static str {
        match self {
            Self::Normal => "oak_log",
            Self::Mesa => "dark_oak_log",
        }
    }

    fn planks(self) -> &'static str {
        match self {
            Self::Normal => "oak_planks",
            Self::Mesa => "dark_oak_planks",
        }
    }

    fn fence(self) -> &'static str {
        match self {
            Self::Normal => "oak_fence",
            Self::Mesa => "dark_oak_fence",
        }
    }

    /// `MineShaftPiece.canBeReplaced`: the type's wood, planks and fence, and chains, stay.
    fn keep(self) -> &'static [&'static str] {
        match self {
            Self::Normal => &["oak_planks", "oak_log", "oak_fence", "iron_chain"],
            Self::Mesa => &[
                "dark_oak_planks",
                "dark_oak_log",
                "dark_oak_fence",
                "iron_chain",
            ],
        }
    }
}

#[derive(Debug, Clone)]
enum Kind {
    Room {
        /// Where children open into the room: `childEntranceBoxes`.
        entrances: Vec<BoundingBox>,
    },
    Corridor {
        has_rails: bool,
        spider_corridor: bool,
        num_sections: i32,
    },
    Crossing {
        direction: Direction,
        two_floored: bool,
    },
    Stairs,
}

#[derive(Debug, Clone)]
pub struct MineshaftPiece {
    frame: PieceFrame,
    gen_depth: i32,
    ty: MineshaftType,
    kind: Kind,
}

impl MineshaftPiece {
    fn new(
        bounding_box: BoundingBox,
        orientation: Option<Direction>,
        gen_depth: i32,
        ty: MineshaftType,
        kind: Kind,
    ) -> Self {
        Self {
            frame: PieceFrame::new(bounding_box, orientation).with_keep(ty.keep()),
            gen_depth,
            ty,
            kind,
        }
    }

    fn bb(&self) -> BoundingBox {
        self.frame.bounding_box
    }

    fn shift_y(&mut self, dy: i32) {
        self.frame.bounding_box = self.frame.bounding_box.moved(0, dy, 0);
        if let Kind::Room { entrances } = &mut self.kind {
            for entrance in entrances {
                *entrance = entrance.moved(0, dy, 0);
            }
        }
    }
}

/// `MineshaftStructure.findGenerationPoint`.
pub fn generate(ctx: &GenerationContext<'_>, ty: MineshaftType) -> GeneratedStructure {
    let mut random = ctx.random();
    random.next_double();
    let chunk = ctx.chunk;
    let mut pieces = Vec::new();
    let room_box = {
        let west = chunk.x * 16 + 2;
        let north = chunk.z * 16 + 2;
        let max_x = west + 7 + random.next_int_bounded(6);
        let max_y = 54 + random.next_int_bounded(6);
        let max_z = north + 7 + random.next_int_bounded(6);
        BoundingBox::new(west, 50, north, max_x, max_y, max_z)
    };
    pieces.push(MineshaftPiece::new(
        room_box,
        None,
        0,
        ty,
        Kind::Room {
            entrances: Vec::new(),
        },
    ));
    add_room_children(&mut pieces, &mut random);

    let sea_level = ctx.terrain.sea_level();
    let bounds = encapsulate(pieces.iter().map(MineshaftPiece::bb)).expect("the room is a piece");
    let dy = match ty {
        MineshaftType::Mesa => {
            let middle = center(&bounds);
            let surface =
                ctx.terrain
                    .base_height(middle.x, middle.z, HeightmapType::WorldSurfaceWg);
            let target = if surface <= sea_level {
                sea_level
            } else {
                random.next_int_bounded(surface - sea_level + 1) + sea_level
            };
            target - middle.y
        }
        // `StructurePiecesBuilder.moveBelowSeaLevel(seaLevel, minY, random, 10)`.
        MineshaftType::Normal => {
            let max_y = sea_level - 10;
            let mut top = bounds.y_span() + ctx.terrain.min_y() + 1;
            if top < max_y {
                top += random.next_int_bounded(max_y - top);
            }
            top - bounds.max_y
        }
    };
    for piece in &mut pieces {
        piece.shift_y(dy);
    }
    GeneratedStructure {
        position: BlockPos::new(chunk.x * 16 + 8, 50 + dy, chunk.z * 16),
        pieces: pieces
            .into_iter()
            .map(|piece| Box::new(piece) as Box<dyn StructurePiece>)
            .collect(),
        after_place: None,
    }
}

/// `MineshaftPieces.generateAndAddPiece` (with `createRandomShaftPiece`): the index of the piece
/// added, if any.
#[allow(clippy::too_many_arguments)]
fn generate_and_add(
    pieces: &mut Vec<MineshaftPiece>,
    random: &mut dyn RandomSource,
    x: i32,
    y: i32,
    z: i32,
    direction: Direction,
    depth: i32,
) -> Option<usize> {
    if depth > MAX_DEPTH {
        return None;
    }
    let start = pieces[0].bb();
    if (x - start.min_x).abs() > 80 || (z - start.min_z).abs() > 80 {
        return None;
    }
    let ty = pieces[0].ty;
    let gen_depth = depth + 1;
    let selection = random.next_int_bounded(100);
    let piece = if selection >= 80 {
        find_crossing(pieces, random, x, y, z, direction).map(|b| {
            let two_floored = b.y_span() > 3;
            MineshaftPiece::new(
                b,
                None,
                gen_depth,
                ty,
                Kind::Crossing {
                    direction,
                    two_floored,
                },
            )
        })
    } else if selection >= 70 {
        find_stairs(pieces, x, y, z, direction)
            .map(|b| MineshaftPiece::new(b, Some(direction), gen_depth, ty, Kind::Stairs))
    } else {
        find_corridor(pieces, random, x, y, z, direction).map(|b| {
            let has_rails = random.next_int_bounded(3) == 0;
            let spider_corridor = !has_rails && random.next_int_bounded(23) == 0;
            let num_sections = match direction {
                Direction::North | Direction::South => z_span(&b) / 5,
                _ => x_span(&b) / 5,
            };
            let kind = Kind::Corridor {
                has_rails,
                spider_corridor,
                num_sections,
            };
            MineshaftPiece::new(b, Some(direction), gen_depth, ty, kind)
        })
    }?;
    pieces.push(piece);
    let index = pieces.len() - 1;
    add_children(pieces, index, random);
    Some(index)
}

fn add_children(pieces: &mut Vec<MineshaftPiece>, index: usize, random: &mut dyn RandomSource) {
    match pieces[index].kind {
        Kind::Room { .. } => add_room_children(pieces, random),
        Kind::Corridor { .. } => add_corridor_children(pieces, index, random),
        Kind::Crossing { .. } => add_crossing_children(pieces, index, random),
        Kind::Stairs => add_stairs_children(pieces, index, random),
    }
}

fn add_room_children(pieces: &mut Vec<MineshaftPiece>, random: &mut dyn RandomSource) {
    let b = pieces[0].bb();
    let depth = pieces[0].gen_depth;
    let height_space = (b.y_span() - 3 - 1).max(1);
    let mut entrances = Vec::new();
    // North, south, west, east walls in that order; each walks the wall in random strides.
    for side in 0..4 {
        let span = if side < 2 { x_span(&b) } else { z_span(&b) };
        let mut pos = 0;
        while pos < span {
            pos += random.next_int_bounded(span);
            if pos + 3 > span {
                break;
            }
            let y = b.min_y + random.next_int_bounded(height_space) + 1;
            let (x, z, direction) = match side {
                0 => (b.min_x + pos, b.min_z - 1, Direction::North),
                1 => (b.min_x + pos, b.max_z + 1, Direction::South),
                2 => (b.min_x - 1, b.min_z + pos, Direction::West),
                _ => (b.max_x + 1, b.min_z + pos, Direction::East),
            };
            if let Some(child) = generate_and_add(pieces, random, x, y, z, direction, depth) {
                let c = pieces[child].bb();
                entrances.push(match side {
                    0 => BoundingBox::new(c.min_x, c.min_y, b.min_z, c.max_x, c.max_y, b.min_z + 1),
                    1 => BoundingBox::new(c.min_x, c.min_y, b.max_z - 1, c.max_x, c.max_y, b.max_z),
                    2 => BoundingBox::new(b.min_x, c.min_y, c.min_z, b.min_x + 1, c.max_y, c.max_z),
                    _ => BoundingBox::new(b.max_x - 1, c.min_y, c.min_z, b.max_x, c.max_y, c.max_z),
                });
            }
            pos += 4;
        }
    }
    if let Kind::Room { entrances: room } = &mut pieces[0].kind {
        room.extend(entrances);
    }
}

fn add_corridor_children(
    pieces: &mut Vec<MineshaftPiece>,
    index: usize,
    random: &mut dyn RandomSource,
) {
    let b = pieces[index].bb();
    let depth = pieces[index].gen_depth;
    let orientation = pieces[index]
        .frame
        .orientation
        .expect("corridors are oriented");
    let end = random.next_int_bounded(4);
    let y = b.min_y - 1 + random.next_int_bounded(3);
    let (x, z, direction) = match orientation {
        Direction::South => match end {
            0 | 1 => (b.min_x, b.max_z + 1, orientation),
            2 => (b.min_x - 1, b.max_z - 3, Direction::West),
            _ => (b.max_x + 1, b.max_z - 3, Direction::East),
        },
        Direction::West => match end {
            0 | 1 => (b.min_x - 1, b.min_z, orientation),
            2 => (b.min_x, b.min_z - 1, Direction::North),
            _ => (b.min_x, b.max_z + 1, Direction::South),
        },
        Direction::East => match end {
            0 | 1 => (b.max_x + 1, b.min_z, orientation),
            2 => (b.max_x - 3, b.min_z - 1, Direction::North),
            _ => (b.max_x - 3, b.max_z + 1, Direction::South),
        },
        _ => match end {
            0 | 1 => (b.min_x, b.min_z - 1, orientation),
            2 => (b.min_x - 1, b.min_z, Direction::West),
            _ => (b.max_x + 1, b.min_z, Direction::East),
        },
    };
    generate_and_add(pieces, random, x, y, z, direction, depth);
    if depth >= MAX_DEPTH {
        return;
    }
    if matches!(orientation, Direction::North | Direction::South) {
        let mut z = b.min_z + 3;
        while z + 3 <= b.max_z {
            match random.next_int_bounded(5) {
                0 => {
                    generate_and_add(
                        pieces,
                        random,
                        b.min_x - 1,
                        b.min_y,
                        z,
                        Direction::West,
                        depth + 1,
                    );
                }
                1 => {
                    generate_and_add(
                        pieces,
                        random,
                        b.max_x + 1,
                        b.min_y,
                        z,
                        Direction::East,
                        depth + 1,
                    );
                }
                _ => {}
            }
            z += 5;
        }
    } else {
        let mut x = b.min_x + 3;
        while x + 3 <= b.max_x {
            match random.next_int_bounded(5) {
                0 => {
                    generate_and_add(
                        pieces,
                        random,
                        x,
                        b.min_y,
                        b.min_z - 1,
                        Direction::North,
                        depth + 1,
                    );
                }
                1 => {
                    generate_and_add(
                        pieces,
                        random,
                        x,
                        b.min_y,
                        b.max_z + 1,
                        Direction::South,
                        depth + 1,
                    );
                }
                _ => {}
            }
            x += 5;
        }
    }
}

fn add_crossing_children(
    pieces: &mut Vec<MineshaftPiece>,
    index: usize,
    random: &mut dyn RandomSource,
) {
    let b = pieces[index].bb();
    let depth = pieces[index].gen_depth;
    let Kind::Crossing {
        direction,
        two_floored,
    } = pieces[index].kind
    else {
        unreachable!("called for crossings");
    };
    let north = (b.min_x + 1, b.min_z - 1, Direction::North);
    let south = (b.min_x + 1, b.max_z + 1, Direction::South);
    let west = (b.min_x - 1, b.min_z + 1, Direction::West);
    let east = (b.max_x + 1, b.min_z + 1, Direction::East);
    let exits = match direction {
        Direction::South => [south, west, east],
        Direction::West => [north, south, west],
        Direction::East => [north, south, east],
        _ => [north, west, east],
    };
    for (x, z, d) in exits {
        generate_and_add(pieces, random, x, b.min_y, z, d, depth);
    }
    if two_floored {
        for (x, z, d) in [north, west, east, south] {
            if random.next_boolean() {
                generate_and_add(pieces, random, x, b.min_y + 3 + 1, z, d, depth);
            }
        }
    }
}

fn add_stairs_children(
    pieces: &mut Vec<MineshaftPiece>,
    index: usize,
    random: &mut dyn RandomSource,
) {
    let b = pieces[index].bb();
    let depth = pieces[index].gen_depth;
    let (x, z, d) = match pieces[index].frame.orientation {
        Some(Direction::South) => (b.min_x, b.max_z + 1, Direction::South),
        Some(Direction::West) => (b.min_x - 1, b.min_z, Direction::West),
        Some(Direction::East) => (b.max_x + 1, b.min_z, Direction::East),
        _ => (b.min_x, b.min_z - 1, Direction::North),
    };
    generate_and_add(pieces, random, x, b.min_y, z, d, depth);
}

fn free(pieces: &[MineshaftPiece], candidate: &BoundingBox) -> bool {
    !collides(pieces.iter().map(|p| &p.frame.bounding_box), candidate)
}

/// `MineShaftCrossing.findCrossing`.
fn find_crossing(
    pieces: &[MineshaftPiece],
    random: &mut dyn RandomSource,
    x: i32,
    y: i32,
    z: i32,
    direction: Direction,
) -> Option<BoundingBox> {
    let top = if random.next_int_bounded(4) == 0 {
        6
    } else {
        2
    };
    let b = match direction {
        Direction::South => BoundingBox::new(-1, 0, 0, 3, top, 4),
        Direction::West => BoundingBox::new(-4, 0, -1, 0, top, 3),
        Direction::East => BoundingBox::new(0, 0, -1, 4, top, 3),
        _ => BoundingBox::new(-1, 0, -4, 3, top, 0),
    }
    .moved(x, y, z);
    free(pieces, &b).then_some(b)
}

/// `MineShaftStairs.findStairs`.
fn find_stairs(
    pieces: &[MineshaftPiece],
    x: i32,
    y: i32,
    z: i32,
    direction: Direction,
) -> Option<BoundingBox> {
    let b = match direction {
        Direction::South => BoundingBox::new(0, -5, 0, 2, 2, 8),
        Direction::West => BoundingBox::new(-8, -5, 0, 0, 2, 2),
        Direction::East => BoundingBox::new(0, -5, 0, 8, 2, 2),
        _ => BoundingBox::new(0, -5, -8, 2, 2, 0),
    }
    .moved(x, y, z);
    free(pieces, &b).then_some(b)
}

/// `MineShaftCorridor.findCorridorSize`: the longest of 2-4 sections (from a random start
/// downwards) that fits.
fn find_corridor(
    pieces: &[MineshaftPiece],
    random: &mut dyn RandomSource,
    x: i32,
    y: i32,
    z: i32,
    direction: Direction,
) -> Option<BoundingBox> {
    let mut sections = random.next_int_bounded(3) + 2;
    while sections > 0 {
        let length = sections * 5;
        let b = match direction {
            Direction::South => BoundingBox::new(0, 0, 0, 2, 2, length - 1),
            Direction::West => BoundingBox::new(-(length - 1), 0, 0, 0, 2, 2),
            Direction::East => BoundingBox::new(0, 0, 0, length - 1, 2, 2),
            _ => BoundingBox::new(0, 0, -(length - 1), 2, 2, 0),
        }
        .moved(x, y, z);
        if free(pieces, &b) {
            return Some(b);
        }
        sections -= 1;
    }
    None
}

impl StructurePiece for MineshaftPiece {
    fn bounding_box(&self) -> BoundingBox {
        self.frame.bounding_box
    }

    fn post_process(
        &self,
        world: &mut PieceWorld<'_, '_>,
        random: &mut dyn RandomSource,
        chunk_box: &BoundingBox,
        _reference: BlockPos,
    ) {
        if self.is_in_invalid_location(world, chunk_box) {
            return;
        }
        match &self.kind {
            Kind::Room { entrances } => self.place_room(world, chunk_box, entrances),
            Kind::Corridor {
                has_rails,
                spider_corridor,
                num_sections,
            } => self.place_corridor(
                world,
                random,
                chunk_box,
                *has_rails,
                *spider_corridor,
                *num_sections,
            ),
            Kind::Crossing { two_floored, .. } => {
                self.place_crossing(world, chunk_box, *two_floored)
            }
            Kind::Stairs => self.place_stairs(world, chunk_box),
        }
    }
}

impl MineshaftPiece {
    /// `MineShaftPiece.isInInvalidLocation`: deep dark, or liquid anywhere on the shell of the
    /// box grown by one and clipped to the chunk.
    fn is_in_invalid_location(&self, world: &PieceWorld<'_, '_>, chunk_box: &BoundingBox) -> bool {
        let b = self.bb();
        let x0 = (b.min_x - 1).max(chunk_box.min_x);
        let y0 = (b.min_y - 1).max(chunk_box.min_y);
        let z0 = (b.min_z - 1).max(chunk_box.min_z);
        let x1 = (b.max_x + 1).min(chunk_box.max_x);
        let y1 = (b.max_y + 1).min(chunk_box.max_y);
        let z1 = (b.max_z + 1).min(chunk_box.max_z);
        // PARITY-CHECK: vanilla reads `level.getBiome`, the fuzzy-zoomed biome; this reads the
        // stored quart. They differ only within a quart of a biome border.
        let middle = BlockPos::new((x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2);
        if world
            .biome(middle)
            .is_some_and(|biome| biome.namespace() == "minecraft" && biome.path() == "deep_dark")
        {
            return true;
        }
        let liquid = |x: i32, y: i32, z: i32| world.is(BlockPos::new(x, y, z), BlockFact::Liquid);
        for x in x0..=x1 {
            for z in z0..=z1 {
                if liquid(x, y0, z) || liquid(x, y1, z) {
                    return true;
                }
            }
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                if liquid(x, y, z0) || liquid(x, y, z1) {
                    return true;
                }
            }
        }
        for z in z0..=z1 {
            for y in y0..=y1 {
                if liquid(x0, y, z) || liquid(x1, y, z) {
                    return true;
                }
            }
        }
        false
    }

    /// `MineShaftPiece.setPlanksBlock`: planks under an interior block that is not already a floor.
    fn set_planks(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        planks: &BlockState,
        x: i32,
        y: i32,
        z: i32,
    ) {
        if !self.frame.is_interior(world, x, y, z, chunk_box) {
            return;
        }
        let pos = self.frame.world_pos(x, y, z);
        if !world.face_sturdy(pos, Direction::Up) {
            world.set_block(pos, planks.clone());
        }
    }

    fn place_room(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        entrances: &[BoundingBox],
    ) {
        let b = self.bb();
        let air = world.state("cave_air", &[]);
        let f = &self.frame;
        f.generate_box(
            world,
            chunk_box,
            (b.min_x, b.min_y + 1, b.min_z),
            (b.max_x, (b.min_y + 3).min(b.max_y), b.max_z),
            &air,
            &air,
            false,
        );
        for e in entrances {
            f.generate_box(
                world,
                chunk_box,
                (e.min_x, e.max_y - 2, e.min_z),
                (e.max_x, e.max_y, e.max_z),
                &air,
                &air,
                false,
            );
        }
        f.generate_upper_half_sphere(
            world,
            chunk_box,
            (b.min_x, b.min_y + 4, b.min_z),
            (b.max_x, b.max_y, b.max_z),
            &air,
            false,
        );
    }

    fn place_stairs(&self, world: &mut PieceWorld<'_, '_>, chunk_box: &BoundingBox) {
        let air = world.state("cave_air", &[]);
        let f = &self.frame;
        f.generate_box(world, chunk_box, (0, 5, 0), (2, 7, 1), &air, &air, false);
        f.generate_box(world, chunk_box, (0, 0, 7), (2, 2, 8), &air, &air, false);
        for i in 0..5 {
            let bottom = 5 - i - i32::from(i < 4);
            f.generate_box(
                world,
                chunk_box,
                (0, bottom, 2 + i),
                (2, 7 - i, 2 + i),
                &air,
                &air,
                false,
            );
        }
    }

    fn place_crossing(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        two_floored: bool,
    ) {
        let b = self.bb();
        let air = world.state("cave_air", &[]);
        let planks = world.state(self.ty.planks(), &[]);
        let f = &self.frame;
        if two_floored {
            f.generate_box(
                world,
                chunk_box,
                (b.min_x + 1, b.min_y, b.min_z),
                (b.max_x - 1, b.min_y + 2, b.max_z),
                &air,
                &air,
                false,
            );
            f.generate_box(
                world,
                chunk_box,
                (b.min_x, b.min_y, b.min_z + 1),
                (b.max_x, b.min_y + 2, b.max_z - 1),
                &air,
                &air,
                false,
            );
            f.generate_box(
                world,
                chunk_box,
                (b.min_x + 1, b.max_y - 2, b.min_z),
                (b.max_x - 1, b.max_y, b.max_z),
                &air,
                &air,
                false,
            );
            f.generate_box(
                world,
                chunk_box,
                (b.min_x, b.max_y - 2, b.min_z + 1),
                (b.max_x, b.max_y, b.max_z - 1),
                &air,
                &air,
                false,
            );
            f.generate_box(
                world,
                chunk_box,
                (b.min_x + 1, b.min_y + 3, b.min_z + 1),
                (b.max_x - 1, b.min_y + 3, b.max_z - 1),
                &air,
                &air,
                false,
            );
        } else {
            f.generate_box(
                world,
                chunk_box,
                (b.min_x + 1, b.min_y, b.min_z),
                (b.max_x - 1, b.max_y, b.max_z),
                &air,
                &air,
                false,
            );
            f.generate_box(
                world,
                chunk_box,
                (b.min_x, b.min_y, b.min_z + 1),
                (b.max_x, b.max_y, b.max_z - 1),
                &air,
                &air,
                false,
            );
        }
        for (x, z) in [
            (b.min_x + 1, b.min_z + 1),
            (b.min_x + 1, b.max_z - 1),
            (b.max_x - 1, b.min_z + 1),
            (b.max_x - 1, b.max_z - 1),
        ] {
            // `placeSupportPillar`: only under something.
            if !f.is_air(world, x, b.max_y + 1, z, chunk_box) {
                f.generate_box(
                    world,
                    chunk_box,
                    (x, b.min_y, z),
                    (x, b.max_y, z),
                    &planks,
                    &air,
                    false,
                );
            }
        }
        let y = b.min_y - 1;
        for x in b.min_x..=b.max_x {
            for z in b.min_z..=b.max_z {
                self.set_planks(world, chunk_box, &planks, x, y, z);
            }
        }
    }

    fn place_corridor(
        &self,
        world: &mut PieceWorld<'_, '_>,
        random: &mut dyn RandomSource,
        chunk_box: &BoundingBox,
        has_rails: bool,
        spider_corridor: bool,
        num_sections: i32,
    ) {
        let f = &self.frame;
        let length = num_sections * 5 - 1;
        let air = world.state("cave_air", &[]);
        let planks = world.state(self.ty.planks(), &[]);
        let cobweb = world.state("cobweb", &[]);
        f.generate_box(
            world,
            chunk_box,
            (0, 0, 0),
            (2, 1, length),
            &air,
            &air,
            false,
        );
        f.generate_maybe_box(
            world,
            chunk_box,
            random,
            0.8,
            (0, 2, 0),
            (2, 2, length),
            &air,
            &air,
            false,
            false,
        );
        if spider_corridor {
            f.generate_maybe_box(
                world,
                chunk_box,
                random,
                0.6,
                (0, 0, 0),
                (2, 1, length),
                &cobweb,
                &air,
                false,
                true,
            );
        }
        // PARITY-CHECK: vanilla keeps `hasPlacedSpider` on the piece across chunks, so a chunk
        // decorated after the one holding the spawner skips the spawner's `nextInt(3)` draws.
        // Here every chunk starts from "not placed", which agrees with vanilla for the chunk that
        // gets the spawner and every chunk decorated before it.
        let mut placed_spider = false;
        for section in 0..num_sections {
            let z = 2 + section * 5;
            self.place_support(world, chunk_box, random, 0, 0, z, 2, 2);
            for (probability, x, dz) in [
                (0.1, 0, -1),
                (0.1, 2, -1),
                (0.1, 0, 1),
                (0.1, 2, 1),
                (0.05, 0, -2),
                (0.05, 2, -2),
                (0.05, 0, 2),
                (0.05, 2, 2),
            ] {
                self.maybe_place_cobweb(
                    world,
                    chunk_box,
                    random,
                    probability,
                    x,
                    2,
                    z + dz,
                    &cobweb,
                );
            }
            if random.next_int_bounded(100) == 0 {
                self.create_minecart_chest(world, chunk_box, random, 2, 0, z - 1);
            }
            if random.next_int_bounded(100) == 0 {
                self.create_minecart_chest(world, chunk_box, random, 0, 0, z + 1);
            }
            if !spider_corridor || placed_spider {
                continue;
            }
            let spider_z = z - 1 + random.next_int_bounded(3);
            let pos = f.world_pos(1, 0, spider_z);
            if !chunk_box.contains(pos) || !f.is_interior(world, 1, 0, spider_z, chunk_box) {
                continue;
            }
            placed_spider = true;
            f.place_spawner(world, pos, "minecraft:cave_spider");
        }
        for x in 0..=2 {
            for z in 0..=length {
                self.set_planks(world, chunk_box, &planks, x, -1, z);
            }
        }
        self.place_double_lower_or_upper_support(world, chunk_box, 0, -1, 2);
        if num_sections > 1 {
            self.place_double_lower_or_upper_support(world, chunk_box, 0, -1, length - 2);
        }
        if has_rails {
            let rail = world.state("rail", &[("shape", "north_south")]);
            for z in 0..=length {
                let floor = f.bits(world, 1, -1, z, chunk_box);
                if floor & BlockFact::Air as u32 != 0 || floor & BlockFact::SolidRender as u32 == 0
                {
                    continue;
                }
                let probability = if f.is_interior(world, 1, 0, z, chunk_box) {
                    0.7
                } else {
                    0.9
                };
                f.maybe_place(world, chunk_box, random, probability, 1, 0, z, &rail);
            }
        }
    }

    /// `MineShaftCorridor.createChest`: a rail with a chest minecart on it.
    fn create_minecart_chest(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        random: &mut dyn RandomSource,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let pos = self.frame.world_pos(x, y, z);
        let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
        if !chunk_box.contains(pos) || !world.is_air(pos) || world.is_air(below) {
            return false;
        }
        let shape = if random.next_boolean() {
            "north_south"
        } else {
            "east_west"
        };
        let rail = world.state("rail", &[("shape", shape)]);
        self.frame.place(world, &rail, x, y, z, chunk_box);
        let seed = random.next_long();
        let entity_pos = [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5];
        // PARITY-CHECK: only the fields generation decides are written. Vanilla's saved entity
        // also carries its UUID, motion and the rest of `Entity.saveWithoutId`'s defaults.
        let snbt = format!(
            "{{id:\"minecraft:chest_minecart\",Pos:[{:?}d,{:?}d,{:?}d],LootTable:\"{LOOT_TABLE}\",LootTableSeed:{seed}L}}",
            entity_pos[0], entity_pos[1], entity_pos[2]
        );
        world.add_entity(StructureEntity {
            pos: entity_pos,
            snbt,
        });
        true
    }

    /// `MineShaftCorridor.placeSupport`: two fence posts and a beam, if there is a ceiling.
    #[allow(clippy::too_many_arguments)]
    fn place_support(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        random: &mut dyn RandomSource,
        x0: i32,
        y0: i32,
        z: i32,
        y1: i32,
        x1: i32,
    ) {
        let f = &self.frame;
        // `isSupportingBox`: every block above the beam is solid.
        if (x0..=x1).any(|x| f.is_air(world, x, y1 + 1, z, chunk_box)) {
            return;
        }
        let air = world.state("cave_air", &[]);
        let planks = world.state(self.ty.planks(), &[]);
        let fence_west = world.state(self.ty.fence(), &[("west", "true")]);
        let fence_east = world.state(self.ty.fence(), &[("east", "true")]);
        f.generate_box(
            world,
            chunk_box,
            (x0, y0, z),
            (x0, y1 - 1, z),
            &fence_west,
            &air,
            false,
        );
        f.generate_box(
            world,
            chunk_box,
            (x1, y0, z),
            (x1, y1 - 1, z),
            &fence_east,
            &air,
            false,
        );
        if random.next_int_bounded(4) == 0 {
            f.generate_box(
                world,
                chunk_box,
                (x0, y1, z),
                (x0, y1, z),
                &planks,
                &air,
                false,
            );
            f.generate_box(
                world,
                chunk_box,
                (x1, y1, z),
                (x1, y1, z),
                &planks,
                &air,
                false,
            );
        } else {
            f.generate_box(
                world,
                chunk_box,
                (x0, y1, z),
                (x1, y1, z),
                &planks,
                &air,
                false,
            );
            let south = world.state("wall_torch", &[("facing", "south")]);
            let north = world.state("wall_torch", &[("facing", "north")]);
            f.maybe_place(world, chunk_box, random, 0.05, x0 + 1, y1, z - 1, &south);
            f.maybe_place(world, chunk_box, random, 0.05, x0 + 1, y1, z + 1, &north);
        }
    }

    /// `MineShaftCorridor.maybePlaceCobWeb`.
    #[allow(clippy::too_many_arguments)]
    fn maybe_place_cobweb(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        random: &mut dyn RandomSource,
        probability: f32,
        x: i32,
        y: i32,
        z: i32,
        cobweb: &BlockState,
    ) {
        if self.frame.is_interior(world, x, y, z, chunk_box)
            && random.next_float() < probability
            && self.has_sturdy_neighbours(world, chunk_box, x, y, z, 2)
        {
            self.frame.place(world, cobweb, x, y, z, chunk_box);
        }
    }

    fn has_sturdy_neighbours(
        &self,
        world: &PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        x: i32,
        y: i32,
        z: i32,
        count: i32,
    ) -> bool {
        let origin = self.frame.world_pos(x, y, z);
        let mut sturdy = 0;
        for direction in Direction::VALUES {
            let pos = BlockPos::new(
                origin.x + direction.step_x(),
                origin.y + direction.step_y(),
                origin.z + direction.step_z(),
            );
            if chunk_box.contains(pos) && world.face_sturdy(pos, direction.opposite()) {
                sturdy += 1;
                if sturdy >= count {
                    return true;
                }
            }
        }
        false
    }

    fn place_double_lower_or_upper_support(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        x: i32,
        y: i32,
        z: i32,
    ) {
        let wood = world.state(self.ty.wood(), &[]);
        for x in [x, x + 2] {
            if self
                .frame
                .is_block(world, x, y, z, chunk_box, self.ty.planks())
            {
                self.fill_pillar_down_or_chain_up(world, &wood, x, y, z, chunk_box);
            }
        }
    }

    /// `MineShaftCorridor.fillPillarDownOrChainUp`: a log pillar down to a sturdy floor within
    /// 20 blocks, else a fence and chain up to a ceiling within 50, searching both together.
    fn fill_pillar_down_or_chain_up(
        &self,
        world: &mut PieceWorld<'_, '_>,
        pillar: &BlockState,
        x: i32,
        y: i32,
        z: i32,
        chunk_box: &BoundingBox,
    ) {
        let pos = self.frame.world_pos(x, y, z);
        if !chunk_box.contains(pos) {
            return;
        }
        let world_y = pos.y;
        let at = |y: i32| BlockPos::new(pos.x, y, pos.z);
        let mut distance = 1;
        let mut check_below = true;
        let mut check_above = true;
        while check_below || check_above {
            if check_below {
                let below = at(world_y - distance);
                let empty_below =
                    is_replaceable_by_structures(world, below) && !world.is_block(below, "lava");
                if !empty_below && world.face_sturdy(below, Direction::Up) {
                    for y in world_y - distance + 1..world_y {
                        world.set_block(at(y), pillar.clone());
                    }
                    return;
                }
                check_below =
                    distance <= MAX_PILLAR_HEIGHT && empty_below && below.y > world.min_y() + 1;
            }
            if check_above {
                let above = at(world_y + distance);
                let empty_above = is_replaceable_by_structures(world, above);
                if !empty_above
                    && world.supports_center_below(above)
                    && !is_falling_block(world.block_path(above))
                {
                    let fence = world.state(self.ty.fence(), &[]);
                    world.set_block(at(world_y + 1), fence);
                    let chain = world.state("iron_chain", &[]);
                    for y in world_y + 2..world_y + distance {
                        world.set_block(at(y), chain.clone());
                    }
                    return;
                }
                check_above =
                    distance <= MAX_CHAIN_HEIGHT && empty_above && above.y < world.max_y();
            }
            distance += 1;
        }
    }
}

/// `instanceof FallingBlock`.
fn is_falling_block(path: &str) -> bool {
    matches!(
        path,
        "sand" | "red_sand" | "gravel" | "anvil" | "chipped_anvil" | "damaged_anvil" | "dragon_egg"
    ) || path.ends_with("_concrete_powder")
}
