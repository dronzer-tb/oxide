//! A structure template as `StructureTemplate.placeInWorld` reads it, and that method.
//!
//! The jigsaw side (`oxide-structures`) keeps a template as a flat block list plus connectors.
//! Placing one block by block through processors needs more: every palette, each in vanilla's
//! *placement order* — full collision cubes first, then the other blocks, then blocks carrying
//! block-entity data, each group sorted by Y, then X, then Z (`StructureTemplate.buildInfoList`).
//! That order is observable, because `block_rot` draws one float per block from the feature's
//! random stream, so it is built here from the raw file once, when the feature is compiled.
//!
//! [`place_in_world`] is `placeInWorld`. In order it picks a palette, runs the processors
//! (`processBlockInfos`), writes the blocks inside the bounding box with their block entities,
//! waterlogs and floods (`LiquidBlockContainer.placeLiquid`), and then runs the neighbour-shape
//! pass ([`super::shape`]). [`PlaceSettings::new`] is what decoration uses (mirror `NONE`, pivot
//! at the origin, the feature's random); structure pieces set the rest of
//! `StructurePlaceSettings` themselves.
//!
//! What the region cannot carry is tallied: block entities the template gives no data
//! (`block_entity:<block>`, the block itself is still placed, as for the desert well) and
//! template entities (`template_entities:<template>`). Under the feature's random stream a
//! template whose blocks carry block-entity NBT is refused before anything is written
//! (`template_block_nbt:<template>`): no decoration template in 26.2 has one, and keeping the
//! refusal keeps decoration output unchanged. Scheduled ticks, post-processing marks and POI
//! updates are not recorded, as for every family.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::str::FromStr;

use oxide_core::mth::get_seed;
use oxide_core::rotation::{rotate_block_state, transform, Direction, Mirror, Rotation};
use oxide_core::{BlockPos, BlockState, LegacyRandom, RandomSource, ResourceLocation};
use oxide_datapack::structure_template::{RawBlockState, RawStructureNbt};
use oxide_datapack::{BlockFact, BlockFacts, TagSet};

use super::processor::{process_block_infos, Processor};
use super::shape::update_shapes;
use crate::decorator::block_entity::with_loot_table;
use crate::decorator::context::{block_in, FeatureContext, UnsupportedTally};
use crate::DecorationRegion;

/// One block of a template: template-relative before processing, world position after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInfo {
    pub pos: BlockPos,
    pub state: BlockState,
    pub nbt: Option<BlockNbt>,
}

/// The block-entity data a [`BlockInfo`] carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockNbt {
    /// The template's own, an index into [`Template::nbt`].
    Template(u32),
    /// What a rule's `append_loot` modifier leaves: a loot table and the seed it drew.
    Loot { table: String, seed: i64 },
}

/// One template block's NBT, read once when the template loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateNbt {
    /// The block-entity id the template saved.
    pub id: String,
    /// Every field but `id`, `x`, `y` and `z`, as SNBT `key:value` pairs joined by commas.
    pub fields: String,
    pub loot_table: Option<String>,
    /// A structure block in `DATA` mode: its `metadata`.
    pub data_marker: Option<String>,
    /// A jigsaw block: the `final_state` it turns into once its piece is placed.
    pub final_state: Option<String>,
}

/// A parsed template. [`Template::default`] is what vanilla's `getOrCreate` hands back for an
/// id with no file: no palettes and a zero size.
#[derive(Debug, Clone, Default)]
pub struct Template {
    size: [i32; 3],
    /// Each palette's blocks in placement order.
    pub(super) palettes: Vec<Vec<BlockInfo>>,
    nbt: Vec<TemplateNbt>,
    entity_count: usize,
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
    pub mirror: Mirror,
    pub pivot: BlockPos,
    pub bounding_box: Option<BoundingBox>,
    pub processors: &'p [Processor],
    /// `settings.getRandom(pos)` with no random set, as for every structure piece: a
    /// `LegacyRandom` seeded by `Mth.getSeed(pos)` for each call. Otherwise the settings carry
    /// the feature's stream, the context's random.
    pub positional_random: bool,
    /// `LiquidSettings.APPLY_WATERLOGGING`.
    pub apply_waterlogging: bool,
    pub ignore_entities: bool,
}

impl<'p> PlaceSettings<'p> {
    /// The settings a decoration feature places with.
    pub fn new(
        rotation: Rotation,
        bounding_box: Option<BoundingBox>,
        processors: &'p [Processor],
    ) -> Self {
        Self {
            rotation,
            mirror: Mirror::None,
            pivot: BlockPos::new(0, 0, 0),
            bounding_box,
            processors,
            positional_random: false,
            apply_waterlogging: true,
            ignore_entities: false,
        }
    }
}

pub(super) fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}

pub(super) fn step(pos: BlockPos, direction: Direction) -> BlockPos {
    offset(
        pos,
        direction.step_x(),
        direction.step_y(),
        direction.step_z(),
    )
}

impl Template {
    /// `StructureTemplate.load` + `loadPalette`: palette states read the way
    /// `NbtUtils.readBlockState` does (an unknown block is air, missing properties take the
    /// block's defaults), an out-of-range palette index is air, then `buildInfoList`'s order.
    pub fn from_raw(raw: &RawStructureNbt, facts: &BlockFacts) -> Self {
        let air = BlockState::new(ResourceLocation::minecraft("air"));
        let mut nbt = Vec::new();
        let nbt_index: Vec<Option<u32>> = raw
            .blocks
            .iter()
            .map(|entry| {
                let fastnbt::Value::Compound(map) = entry.nbt.as_ref()? else {
                    return None;
                };
                nbt.push(TemplateNbt::read(map));
                Some(nbt.len() as u32 - 1)
            })
            .collect();
        let palettes = raw
            .all_palettes()
            .into_iter()
            .map(|palette| {
                let states: Vec<BlockState> = palette
                    .iter()
                    .map(|entry| read_block_state(entry, facts, &air))
                    .collect();
                let (mut full, mut other, mut with_nbt) = (Vec::new(), Vec::new(), Vec::new());
                for (entry, nbt) in raw.blocks.iter().zip(&nbt_index) {
                    let info = BlockInfo {
                        pos: BlockPos::new(entry.pos[0], entry.pos[1], entry.pos[2]),
                        state: states.get(entry.state).unwrap_or(&air).clone(),
                        nbt: nbt.map(BlockNbt::Template),
                    };
                    if info.nbt.is_some() {
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
            nbt,
            entity_count: raw.entities.len(),
        }
    }

    /// The NBT a [`BlockNbt::Template`] points at.
    pub fn nbt(&self, index: u32) -> &TemplateNbt {
        &self.nbt[index as usize]
    }

    /// `settings.getRandomPalette(palettes, position)` — the index of the palette placed at
    /// `position`, or `None` for a template with none. Under the feature's stream this draws
    /// from it even when there is a single palette.
    fn palette_index(
        &self,
        world: &mut TemplateWorld<'_, '_>,
        position: BlockPos,
        settings: &PlaceSettings<'_>,
    ) -> Option<usize> {
        if self.palettes.is_empty() {
            return None;
        }
        let count = self.palettes.len() as i32;
        let index = if settings.positional_random {
            LegacyRandom::new(get_seed(position.x, position.y, position.z)).next_int_bounded(count)
        } else {
            world.random.next_int_bounded(count)
        };
        Some(index as usize)
    }

    /// `filterBlocks(position, settings, block)`: the blocks of `block` in the palette placed at
    /// `position`, at their world positions, unrotated states, inside the settings' bounding box.
    /// Positional palettes only: under the feature's stream the palette choice would draw again.
    pub fn filter_blocks(
        &self,
        position: BlockPos,
        settings: &PlaceSettings<'_>,
        block: &ResourceLocation,
    ) -> Vec<(BlockPos, &BlockInfo)> {
        debug_assert!(settings.positional_random);
        if self.palettes.is_empty() {
            return Vec::new();
        }
        let count = self.palettes.len() as i32;
        let index = LegacyRandom::new(get_seed(position.x, position.y, position.z))
            .next_int_bounded(count) as usize;
        self.palettes[index]
            .iter()
            .filter(|info| info.state.name == *block)
            .filter_map(|info| {
                let moved = transform(info.pos, settings.mirror, settings.rotation, settings.pivot);
                let pos = offset(moved, position.x, position.y, position.z);
                let inside = settings
                    .bounding_box
                    .is_none_or(|bounds| bounds.is_inside(pos));
                inside.then_some((pos, info))
            })
            .collect()
    }

    pub fn raw_size(&self) -> [i32; 3] {
        self.size
    }

    /// `getSize(rotation)`: X and Z swap under a quarter turn.
    pub fn size(&self, rotation: Rotation) -> [i32; 3] {
        match rotation {
            Rotation::Clockwise90 | Rotation::Counterclockwise90 => {
                [self.size[2], self.size[1], self.size[0]]
            }
            Rotation::None | Rotation::Clockwise180 => self.size,
        }
    }

    /// `getBoundingBox(settings, position)` with the pivot at the origin and no mirror.
    pub fn bounding_box(&self, position: BlockPos, rotation: Rotation) -> BoundingBox {
        self.bounding_box_with(position, rotation, Mirror::None, BlockPos::new(0, 0, 0))
    }

    /// `getBoundingBox(position, rotation, pivot, mirror)`.
    pub fn bounding_box_with(
        &self,
        position: BlockPos,
        rotation: Rotation,
        mirror: Mirror,
        pivot: BlockPos,
    ) -> BoundingBox {
        let origin = BlockPos::new(0, 0, 0);
        let far = BlockPos::new(self.size[0] - 1, self.size[1] - 1, self.size[2] - 1);
        let a = transform(origin, mirror, rotation, pivot);
        let b = transform(far, mirror, rotation, pivot);
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
    facts.complete(&BlockState {
        name,
        properties: entry.properties.clone(),
    })
}

/// What placing a template reads and writes: the level, the tags and seed processors test
/// against, and the random `placeInWorld` is handed (decoration's stream, or a piece's).
pub struct TemplateWorld<'w, 'r> {
    pub region: &'w mut DecorationRegion<'r>,
    pub block_tags: &'w TagSet,
    pub facts: &'w BlockFacts,
    pub seed: i64,
    pub random: &'w mut dyn RandomSource,
    /// Where what the region cannot carry is tallied; structure pieces keep no tally.
    pub unsupported: Option<&'w mut UnsupportedTally>,
}

impl<'w, 'r> TemplateWorld<'w, 'r> {
    pub fn from_feature(cx: &'w mut FeatureContext<'_, '_, 'r>) -> Self {
        Self {
            region: &mut *cx.region,
            block_tags: &cx.env.block_tags,
            facts: &cx.env.facts,
            seed: cx.env.seed,
            random: &mut *cx.random,
            unsupported: Some(&mut *cx.unsupported),
        }
    }

    pub(super) fn record_unsupported(&mut self, category: &str, detail: &str) {
        if let Some(tally) = self.unsupported.as_deref_mut() {
            tally.record(category, detail);
        }
    }
}

/// `StructureTemplate.placeInWorld(level, position, position, settings, random, flags)`.
/// `id` names the template for the tallies. Returns what vanilla returns.
pub fn place_in_world(
    template: &Template,
    id: &ResourceLocation,
    world: &mut TemplateWorld<'_, '_>,
    position: BlockPos,
    settings: &PlaceSettings<'_>,
) -> bool {
    if !settings.positional_random && !template.nbt.is_empty() {
        world.record_unsupported("template_block_nbt", &id.to_string());
        return false;
    }
    let Some(index) = template.palette_index(world, position, settings) else {
        return false;
    };
    let blocks = &template.palettes[index];
    let no_entities = settings.ignore_entities || template.entity_count == 0;
    if (blocks.is_empty() && no_entities) || template.size.iter().any(|&side| side < 1) {
        return false;
    }

    let processed = process_block_infos(world, position, settings, blocks);
    let facts = world.facts;
    let mut to_fill: Vec<BlockPos> = Vec::new();
    let mut locked: HashSet<BlockPos> = HashSet::new();
    let mut placed: Vec<BlockPos> = Vec::with_capacity(processed.len());
    for info in processed {
        if settings
            .bounding_box
            .is_some_and(|bounds| !bounds.is_inside(info.pos))
        {
            continue;
        }
        let previous = block_in(world.region, info.pos).clone();
        // PARITY-CHECK: `rotate_block_state` and `mirror_block_state` cover the standard
        // directional properties (see their docs); a template block with a per-block rotation
        // rule (rails' `shape`, a stair's `shape` under a mirror) would come out unturned.
        // Verify by diffing a placed template against the game for any template whose palette
        // has more than `axis`/`facing`-style properties.
        let mirrored = mirror_block_state(&info.state, settings.mirror);
        let state = rotate_block_state(&mirrored, settings.rotation);
        if !world.region.set_block(info.pos, state.clone()) {
            continue;
        }
        placed.push(info.pos);
        if facts.is(&state, BlockFact::BlockEntity) {
            match &info.nbt {
                Some(nbt) => place_block_entity(template, world, info.pos, &state, nbt),
                None => world.record_unsupported("block_entity", &state.name.to_string()),
            }
        }
        if !settings.apply_waterlogging {
            continue;
        }
        if facts.is(&state, BlockFact::FluidSource) {
            locked.insert(info.pos);
        } else if is_liquid_container(&state) {
            place_liquid(world, info.pos, &state, &previous);
            if !facts.is(&previous, BlockFact::FluidSource) {
                to_fill.push(info.pos);
            }
        }
    }

    flood(world, &mut to_fill, &locked);
    if !placed.is_empty() {
        update_shapes(world, &placed);
    }
    if template.entity_count > 0 && !settings.ignore_entities {
        world.record_unsupported("template_entities", &id.to_string());
    }
    true
}

/// The block-entity id of a `RandomizableContainer` block: what `placeInWorld` draws a
/// `LootTableSeed` for, and what `RandomizableContainer.setBlockEntityLootTable` fills in.
pub fn randomizable_container_id(block: &ResourceLocation) -> Option<&'static str> {
    if block.namespace() != "minecraft" {
        return None;
    }
    let path = block.path();
    Some(match path {
        "chest" => "minecraft:chest",
        "trapped_chest" => "minecraft:trapped_chest",
        "barrel" => "minecraft:barrel",
        "dispenser" => "minecraft:dispenser",
        "dropper" => "minecraft:dropper",
        "hopper" => "minecraft:hopper",
        "crafter" => "minecraft:crafter",
        "decorated_pot" => "minecraft:decorated_pot",
        _ if path == "shulker_box" || path.ends_with("_shulker_box") => "minecraft:shulker_box",
        _ => return None,
    })
}

/// The block-entity half of one `placeInWorld` write: the loot seed a container draws from the
/// placement's random, then the data loaded into the block entity, as the SNBT it saves.
///
/// PARITY-CHECK: a block entity is written as the template's fields, not round-tripped through
/// the block entity's own `saveAdditional`, so a field the type always saves but the template
/// left out (a furnace's `cooking_time_spent`, say) is missing from the chunk. A container with
/// a loot table saves exactly `LootTable` and `LootTableSeed`, as vanilla does. Verify by diffing
/// block-entity NBT of a placed igloo laboratory and an end-city ship against the game.
fn place_block_entity(
    template: &Template,
    world: &mut TemplateWorld<'_, '_>,
    pos: BlockPos,
    state: &BlockState,
    nbt: &BlockNbt,
) {
    let container = randomizable_container_id(&state.name);
    let snbt = match nbt {
        BlockNbt::Template(index) => {
            let nbt = template.nbt(*index);
            // `blockInfo.nbt.putLong("LootTableSeed", random.nextLong())`, which only a loot
            // table makes observable.
            let seed = container.map(|_| world.random.next_long());
            match (&nbt.loot_table, seed) {
                (Some(table), Some(seed)) => with_loot_table(&nbt.id, pos, table, seed),
                _ => {
                    let mut snbt =
                        format!("{{id:\"{}\",x:{},y:{},z:{}", nbt.id, pos.x, pos.y, pos.z);
                    if !nbt.fields.is_empty() {
                        snbt.push(',');
                        snbt.push_str(&nbt.fields);
                    }
                    snbt.push('}');
                    snbt
                }
            }
        }
        // Only suspicious sand and gravel take an `append_loot` rule in 26.2; a container would
        // redraw its seed here, as above.
        BlockNbt::Loot { table, seed } => match container {
            Some(id) => with_loot_table(id, pos, table, world.random.next_long()),
            None => with_loot_table("minecraft:brushable_block", pos, table, *seed),
        },
    };
    world.region.put_block_entity(pos, snbt);
}

/// `Block.mirror` in the vocabulary of [`rotate_block_state`]: a `facing` on the mirrored axis
/// turns half round (`Mirror.getRotation`, what directional blocks and chests do), connection
/// flags across that axis swap (`CrossCollisionBlock`, `WallBlock`), and stairs follow
/// `StairBlock.mirror`, including its keeping an inner corner's side under `FRONT_BACK`.
pub fn mirror_block_state(state: &BlockState, mirror: Mirror) -> BlockState {
    let (a, b) = match mirror {
        Mirror::None => return state.clone(),
        Mirror::FrontBack => (Direction::East, Direction::West),
        Mirror::LeftRight => (Direction::North, Direction::South),
    };
    if state.properties.is_empty() {
        return state.clone();
    }
    let mut out = state.clone();
    if let Some(facing) = state
        .properties
        .get("facing")
        .and_then(|f| Direction::from_name(f))
    {
        if facing != a && facing != b {
            return out;
        }
        out = rotate_block_state(&out, Rotation::Clockwise180);
        if state.name.path().ends_with("_stairs") {
            let swap_inner = mirror == Mirror::LeftRight;
            let shape = match state.properties.get("shape").map(String::as_str) {
                Some("inner_left") if swap_inner => "inner_right",
                Some("inner_right") if swap_inner => "inner_left",
                Some("outer_left") => "outer_right",
                Some("outer_right") => "outer_left",
                _ => return out,
            };
            out.properties.insert("shape".into(), shape.into());
        }
        return out;
    }
    if let (Some(first), Some(second)) = (
        state.properties.get(a.name()).cloned(),
        state.properties.get(b.name()).cloned(),
    ) {
        out.properties.insert(a.name().into(), second);
        out.properties.insert(b.name().into(), first);
    }
    out
}

impl TemplateNbt {
    fn read(map: &std::collections::HashMap<String, fastnbt::Value>) -> Self {
        let string = |key: &str| match map.get(key) {
            Some(fastnbt::Value::String(value)) => Some(value.clone()),
            _ => None,
        };
        let mut keys: Vec<&String> = map
            .keys()
            .filter(|key| !matches!(key.as_str(), "id" | "x" | "y" | "z"))
            .collect();
        keys.sort();
        let mut fields = String::new();
        for key in keys {
            if !fields.is_empty() {
                fields.push(',');
            }
            write_snbt_key(&mut fields, key);
            fields.push(':');
            write_snbt(&mut fields, &map[key]);
        }
        let id = string("id").unwrap_or_default();
        let data_marker = (id == "minecraft:structure_block"
            && string("mode").as_deref() == Some("DATA"))
        .then(|| string("metadata").unwrap_or_default());
        Self {
            final_state: (id == "minecraft:jigsaw")
                .then(|| string("final_state").unwrap_or_else(|| "minecraft:air".into())),
            loot_table: string("LootTable"),
            data_marker,
            fields,
            id,
        }
    }
}

fn write_snbt_key(out: &mut String, key: &str) {
    let bare = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'));
    if bare {
        out.push_str(key);
    } else {
        write_snbt_string(out, key);
    }
}

fn write_snbt_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
}

/// `NbtUtils`-style SNBT, compounds with their keys sorted so output is stable.
fn write_snbt(out: &mut String, value: &fastnbt::Value) {
    use fastnbt::Value;
    let list = |out: &mut String, prefix: &str, items: Vec<String>| {
        out.push('[');
        out.push_str(prefix);
        out.push_str(&items.join(","));
        out.push(']');
    };
    match value {
        Value::Byte(v) => _ = write!(out, "{v}b"),
        Value::Short(v) => _ = write!(out, "{v}s"),
        Value::Int(v) => _ = write!(out, "{v}"),
        Value::Long(v) => _ = write!(out, "{v}L"),
        Value::Float(v) => _ = write!(out, "{v:?}f"),
        Value::Double(v) => _ = write!(out, "{v:?}d"),
        Value::String(v) => write_snbt_string(out, v),
        Value::ByteArray(v) => list(out, "B;", v.iter().map(|b| format!("{b}B")).collect()),
        Value::IntArray(v) => list(out, "I;", v.iter().map(|i| i.to_string()).collect()),
        Value::LongArray(v) => list(out, "L;", v.iter().map(|l| format!("{l}L")).collect()),
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_snbt(out, item);
            }
            out.push(']');
        }
        Value::Compound(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_snbt_key(out, key);
                out.push(':');
                write_snbt(out, &map[key]);
            }
            out.push('}');
        }
    }
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
fn place_liquid(
    world: &mut TemplateWorld<'_, '_>,
    pos: BlockPos,
    state: &BlockState,
    fluid: &BlockState,
) -> bool {
    let facts = &world.facts;
    let water_source = facts.is(fluid, BlockFact::Water) && facts.is(fluid, BlockFact::FluidSource);
    if state
        .properties
        .get("waterlogged")
        .is_some_and(|value| value == "false")
        && water_source
    {
        world
            .region
            .set_block(pos, state.clone().with_property("waterlogged", "true"));
        return true;
    }
    false
}

/// The `toFill` loop of `placeInWorld`: each container that did not start in a source takes
/// the first unlocked source next to it (up, north, east, south, west), repeated while a pass
/// fills anything.
fn flood(
    world: &mut TemplateWorld<'_, '_>,
    to_fill: &mut Vec<BlockPos>,
    locked: &HashSet<BlockPos>,
) {
    const AROUND: [Direction; 5] = [
        Direction::Up,
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
    ];
    let mut filled = true;
    while filled && !to_fill.is_empty() {
        filled = false;
        let mut index = 0;
        while index < to_fill.len() {
            let pos = to_fill[index];
            let mut source = block_in(world.region, pos).clone();
            for direction in AROUND {
                if world.facts.is(&source, BlockFact::FluidSource) {
                    break;
                }
                let neighbour = step(pos, direction);
                let state = block_in(world.region, neighbour);
                if world.facts.is(state, BlockFact::FluidSource) && !locked.contains(&neighbour) {
                    source = state.clone();
                }
            }
            if world.facts.is(&source, BlockFact::FluidSource) {
                let state = block_in(world.region, pos).clone();
                if is_liquid_container(&state) {
                    place_liquid(world, pos, &state, &source);
                    filled = true;
                    to_fill.remove(index);
                    continue;
                }
            }
            index += 1;
        }
    }
}
