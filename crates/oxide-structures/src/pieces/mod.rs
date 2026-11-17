//! Piece-built ("legacy") structures: everything vanilla assembles from hand-written
//! `StructurePiece` subclasses rather than jigsaw pools.
//!
//! A structure is two steps, as in vanilla:
//!
//! 1. **Generation** (`Structure.findGenerationPoint`): from the world seed and the start chunk
//!    it lays out its pieces — boxes, orientations and per-piece choices — drawing from the
//!    start's [`GenerationContext::random`]. Nothing here touches blocks.
//! 2. **Placement** (`StructureStart.placeInChunk`): for one chunk, every piece whose box meets
//!    the chunk writes its blocks clipped to it ([`StructurePiece::post_process`]). Pieces share
//!    the decoration random in piece order, so [`place_in_chunk`] keeps that order.
//!
//! Where structures start, reference bookkeeping, and which decoration step stamps them belong to
//! the caller; this module answers "what are the pieces" and "what does a piece write here".

pub mod igloo;
pub mod mineshaft;
pub mod shipwreck;
pub mod template_piece;

use std::fmt::Debug;

use oxide_chunkgen::decorator::block_entity;
use oxide_chunkgen::DecorationRegion;
use oxide_core::rotation::{rotate_block_state, Direction, Mirror, Rotation};
use oxide_core::{
    BiomeId, BlockPos, BlockState, ChunkPos, HeightmapType, LegacyRandom, RandomSource,
    ResourceLocation,
};
use oxide_datapack::BlockFact;

use crate::BoundingBox;

/// What generation may read of the world: the same things vanilla's `GenerationContext` hands a
/// structure (the chunk generator's noise heights and biomes), nothing generated so far.
pub trait StructureTerrain: Sync {
    fn sea_level(&self) -> i32;
    fn min_y(&self) -> i32;
    /// The world's height in blocks (`LevelHeightAccessor.getHeight`).
    fn height(&self) -> i32;
    /// `ChunkGenerator.getBaseHeight`: the first free Y above the noise terrain's `kind` surface.
    fn base_height(&self, x: i32, z: i32, kind: HeightmapType) -> i32;
    /// `BiomeSource.getNoiseBiome` at quart coordinates.
    fn noise_biome(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> BiomeId;
}

/// `Structure.GenerationContext`: the start chunk, the world seed, and terrain access.
pub struct GenerationContext<'a> {
    pub seed: i64,
    pub chunk: ChunkPos,
    pub terrain: &'a dyn StructureTerrain,
}

impl GenerationContext<'_> {
    /// `GenerationContext.makeRandom`: a legacy random on the start chunk's large-feature seed.
    pub fn random(&self) -> LegacyRandom {
        let mut random = LegacyRandom::new(0);
        random.set_large_feature_seed(self.seed, self.chunk.x, self.chunk.z);
        random
    }
}

/// `Structure.afterPlace`: runs once per chunk after every piece, with all of them.
pub type AfterPlace =
    fn(&mut PieceWorld<'_, '_>, &mut dyn RandomSource, &BoundingBox, &[Box<dyn StructurePiece>]);

/// `Structure.GenerationStub` with its pieces built: `position` is what the caller tests the
/// structure's biomes at.
pub struct GeneratedStructure {
    pub position: BlockPos,
    pub pieces: Vec<Box<dyn StructurePiece>>,
    pub after_place: Option<AfterPlace>,
}

impl Debug for GeneratedStructure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedStructure")
            .field("position", &self.position)
            .field("pieces", &self.pieces)
            .finish()
    }
}

impl GeneratedStructure {
    /// `PiecesContainer.calculateBoundingBox`.
    pub fn bounding_box(&self) -> Option<BoundingBox> {
        encapsulate(self.pieces.iter().map(|piece| piece.bounding_box()))
    }
}

/// One placed piece of a structure.
///
/// `post_process` takes `&self`: a piece writes the same blocks into a chunk whichever chunks were
/// placed before it. Where vanilla keeps per-piece state across chunks it is called out at the
/// piece with a `PARITY-CHECK`.
pub trait StructurePiece: Send + Sync + Debug {
    fn bounding_box(&self) -> BoundingBox;

    /// `StructurePiece.postProcess`: write this piece's blocks inside `chunk_box`. `reference` is
    /// `placeInChunk`'s reference position (centre of the first piece, at its floor).
    fn post_process(
        &self,
        world: &mut PieceWorld<'_, '_>,
        random: &mut dyn RandomSource,
        chunk_box: &BoundingBox,
        reference: BlockPos,
    );
}

/// `StructureStart.placeInChunk`: every piece meeting `chunk_box`, in order, then `afterPlace`.
pub fn place_in_chunk(
    structure: &GeneratedStructure,
    world: &mut PieceWorld<'_, '_>,
    random: &mut dyn RandomSource,
    chunk_box: &BoundingBox,
) {
    let Some(first) = structure.pieces.first() else {
        return;
    };
    let first = first.bounding_box();
    let middle = center(&first);
    let reference = BlockPos::new(middle.x, first.min_y, middle.z);
    for piece in &structure.pieces {
        if piece.bounding_box().intersects(chunk_box) {
            piece.post_process(world, random, chunk_box, reference);
        }
    }
    if let Some(after_place) = structure.after_place {
        after_place(world, random, chunk_box, &structure.pieces);
    }
}

/// `WorldGenRegion.getWritableArea` for a chunk: its columns, the full world height.
pub fn chunk_box(chunk: ChunkPos, min_y: i32, max_y: i32) -> BoundingBox {
    BoundingBox::new(
        chunk.x * 16,
        min_y,
        chunk.z * 16,
        chunk.x * 16 + 15,
        max_y - 1,
        chunk.z * 16 + 15,
    )
}

/// An entity a piece spawned (`addFreshEntity`), as the SNBT vanilla saves it.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureEntity {
    pub pos: [f64; 3],
    pub snbt: String,
}

/// The level a piece writes into: the decoration window plus a sink for spawned entities.
pub struct PieceWorld<'r, 'a> {
    pub region: &'r mut DecorationRegion<'a>,
    pub entities: &'r mut Vec<StructureEntity>,
}

impl PieceWorld<'_, '_> {
    /// `block`'s default state with `properties` set, as `defaultBlockState().setValue(..)`.
    pub fn state(&self, block: &str, properties: &[(&str, &str)]) -> BlockState {
        let mut state = BlockState::new(ResourceLocation::minecraft(block));
        for (key, value) in properties {
            state.properties.insert((*key).into(), (*value).into());
        }
        self.region.facts().complete(&state)
    }

    pub fn bits(&self, pos: BlockPos) -> u32 {
        self.region.bits_or_void(pos)
    }

    pub fn is(&self, pos: BlockPos, fact: BlockFact) -> bool {
        self.bits(pos) & fact as u32 != 0
    }

    pub fn is_air(&self, pos: BlockPos) -> bool {
        self.is(pos, BlockFact::Air)
    }

    /// Whether the block at `pos` is `block` (a `minecraft:` path).
    pub fn is_block(&self, pos: BlockPos, block: &str) -> bool {
        self.region.block(pos).is_some_and(|state| {
            state.name.namespace() == "minecraft" && state.name.path() == block
        })
    }

    pub fn block_path(&self, pos: BlockPos) -> &str {
        self.region
            .block(pos)
            .map_or("void_air", |state| state.name.path())
    }

    /// `level.setBlock(pos, state, 2)`.
    pub fn set_block(&mut self, pos: BlockPos, state: BlockState) {
        self.region.set_block(pos, state);
    }

    pub fn min_y(&self) -> i32 {
        self.region.min_y()
    }

    pub fn max_y(&self) -> i32 {
        self.region.max_y()
    }

    pub fn height(&self, kind: HeightmapType, x: i32, z: i32) -> i32 {
        self.region
            .height(kind, x, z)
            .unwrap_or(self.region.min_y())
    }

    pub fn biome(&self, pos: BlockPos) -> Option<&BiomeId> {
        self.region.biome(pos)
    }

    /// `BlockState.isFaceSturdy(level, pos, face)`.
    ///
    /// PARITY-CHECK: the block facts carry sturdiness of the top and bottom faces only; a side
    /// face counts as sturdy when the block is a full collision cube. That is exact for the full
    /// blocks structures sit in and wrong for the odd stair or slab side.
    pub fn face_sturdy(&self, pos: BlockPos, face: Direction) -> bool {
        let fact = match face {
            Direction::Up => BlockFact::SturdyTop,
            Direction::Down => BlockFact::SturdyBottom,
            _ => BlockFact::FullCollision,
        };
        self.is(pos, fact)
    }

    /// `Block.canSupportCenter(level, pos, DOWN)`: the bottom face covers a 2x2 centre post.
    ///
    /// PARITY-CHECK: approximated as a sturdy bottom, or a block whose shape is a centred post
    /// (fences, walls, chains, bars, panes).
    pub fn supports_center_below(&self, pos: BlockPos) -> bool {
        if self.is(pos, BlockFact::SturdyBottom) {
            return true;
        }
        let path = self.block_path(pos);
        path.ends_with("_fence")
            || path.ends_with("_wall")
            || path.ends_with("_chain")
            || path.ends_with("_bars")
            || path.ends_with("_pane")
            || path == "glass_pane"
    }

    /// Fills in the block entity of the block at `pos`.
    pub fn put_block_entity(&mut self, pos: BlockPos, snbt: String) {
        self.region.put_block_entity(pos, snbt);
    }

    pub fn add_entity(&mut self, entity: StructureEntity) {
        self.entities.push(entity);
    }
}

/// `Direction.Plane.HORIZONTAL`, in its iteration order.
pub const HORIZONTAL: [Direction; 4] = [
    Direction::North,
    Direction::East,
    Direction::South,
    Direction::West,
];

/// `Direction.Plane.HORIZONTAL.getRandomDirection`: one `nextInt(4)`.
pub fn random_horizontal(random: &mut dyn RandomSource) -> Direction {
    HORIZONTAL[random.next_int_bounded(4) as usize]
}

/// `Direction.get2DDataValue`: south, west, north, east.
pub fn data_2d(direction: Direction) -> i32 {
    match direction {
        Direction::South => 0,
        Direction::West => 1,
        Direction::North => 2,
        Direction::East => 3,
        _ => -1,
    }
}

/// `StructurePiece.makeBoundingBox`: `width` runs across `direction`, `depth` along it.
pub fn make_bounding_box(
    x: i32,
    y: i32,
    z: i32,
    direction: Direction,
    width: i32,
    height: i32,
    depth: i32,
) -> BoundingBox {
    if matches!(direction, Direction::North | Direction::South) {
        BoundingBox::new(x, y, z, x + width - 1, y + height - 1, z + depth - 1)
    } else {
        BoundingBox::new(x, y, z, x + depth - 1, y + height - 1, z + width - 1)
    }
}

/// `BoundingBox.orientBox`: a box of `size` whose local `offset` corner sits at `(x, y, z)`,
/// laid out along `direction`.
#[allow(clippy::too_many_arguments)]
pub fn orient_box(
    x: i32,
    y: i32,
    z: i32,
    off_x: i32,
    off_y: i32,
    off_z: i32,
    width: i32,
    height: i32,
    depth: i32,
    direction: Direction,
) -> BoundingBox {
    match direction {
        Direction::South => BoundingBox::new(
            x + off_x,
            y + off_y,
            z + off_z,
            x + width - 1 + off_x,
            y + height - 1 + off_y,
            z + depth - 1 + off_z,
        ),
        Direction::West => BoundingBox::new(
            x - depth + 1 + off_z,
            y + off_y,
            z + off_x,
            x + off_z,
            y + height - 1 + off_y,
            z + width - 1 + off_x,
        ),
        Direction::East => BoundingBox::new(
            x + off_z,
            y + off_y,
            z + off_x,
            x + depth - 1 + off_z,
            y + height - 1 + off_y,
            z + width - 1 + off_x,
        ),
        // North, and vanilla's `default` arm.
        _ => BoundingBox::new(
            x + off_x,
            y + off_y,
            z - depth + 1 + off_z,
            x + width - 1 + off_x,
            y + height - 1 + off_y,
            z + off_z,
        ),
    }
}

pub fn x_span(b: &BoundingBox) -> i32 {
    b.max_x - b.min_x + 1
}

pub fn z_span(b: &BoundingBox) -> i32 {
    b.max_z - b.min_z + 1
}

/// `BoundingBox.getCenter`.
pub fn center(b: &BoundingBox) -> BlockPos {
    BlockPos::new(
        b.min_x + (b.max_x - b.min_x + 1) / 2,
        b.min_y + (b.max_y - b.min_y + 1) / 2,
        b.min_z + (b.max_z - b.min_z + 1) / 2,
    )
}

/// `BoundingBox.encapsulatingBoxes`.
pub fn encapsulate(boxes: impl IntoIterator<Item = BoundingBox>) -> Option<BoundingBox> {
    boxes.into_iter().reduce(|a, b| {
        BoundingBox::new(
            a.min_x.min(b.min_x),
            a.min_y.min(b.min_y),
            a.min_z.min(b.min_z),
            a.max_x.max(b.max_x),
            a.max_y.max(b.max_y),
            a.max_z.max(b.max_z),
        )
    })
}

/// `StructurePiece.findCollisionPiece`: whether any box in `boxes` meets `candidate`.
pub fn collides<'b>(
    mut boxes: impl Iterator<Item = &'b BoundingBox>,
    candidate: &BoundingBox,
) -> bool {
    boxes.any(|b| b.intersects(candidate))
}

/// `StructurePiece.SHAPE_CHECK_BLOCKS`: placed blocks marked for post-processing so they connect.
fn is_shape_check_block(path: &str) -> bool {
    matches!(
        path,
        "nether_brick_fence"
            | "torch"
            | "wall_torch"
            | "oak_fence"
            | "spruce_fence"
            | "dark_oak_fence"
            | "pale_oak_fence"
            | "acacia_fence"
            | "birch_fence"
            | "jungle_fence"
            | "ladder"
            | "iron_bars"
    )
}

/// `Mirror.mirror(Direction)`.
fn mirror_direction(direction: Direction, mirror: Mirror) -> Direction {
    match (mirror, direction) {
        (Mirror::LeftRight, Direction::North | Direction::South) => direction.opposite(),
        (Mirror::FrontBack, Direction::West | Direction::East) => direction.opposite(),
        _ => direction,
    }
}

/// `BlockState.mirror` for the properties piece-built structures place: horizontal `facing`
/// (with a door's `hinge` and a stair's curve handedness flipping too), and fence/pane
/// connection flags.
fn mirror_state(state: &mut BlockState, mirror: Mirror) {
    if mirror == Mirror::None {
        return;
    }
    if let Some(facing) = state
        .properties
        .get("facing")
        .and_then(|f| Direction::from_name(f))
    {
        let mirrored = mirror_direction(facing, mirror);
        state
            .properties
            .insert("facing".into(), mirrored.name().into());
    }
    if let Some(hinge) = state.properties.get_mut("hinge") {
        *hinge = if hinge == "left" { "right" } else { "left" }.into();
    }
    if state.name.path().ends_with("_stairs") {
        if let Some(shape) = state.properties.get_mut("shape") {
            let flipped = match shape.as_str() {
                "inner_left" => Some("inner_right"),
                "inner_right" => Some("inner_left"),
                "outer_left" => Some("outer_right"),
                "outer_right" => Some("outer_left"),
                _ => None,
            };
            if let Some(flipped) = flipped {
                *shape = flipped.into();
            }
        }
    }
    let (a, b) = match mirror {
        Mirror::LeftRight => ("north", "south"),
        _ => ("west", "east"),
    };
    if let (Some(va), Some(vb)) = (
        state.properties.get(a).cloned(),
        state.properties.get(b).cloned(),
    ) {
        state.properties.insert(a.into(), vb);
        state.properties.insert(b.into(), va);
    }
}

/// `BlockState.rotate`: the shared directional properties, plus a straight rail's `shape`.
fn rotate_state(state: &BlockState, rotation: Rotation) -> BlockState {
    let mut rotated = rotate_block_state(state, rotation);
    if matches!(
        rotation,
        Rotation::Clockwise90 | Rotation::Counterclockwise90
    ) && state.name.path().ends_with("rail")
    {
        if let Some(shape) = rotated.properties.get_mut("shape") {
            let turned = match shape.as_str() {
                "north_south" => Some("east_west"),
                "east_west" => Some("north_south"),
                _ => None,
            };
            if let Some(turned) = turned {
                *shape = turned.into();
            }
        }
    }
    rotated
}

/// `StructurePiece.BlockSelector`: called as `(random, x, y, z, on_edge)` for each block of a box.
pub type BlockSelector<'s> =
    dyn FnMut(&mut dyn RandomSource, i32, i32, i32, bool) -> BlockState + 's;

/// A piece's orientation and box: `StructurePiece`'s coordinate transform and block helpers.
///
/// Coordinates passed to the helpers are piece-local; `keep` lists the blocks `placeBlock` must
/// not overwrite (`StructurePiece.canBeReplaced`).
#[derive(Debug, Clone, Copy)]
pub struct PieceFrame {
    pub bounding_box: BoundingBox,
    pub orientation: Option<Direction>,
    pub mirror: Mirror,
    pub rotation: Rotation,
    pub keep: &'static [&'static str],
}

impl PieceFrame {
    /// `StructurePiece.setOrientation`.
    pub fn new(bounding_box: BoundingBox, orientation: Option<Direction>) -> Self {
        let (mirror, rotation) = match orientation {
            Some(Direction::South) => (Mirror::LeftRight, Rotation::None),
            Some(Direction::West) => (Mirror::LeftRight, Rotation::Clockwise90),
            Some(Direction::East) => (Mirror::None, Rotation::Clockwise90),
            _ => (Mirror::None, Rotation::None),
        };
        Self {
            bounding_box,
            orientation,
            mirror,
            rotation,
            keep: &[],
        }
    }

    pub fn with_keep(mut self, keep: &'static [&'static str]) -> Self {
        self.keep = keep;
        self
    }

    pub fn world_x(&self, x: i32, z: i32) -> i32 {
        match self.orientation {
            Some(Direction::North | Direction::South) => self.bounding_box.min_x + x,
            Some(Direction::West) => self.bounding_box.max_x - z,
            Some(Direction::East) => self.bounding_box.min_x + z,
            _ => x,
        }
    }

    pub fn world_y(&self, y: i32) -> i32 {
        match self.orientation {
            None => y,
            Some(_) => y + self.bounding_box.min_y,
        }
    }

    pub fn world_z(&self, x: i32, z: i32) -> i32 {
        match self.orientation {
            Some(Direction::North) => self.bounding_box.max_z - z,
            Some(Direction::South) => self.bounding_box.min_z + z,
            Some(Direction::West | Direction::East) => self.bounding_box.min_z + x,
            _ => z,
        }
    }

    pub fn world_pos(&self, x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos::new(self.world_x(x, z), self.world_y(y), self.world_z(x, z))
    }

    /// `StructurePiece.placeBlock`. Answers whether the block was written.
    pub fn place(
        &self,
        world: &mut PieceWorld<'_, '_>,
        state: &BlockState,
        x: i32,
        y: i32,
        z: i32,
        chunk_box: &BoundingBox,
    ) -> bool {
        let pos = self.world_pos(x, y, z);
        if !chunk_box.contains(pos) {
            return false;
        }
        if !self.keep.is_empty() && self.keep.contains(&world.block_path(pos)) {
            return false;
        }
        let mut state = state.clone();
        mirror_state(&mut state, self.mirror);
        if self.rotation != Rotation::None {
            state = rotate_state(&state, self.rotation);
        }
        let shape_check = is_shape_check_block(state.name.path());
        world.set_block(pos, state);
        let bits = world.bits(pos);
        if bits & (BlockFact::Water as u32 | BlockFact::Lava as u32) != 0 {
            let water = bits & BlockFact::Water as u32 != 0;
            let source = bits & BlockFact::FluidSource as u32 != 0;
            let fluid = match (water, source) {
                (true, true) => "water",
                (true, false) => "flowing_water",
                (false, true) => "lava",
                (false, false) => "flowing_lava",
            };
            world
                .region
                .schedule_fluid_tick(pos, ResourceLocation::minecraft(fluid));
        }
        if shape_check {
            world.region.mark_post_process(pos);
        }
        true
    }

    /// `StructurePiece.getBlock`'s air test: outside `chunk_box` everything reads as air.
    pub fn is_air(
        &self,
        world: &PieceWorld<'_, '_>,
        x: i32,
        y: i32,
        z: i32,
        chunk_box: &BoundingBox,
    ) -> bool {
        let pos = self.world_pos(x, y, z);
        !chunk_box.contains(pos) || world.is_air(pos)
    }

    /// `StructurePiece.getBlock`'s fact bits (air's outside `chunk_box`).
    pub fn bits(
        &self,
        world: &PieceWorld<'_, '_>,
        x: i32,
        y: i32,
        z: i32,
        chunk_box: &BoundingBox,
    ) -> u32 {
        let pos = self.world_pos(x, y, z);
        if chunk_box.contains(pos) {
            world.bits(pos)
        } else {
            BlockFact::Air as u32
        }
    }

    /// `StructurePiece.getBlock(..).is(block)`.
    pub fn is_block(
        &self,
        world: &PieceWorld<'_, '_>,
        x: i32,
        y: i32,
        z: i32,
        chunk_box: &BoundingBox,
        block: &str,
    ) -> bool {
        let pos = self.world_pos(x, y, z);
        chunk_box.contains(pos) && world.is_block(pos, block)
    }

    /// `StructurePiece.isInterior`: the block above is under the ocean-floor heightmap.
    pub fn is_interior(
        &self,
        world: &PieceWorld<'_, '_>,
        x: i32,
        y: i32,
        z: i32,
        chunk_box: &BoundingBox,
    ) -> bool {
        let pos = self.world_pos(x, y + 1, z);
        chunk_box.contains(pos) && pos.y < world.height(HeightmapType::OceanFloorWg, pos.x, pos.z)
    }

    /// `StructurePiece.generateBox` with an edge and a fill state.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_box(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        (x0, y0, z0): (i32, i32, i32),
        (x1, y1, z1): (i32, i32, i32),
        edge: &BlockState,
        fill: &BlockState,
        skip_air: bool,
    ) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if skip_air && self.is_air(world, x, y, z, chunk_box) {
                        continue;
                    }
                    let on_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    self.place(world, if on_edge { edge } else { fill }, x, y, z, chunk_box);
                }
            }
        }
    }

    /// `StructurePiece.generateBox` with a `BlockSelector`, called as `(random, x, y, z, edge)`.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_box_with(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        (x0, y0, z0): (i32, i32, i32),
        (x1, y1, z1): (i32, i32, i32),
        skip_air: bool,
        random: &mut dyn RandomSource,
        selector: &mut BlockSelector<'_>,
    ) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if skip_air && self.is_air(world, x, y, z, chunk_box) {
                        continue;
                    }
                    let on_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    let state = selector(random, x, y, z, on_edge);
                    self.place(world, &state, x, y, z, chunk_box);
                }
            }
        }
    }

    /// `StructurePiece.generateAirBox`.
    pub fn generate_air_box(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        (x0, y0, z0): (i32, i32, i32),
        (x1, y1, z1): (i32, i32, i32),
    ) {
        let air = world.state("air", &[]);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    self.place(world, &air, x, y, z, chunk_box);
                }
            }
        }
    }

    /// `StructurePiece.generateMaybeBox`.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_maybe_box(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        random: &mut dyn RandomSource,
        probability: f32,
        (x0, y0, z0): (i32, i32, i32),
        (x1, y1, z1): (i32, i32, i32),
        edge: &BlockState,
        fill: &BlockState,
        skip_air: bool,
        has_to_be_inside: bool,
    ) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if random.next_float() > probability
                        || skip_air && self.is_air(world, x, y, z, chunk_box)
                        || has_to_be_inside && !self.is_interior(world, x, y, z, chunk_box)
                    {
                        continue;
                    }
                    let on_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    self.place(world, if on_edge { edge } else { fill }, x, y, z, chunk_box);
                }
            }
        }
    }

    /// `StructurePiece.maybeGenerateBlock`.
    #[allow(clippy::too_many_arguments)]
    pub fn maybe_place(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        random: &mut dyn RandomSource,
        probability: f32,
        x: i32,
        y: i32,
        z: i32,
        state: &BlockState,
    ) {
        if random.next_float() < probability {
            self.place(world, state, x, y, z, chunk_box);
        }
    }

    /// `StructurePiece.generateUpperHalfSphere`.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_upper_half_sphere(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        (x0, y0, z0): (i32, i32, i32),
        (x1, y1, z1): (i32, i32, i32),
        fill: &BlockState,
        skip_air: bool,
    ) {
        let diag_x = (x1 - x0 + 1) as f32;
        let diag_y = (y1 - y0 + 1) as f32;
        let diag_z = (z1 - z0 + 1) as f32;
        let cx = x0 as f32 + diag_x / 2.0;
        let cz = z0 as f32 + diag_z / 2.0;
        for y in y0..=y1 {
            let ny = (y - y0) as f32 / diag_y;
            for x in x0..=x1 {
                let nx = (x as f32 - cx) / (diag_x * 0.5);
                for z in z0..=z1 {
                    let nz = (z as f32 - cz) / (diag_z * 0.5);
                    if skip_air && self.is_air(world, x, y, z, chunk_box) {
                        continue;
                    }
                    if nx * nx + ny * ny + nz * nz <= 1.05 {
                        self.place(world, fill, x, y, z, chunk_box);
                    }
                }
            }
        }
    }

    /// `StructurePiece.fillColumnDown`.
    pub fn fill_column_down(
        &self,
        world: &mut PieceWorld<'_, '_>,
        state: &BlockState,
        x: i32,
        y: i32,
        z: i32,
        chunk_box: &BoundingBox,
    ) {
        let mut pos = self.world_pos(x, y, z);
        if !chunk_box.contains(pos) {
            return;
        }
        while is_replaceable_by_structures(world, pos) && pos.y > world.min_y() + 1 {
            world.set_block(pos, state.clone());
            pos.y -= 1;
        }
    }

    /// `StructurePiece.createChest` at piece coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn create_chest(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        random: &mut dyn RandomSource,
        x: i32,
        y: i32,
        z: i32,
        loot_table: &str,
    ) -> bool {
        create_chest(
            world,
            chunk_box,
            random,
            self.world_pos(x, y, z),
            loot_table,
            None,
        )
    }

    /// `StructurePiece.createDispenser`.
    #[allow(clippy::too_many_arguments)]
    pub fn create_dispenser(
        &self,
        world: &mut PieceWorld<'_, '_>,
        chunk_box: &BoundingBox,
        random: &mut dyn RandomSource,
        x: i32,
        y: i32,
        z: i32,
        facing: Direction,
        loot_table: &str,
    ) -> bool {
        let pos = self.world_pos(x, y, z);
        if !chunk_box.contains(pos) || world.is_block(pos, "dispenser") {
            return false;
        }
        let state = world.state("dispenser", &[("facing", facing.name())]);
        if self.place(world, &state, x, y, z, chunk_box) {
            let seed = random.next_long();
            let snbt = block_entity::with_loot_table("minecraft:dispenser", pos, loot_table, seed);
            world.put_block_entity(pos, snbt);
        }
        true
    }

    /// A spawner of `entity` at piece coordinates, written straight to the level as the pieces do
    /// (`level.setBlock(pos, SPAWNER, 2)` then `setEntityId`).
    pub fn place_spawner(&self, world: &mut PieceWorld<'_, '_>, pos: BlockPos, entity: &str) {
        let spawner = world.state("spawner", &[]);
        world.set_block(pos, spawner);
        world.put_block_entity(pos, block_entity::spawner(pos, entity));
    }
}

/// `StructurePiece.isReplaceableByStructures`.
pub fn is_replaceable_by_structures(world: &PieceWorld<'_, '_>, pos: BlockPos) -> bool {
    let bits = world.bits(pos);
    bits & (BlockFact::Air as u32 | BlockFact::Liquid as u32) != 0
        || matches!(
            world.block_path(pos),
            "glow_lichen" | "seagrass" | "tall_seagrass"
        )
}

/// `StructurePiece.reorient`: a chest's facing, turned away from its one solid neighbour.
fn reorient(world: &PieceWorld<'_, '_>, pos: BlockPos, facing: Direction) -> Direction {
    let at = |d: Direction| BlockPos::new(pos.x + d.step_x(), pos.y, pos.z + d.step_z());
    let mut solid_neighbour = None;
    for direction in HORIZONTAL {
        let relative = at(direction);
        if world.is_block(relative, "chest") {
            return facing;
        }
        if !world.is(relative, BlockFact::SolidRender) {
            continue;
        }
        if solid_neighbour.is_none() {
            solid_neighbour = Some(direction);
            continue;
        }
        solid_neighbour = None;
        break;
    }
    if let Some(solid) = solid_neighbour {
        return solid.opposite();
    }
    let mut lock = facing;
    if world.is(at(lock), BlockFact::SolidRender) {
        lock = lock.opposite();
    }
    if world.is(at(lock), BlockFact::SolidRender) {
        lock = lock.clockwise();
    }
    if world.is(at(lock), BlockFact::SolidRender) {
        lock = lock.opposite();
    }
    lock
}

/// `StructurePiece.createChest` at a world position. `state` is the chest to place; `None` is a
/// plain chest reoriented against its neighbours.
pub fn create_chest(
    world: &mut PieceWorld<'_, '_>,
    chunk_box: &BoundingBox,
    random: &mut dyn RandomSource,
    pos: BlockPos,
    loot_table: &str,
    state: Option<BlockState>,
) -> bool {
    if !chunk_box.contains(pos) || world.is_block(pos, "chest") {
        return false;
    }
    let state = state.unwrap_or_else(|| {
        let facing = reorient(world, pos, Direction::North);
        world.state("chest", &[("facing", facing.name())])
    });
    world.set_block(pos, state);
    let seed = random.next_long();
    let snbt = block_entity::with_loot_table("minecraft:chest", pos, loot_table, seed);
    world.put_block_entity(pos, snbt);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orient_box_matches_vanilla_layout() {
        // `BoundingBox.orientBox(10, 64, 20, -1, -1, 0, 5, 5, 7, dir)`, from the 26.2 source.
        assert_eq!(
            orient_box(10, 64, 20, -1, -1, 0, 5, 5, 7, Direction::North),
            BoundingBox::new(9, 63, 14, 13, 67, 20)
        );
        assert_eq!(
            orient_box(10, 64, 20, -1, -1, 0, 5, 5, 7, Direction::South),
            BoundingBox::new(9, 63, 20, 13, 67, 26)
        );
        assert_eq!(
            orient_box(10, 64, 20, -1, -1, 0, 5, 5, 7, Direction::West),
            BoundingBox::new(4, 63, 19, 10, 67, 23)
        );
        assert_eq!(
            orient_box(10, 64, 20, -1, -1, 0, 5, 5, 7, Direction::East),
            BoundingBox::new(10, 63, 19, 16, 67, 23)
        );
    }

    #[test]
    fn frame_maps_local_to_world_per_orientation() {
        let b = BoundingBox::new(100, 10, 200, 104, 14, 210);
        let north = PieceFrame::new(b, Some(Direction::North));
        assert_eq!(north.world_pos(1, 2, 3), BlockPos::new(101, 12, 207));
        let west = PieceFrame::new(b, Some(Direction::West));
        assert_eq!(west.world_pos(1, 2, 3), BlockPos::new(101, 12, 201));
        assert_eq!(west.mirror, Mirror::LeftRight);
        assert_eq!(west.rotation, Rotation::Clockwise90);
    }

    #[test]
    fn mirror_flips_facing_hinge_and_connections() {
        let mut door = BlockState::new(ResourceLocation::minecraft("iron_door"))
            .with_property("facing", "north")
            .with_property("hinge", "left");
        mirror_state(&mut door, Mirror::LeftRight);
        assert_eq!(door.properties["facing"], "south");
        assert_eq!(door.properties["hinge"], "right");
        let mut bars = BlockState::new(ResourceLocation::minecraft("iron_bars"))
            .with_property("north", "true")
            .with_property("south", "false");
        mirror_state(&mut bars, Mirror::LeftRight);
        assert_eq!(bars.properties["north"], "false");
        assert_eq!(bars.properties["south"], "true");
    }
}
