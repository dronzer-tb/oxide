//! A structure template as `StructureTemplate.placeInWorld` reads it, and that method.
//!
//! The jigsaw side (`oxide-structures`) keeps a template as a flat block list plus connectors.
//! Placing one block by block through processors needs more: every palette, each in vanilla's
//! *placement order* — full collision cubes first, then the other blocks, then blocks carrying
//! block-entity data, each group sorted by Y, then X, then Z (`StructureTemplate.buildInfoList`).
//! That order is observable, because `block_rot` draws one float per block from the feature's
//! random stream, so it is built here from the raw file once, when the feature is compiled.
//!
//! [`place_in_world`] is `placeInWorld` for the settings decoration uses: mirror `NONE`, pivot
//! at the origin, liquid settings `APPLY_WATERLOGGING`, entities not ignored, shape not known.
//! In order it picks a palette, runs the processors (`processBlockInfos`), writes the blocks
//! inside the bounding box, waterlogs and floods (`LiquidBlockContainer.placeLiquid`), and then
//! runs the neighbour-shape pass ([`super::shape`]).
//!
//! What the region cannot carry is tallied: block entities (`block_entity:<block>`, the block
//! itself is still placed, as for the desert well) and template entities
//! (`template_entities:<template>`). A template whose blocks carry block-entity NBT is refused
//! before anything is written (`template_block_nbt:<template>`): vanilla draws a loot-table seed
//! from the feature's stream for each one that turns out to be a `RandomizableContainer`, which
//! the exported data cannot answer, so every draw after the first such block is unknowable.
//! Scheduled ticks, post-processing marks and POI updates are not recorded, as for every family.

use std::collections::HashSet;
use std::str::FromStr;

use oxide_core::rotation::{rotate_block_state, transform, Direction, Mirror, Rotation};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::structure_template::{RawBlockState, RawStructureNbt};
use oxide_datapack::{BlockFact, BlockFacts};

use super::processor::{process_block_infos, Processor};
use super::shape::update_shapes;
use crate::decorator::context::{block_in, FeatureContext};

/// One block of a template: template-relative before processing, world position after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInfo {
    pub pos: BlockPos,
    pub state: BlockState,
    /// Whether the template gives this block block-entity NBT.
    pub has_nbt: bool,
}

/// A parsed template. [`Template::default`] is what vanilla's `getOrCreate` hands back for an
/// id with no file: no palettes and a zero size.
#[derive(Debug, Clone, Default)]
pub struct Template {
    size: [i32; 3],
    /// Each palette's blocks in placement order.
    pub(super) palettes: Vec<Vec<BlockInfo>>,
    entity_count: usize,
    has_block_nbt: bool,
}

/// `BoundingBox`, inclusive on both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundingBox {
    pub min: BlockPos,
    pub max: BlockPos,
}

impl BoundingBox {
    /// `BoundingBox.fromCorners`.
    pub fn from_corners(a: BlockPos, b: BlockPos) -> Self {
        Self {
            min: BlockPos::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)),
            max: BlockPos::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)),
        }
    }

    pub fn is_inside(&self, pos: BlockPos) -> bool {
        (self.min.x..=self.max.x).contains(&pos.x)
            && (self.min.y..=self.max.y).contains(&pos.y)
            && (self.min.z..=self.max.z).contains(&pos.z)
    }

    /// `BoundingBox.forAllCorners`, in its order.
    pub fn corners(&self) -> [BlockPos; 8] {
        let (a, b) = (self.min, self.max);
        [
            BlockPos::new(a.x, a.y, a.z),
            BlockPos::new(a.x, a.y, b.z),
            BlockPos::new(a.x, b.y, a.z),
            BlockPos::new(a.x, b.y, b.z),
            BlockPos::new(b.x, a.y, a.z),
            BlockPos::new(b.x, a.y, b.z),
            BlockPos::new(b.x, b.y, a.z),
            BlockPos::new(b.x, b.y, b.z),
        ]
    }
}

/// The parts of `StructurePlaceSettings` decoration sets. The random stream is the feature's,
/// passed to [`place_in_world`] through its context.
#[derive(Debug, Clone, Copy)]
pub struct PlaceSettings<'p> {
    pub rotation: Rotation,
    pub bounding_box: Option<BoundingBox>,
    pub processors: &'p [Processor],
}

pub(super) fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}

pub(super) fn step(pos: BlockPos, direction: Direction) -> BlockPos {
    offset(pos, direction.step_x(), direction.step_y(), direction.step_z())
}

impl Template {
    /// `StructureTemplate.load` + `loadPalette`: palette states read the way
    /// `NbtUtils.readBlockState` does (an unknown block is air, missing properties take the
    /// block's defaults), an out-of-range palette index is air, then `buildInfoList`'s order.
    pub fn from_raw(raw: &RawStructureNbt, facts: &BlockFacts) -> Self {
        let air = BlockState::new(ResourceLocation::minecraft("air"));
        let palettes = raw
            .all_palettes()
            .into_iter()
            .map(|palette| {
                let states: Vec<BlockState> =
                    palette.iter().map(|entry| read_block_state(entry, facts, &air)).collect();
                let (mut full, mut other, mut with_nbt) = (Vec::new(), Vec::new(), Vec::new());
                for entry in &raw.blocks {
                    let info = BlockInfo {
                        pos: BlockPos::new(entry.pos[0], entry.pos[1], entry.pos[2]),
                        state: states.get(entry.state).unwrap_or(&air).clone(),
                        has_nbt: entry.nbt.is_some(),
                    };
                    if info.has_nbt {
                        with_nbt.push(info);
                    } else if !facts.is(&info.state, BlockFact::DynamicShape)
                        && facts.is(&info.state, BlockFact::FullCollision)
                    {
                        full.push(info);
                    } else {
                        other.push(info);
                    }
                }
                // A stable sort, as Java's `List.sort` is.
                for list in [&mut full, &mut other, &mut with_nbt] {
                    list.sort_by_key(|info| (info.pos.y, info.pos.x, info.pos.z));
                }
                full.extend(other);
                full.extend(with_nbt);
                full
            })
            .collect();
        Self {
            size: raw.size,
            palettes,
            entity_count: raw.entities.len(),
            has_block_nbt: raw.blocks.iter().any(|entry| entry.nbt.is_some()),
        }
    }

    pub fn raw_size(&self) -> [i32; 3] {
        self.size
    }

    /// `getSize(rotation)`: X and Z swap under a quarter turn.
    pub fn size(&self, rotation: Rotation) -> [i32; 3] {
        match rotation {
            Rotation::Clockwise90 | Rotation::Counterclockwise90 => [self.size[2], self.size[1], self.size[0]],
            Rotation::None | Rotation::Clockwise180 => self.size,
        }
    }

    /// `getBoundingBox(settings, position)` with the pivot at the origin and no mirror.
    pub fn bounding_box(&self, position: BlockPos, rotation: Rotation) -> BoundingBox {
        let origin = BlockPos::new(0, 0, 0);
        let far = BlockPos::new(self.size[0] - 1, self.size[1] - 1, self.size[2] - 1);
        let a = transform(origin, Mirror::None, rotation, origin);
        let b = transform(far, Mirror::None, rotation, origin);
        let local = BoundingBox::from_corners(a, b);
        BoundingBox {
            min: offset(local.min, position.x, position.y, position.z),
            max: offset(local.max, position.x, position.y, position.z),
        }
    }

    /// `getZeroPositionWithTransform(zeroPos, Mirror.NONE, rotation)`.
    pub fn zero_position(&self, zero: BlockPos, rotation: Rotation) -> BlockPos {
        let (size_x, size_z) = (self.size[0] - 1, self.size[2] - 1);
        match rotation {
            Rotation::Counterclockwise90 => offset(zero, 0, 0, size_x),
            Rotation::Clockwise90 => offset(zero, size_z, 0, 0),
            Rotation::Clockwise180 => offset(zero, size_x, 0, size_z),
            Rotation::None => zero,
        }
    }
}

fn read_block_state(entry: &RawBlockState, facts: &BlockFacts, air: &BlockState) -> BlockState {
    let Ok(name) = ResourceLocation::from_str(&entry.name) else {
        return air.clone();
    };
    if facts.default_state(&name).is_none() {
        return air.clone();
    }
    facts.complete(&BlockState { name, properties: entry.properties.clone() })
}

/// `StructureTemplate.placeInWorld(level, position, position, settings, random, flags)`.
/// `id` names the template for the tallies. Returns what vanilla returns.
pub fn place_in_world(
    template: &Template,
    id: &ResourceLocation,
    cx: &mut FeatureContext<'_, '_, '_>,
    position: BlockPos,
    settings: &PlaceSettings<'_>,
) -> bool {
    if template.palettes.is_empty() {
        return false;
    }
    if template.has_block_nbt {
        cx.unsupported.record("template_block_nbt", &id.to_string());
        return false;
    }
    // `settings.getRandomPalette`: the settings carry the feature's random, so this draws from
    // it even when there is a single palette.
    let index = cx.random.next_int_bounded(template.palettes.len() as i32) as usize;
    let blocks = &template.palettes[index];
    if (blocks.is_empty() && template.entity_count == 0) || template.size.iter().any(|&side| side < 1) {
        return false;
    }

    let processed = process_block_infos(cx, position, settings, blocks);
    let env = cx.env;
    let facts = &env.facts;
    let mut to_fill: Vec<BlockPos> = Vec::new();
    let mut locked: HashSet<BlockPos> = HashSet::new();
    let mut placed: Vec<BlockPos> = Vec::with_capacity(processed.len());
    for info in processed {
        if settings.bounding_box.is_some_and(|bounds| !bounds.is_inside(info.pos)) {
            continue;
        }
        let previous = block_in(cx.region, info.pos).clone();
        // PARITY-CHECK: vanilla mirrors the state before rotating it; decoration's mirror is
        // always NONE, so only the rotation is applied. `rotate_block_state` covers the
        // standard directional properties (see its docs); a template block with a per-block
        // rotation rule would come out unturned. Verify by diffing a placed template against
        // the game for any template whose palette has more than `axis`/`facing`-style
        // properties (the 26.2 fossil and sulfur-spring templates do not).
        let state = rotate_block_state(&info.state, settings.rotation);
        if !cx.region.set_block(info.pos, state.clone()) {
            continue;
        }
        placed.push(info.pos);
        if facts.is(&state, BlockFact::BlockEntity) {
            cx.unsupported.record("block_entity", &state.name.to_string());
        }
        if facts.is(&state, BlockFact::FluidSource) {
            locked.insert(info.pos);
        } else if is_liquid_container(&state) {
            place_liquid(cx, info.pos, &state, &previous);
            if !facts.is(&previous, BlockFact::FluidSource) {
                to_fill.push(info.pos);
            }
        }
    }

    flood(cx, &mut to_fill, &locked);
    if !placed.is_empty() {
        update_shapes(cx, &placed);
    }
    if template.entity_count > 0 {
        cx.unsupported.record("template_entities", &id.to_string());
    }
    true
}

/// Whether `state`'s block is a `LiquidBlockContainer` that can take water.
///
/// PARITY-CHECK: read as "has a `waterlogged` property", which is what every
/// `SimpleWaterloggedBlock` carries. Containers without the property (kelp, seagrass) refuse
/// every `placeLiquid`, so leaving them out changes no block; it could only drop one extra
/// pass of [`flood`] in which nothing changes. Verify by exporting `instanceof
/// SimpleWaterloggedBlock` next to the property if a datapack adds a block that has one
/// without the other.
fn is_liquid_container(state: &BlockState) -> bool {
    state.properties.contains_key("waterlogged")
}

/// `SimpleWaterloggedBlock.placeLiquid`: a dry container standing where a water source was
/// (or next to one, from [`flood`]) becomes waterlogged. Returns whether it took the fluid.
fn place_liquid(cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, state: &BlockState, fluid: &BlockState) -> bool {
    let facts = &cx.env.facts;
    let water_source = facts.is(fluid, BlockFact::Water) && facts.is(fluid, BlockFact::FluidSource);
    if state.properties.get("waterlogged").is_some_and(|value| value == "false") && water_source {
        cx.region.set_block(pos, state.clone().with_property("waterlogged", "true"));
        return true;
    }
    false
}

/// The `toFill` loop of `placeInWorld`: each container that did not start in a source takes
/// the first unlocked source next to it (up, north, east, south, west), repeated while a pass
/// fills anything.
fn flood(cx: &mut FeatureContext<'_, '_, '_>, to_fill: &mut Vec<BlockPos>, locked: &HashSet<BlockPos>) {
    const AROUND: [Direction; 5] =
        [Direction::Up, Direction::North, Direction::East, Direction::South, Direction::West];
    let mut filled = true;
    while filled && !to_fill.is_empty() {
        filled = false;
        let mut index = 0;
        while index < to_fill.len() {
            let pos = to_fill[index];
            let mut source = block_in(cx.region, pos).clone();
            for direction in AROUND {
                if cx.env.facts.is(&source, BlockFact::FluidSource) {
                    break;
                }
                let neighbour = step(pos, direction);
                let state = block_in(cx.region, neighbour);
                if cx.env.facts.is(state, BlockFact::FluidSource) && !locked.contains(&neighbour) {
                    source = state.clone();
                }
            }
            if cx.env.facts.is(&source, BlockFact::FluidSource) {
                let state = block_in(cx.region, pos).clone();
                if is_liquid_container(&state) {
                    place_liquid(cx, pos, &state, &source);
                    filled = true;
                    to_fill.remove(index);
                    continue;
                }
            }
            index += 1;
        }
    }
}
