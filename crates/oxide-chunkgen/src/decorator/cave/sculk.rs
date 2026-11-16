//! `sculk_patch`: the sculk spreader run once at generation time, the deep dark's and ancient
//! city's floor of sculk, veins, sensors and shriekers.
//!
//! Vanilla's `SculkSpreader` keeps a set of charge cursors. Each round the patch seeds
//! `charge_count` cursors of `amount_per_charge` at the origin and updates them `spread_attempts`
//! times, then forgets them. A cursor walks over sculk and veins; on a vein it converts the
//! support block to sculk, on sculk it decays, and on sculk with room above it spends charge to
//! grow a sensor or shrieker. The first `spread_rounds` rounds also let veins spread.
//!
//! Vanilla marks every written block for post-processing and schedules block ticks. The region
//! keeps block writes only, so neither is reproduced; the written states are the same.
//!
//! The reference implementation ends after the rounds. The catalyst and the extra rare shriekers
//! (`catalyst_chance`, `extra_rare_growths`, which every 26.2 sculk patch configures) follow
//! the bytecode of the real 26.2 `SculkPatchFeature.place`: `nextFloat() <= catalyst_chance`
//! and a full collision block below place the catalyst, then each extra growth draws two
//! `nextInt(5) - 2` offsets and needs air over a sturdy top face.
//!
//! PARITY-CHECK: a sensor or shrieker may not grow within 4x3x4 blocks of three others. The
//! reference reads that set from a `minecraft:sculk_growth_inhibitors` block tag, which the 26.2
//! export does not have. The tag is used when a pack provides it; otherwise the set is
//! `sculk_sensor` and `sculk_shrieker`. The three sculk blockers (`sculk`, `sculk_catalyst`,
//! `moving_piston`) are likewise named blocks, as in `multiface_growth`. Confirm both against 26.2.
//!
//! Every read goes through [`Info`]: what the spreader asks of a state (its name tests, tags,
//! vein faces and facts) worked out once per [`StateKey`] per run. A vein the spreader writes is
//! named by its faces and waterlogging and written by a pre-interned id, never built as a
//! `BlockState` per write.

use std::cell::Cell;
use std::collections::HashSet;
use std::ops::Deref;

use anyhow::{bail, Result};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::cave_feature::SculkPatchConfig;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::{BlockFact, BlockFacts};

use super::support::{
    dist_manhattan, face_full, has, is_block, offset, relative, shuffle, state_has, waterlogged,
    Dir,
};
use crate::decorator::context::{block_in, FeatureContext, VOID_AIR};
use crate::decorator::provider::sample_int;
use crate::decorator::state_provider::state_from_spec;
use crate::region::StateKey;

const GROWTH_SPAWN_COST: i32 = 50;
const NO_GROWTH_RADIUS: i32 = 1;
const CHARGE_DECAY_RATE: i32 = 5;
const ADDITIONAL_DECAY_RATE: i32 = 10;
const MAX_CURSORS: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    Default,
    Sculk,
    Vein,
}

#[derive(Clone, Copy)]
enum Spread {
    SamePosition,
    SamePlane,
    WrapAround,
}

const ALL_SPREADS: [Spread; 3] = [Spread::SamePosition, Spread::SamePlane, Spread::WrapAround];

/// `SculkSpreader.ChargeCursor`.
struct Cursor {
    pos: BlockPos,
    charge: i32,
    update_delay: i32,
    decay_delay: i32,
    /// The faces of the vein or sculk the cursor last moved onto, as a face mask.
    facings: Option<u8>,
}

/// `ChargeCursor.NON_CORNER_NEIGHBOURS` in `BlockPos.betweenClosed` order: x fastest, then y,
/// then z.
const NON_CORNER_NEIGHBOURS: [(i32, i32, i32); 18] = {
    let mut offsets = [(0, 0, 0); 18];
    let mut n = 0;
    let mut z = -1;
    while z <= 1 {
        let mut y = -1;
        while y <= 1 {
            let mut x = -1;
            while x <= 1 {
                if (x == 0 || y == 0 || z == 0) && !(x == 0 && y == 0 && z == 0) {
                    offsets[n] = (x, y, z);
                    n += 1;
                }
                x += 1;
            }
            y += 1;
        }
        z += 1;
    }
    assert!(n == 18);
    offsets
};

/// The properties a sculk vein has: its six faces and `waterlogged`.
const VEIN_KEYS: [&str; 7] = [
    "down",
    "up",
    "north",
    "south",
    "west",
    "east",
    "waterlogged",
];

#[derive(Debug)]
pub struct SculkPatchFeature {
    charge_count: i32,
    amount_per_charge: i32,
    spread_attempts: i32,
    growth_rounds: i32,
    spread_rounds: i32,
    extra_rare_growths: IntProvider,
    catalyst_chance: f32,
    sculk: BlockState,
    /// The default vein: every key of [`VEIN_KEYS`] and nothing else, all `false`.
    vein: BlockState,
    catalyst: BlockState,
    /// `shrieker` and `sensor`, then each waterlogged when it has the property.
    growths: [(BlockState, Option<BlockState>); 2],
    water: BlockState,
    air: BlockState,
}

fn default_of(name: &str, facts: &BlockFacts) -> Result<BlockState> {
    state_from_spec(
        &oxide_datapack::placement::BlockStateSpec {
            name: format!("minecraft:{name}"),
            properties: Default::default(),
        },
        facts,
    )
}

impl SculkPatchFeature {
    /// `None` for a count outside the codec's ranges, which vanilla would refuse to load.
    pub fn compile(config: &SculkPatchConfig, facts: &BlockFacts) -> Result<Option<Self>> {
        let in_range = (1..=32).contains(&config.charge_count)
            && (1..=500).contains(&config.amount_per_charge)
            && (1..=64).contains(&config.spread_attempts)
            && (0..=8).contains(&config.growth_rounds)
            && (0..=8).contains(&config.spread_rounds)
            && (0.0..=1.0).contains(&config.catalyst_chance);
        if !in_range {
            return Ok(None);
        }
        let vein = default_of("sculk_vein", facts)?;
        // Writes name a vein by its faces and waterlogging alone, which holds when the default
        // vein has exactly those properties, all off.
        if vein.properties.len() != VEIN_KEYS.len()
            || !VEIN_KEYS
                .iter()
                .all(|key| vein.properties.get(*key).is_some_and(|v| v == "false"))
        {
            bail!("unexpected default sculk vein {vein}");
        }
        let growth = |state: BlockState| {
            let wet = state
                .properties
                .contains_key("waterlogged")
                .then(|| waterlogged(state.clone(), true));
            (state, wet)
        };
        Ok(Some(Self {
            charge_count: config.charge_count,
            amount_per_charge: config.amount_per_charge,
            spread_attempts: config.spread_attempts,
            growth_rounds: config.growth_rounds,
            spread_rounds: config.spread_rounds,
            extra_rare_growths: config.extra_rare_growths.clone(),
            catalyst_chance: config.catalyst_chance,
            sculk: default_of("sculk", facts)?,
            vein,
            catalyst: default_of("sculk_catalyst", facts)?,
            growths: [
                growth(default_of("sculk_shrieker", facts)?.with_property("can_summon", "true")),
                growth(default_of("sculk_sensor", facts)?),
            ],
            water: default_of("water", facts)?,
            air: default_of("air", facts)?,
        }))
    }

    /// `SculkPatchFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !self.can_spread_from(cx, origin) {
            return false;
        }
        let tags = &cx.env.block_tags;
        let tag = |path: &str| tags.get(&ResourceLocation::minecraft(path));
        let mut spreader = Spreader {
            feature: self,
            fire: tag("fire"),
            replaceable: tag("sculk_replaceable"),
            replaceable_world_gen: tag("sculk_replaceable_world_gen"),
            inhibitors: tag("sculk_growth_inhibitors"),
            infos: (0..INFO_SLOTS).map(|_| Cell::new(None)).collect(),
            outside: Info::default(),
            ids: Ids::default(),
        };
        let void = spreader.describe(cx.region.facts(), &VOID_AIR);
        spreader.outside = Info {
            names: void.names | I_OUTSIDE,
            is: 0,
            fact_in: void.fact_in,
        };
        spreader.place(cx)
    }

    fn behaviour(&self, state: &BlockState) -> Behaviour {
        if state.name == self.sculk.name {
            Behaviour::Sculk
        } else if state.name == self.vein.name {
            Behaviour::Vein
        } else {
            Behaviour::Default
        }
    }

    /// `SculkPatchFeature.canSpreadFrom`.
    fn can_spread_from(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos) -> bool {
        let start = block_in(cx.region, origin);
        if self.behaviour(start) != Behaviour::Default {
            return true;
        }
        let water_source_block =
            is_block(start, "water") && state_has(cx, start, BlockFact::FluidSource);
        if !state_has(cx, start, BlockFact::Air) && !water_source_block {
            return false;
        }
        Dir::ALL
            .into_iter()
            .any(|direction| has(cx, relative(origin, direction, 1), BlockFact::FullCollision))
    }
}

// What the spreader knows of a state, as `Info::names` keeps it.
/// Set on every `Info` read from the region; a `Near` slot without it is unread.
const I_READ: u32 = 1 << 0;
/// Outside the region's window, where the block is `void_air`.
const I_OUTSIDE: u32 = 1 << 1;
const I_SCULK: u32 = 1 << 2;
const I_VEIN: u32 = 1 << 3;
/// A vein with exactly the default vein's properties, each `true` or `false`: its faces and
/// waterlogging name it.
const I_CANONICAL: u32 = 1 << 4;
/// `sculk`, `sculk_catalyst` or `moving_piston`: what a vein may not spread against.
const I_BLOCKER: u32 = 1 << 5;
/// `minecraft:water`, whatever its properties.
const I_WATER: u32 = 1 << 6;
const I_FIRE: u32 = 1 << 7;
const I_REPLACEABLE: u32 = 1 << 8;
const I_REPLACEABLE_WORLD_GEN: u32 = 1 << 9;
const I_INHIBITOR: u32 = 1 << 10;
const I_WATERLOGGED: u32 = 1 << 11;
/// The six faces set (`<face>=true`), bit `i` for `Dir::ALL[i]`.
const I_FACE_SHIFT: u32 = 12;

/// Direct-mapped `Info` slots per run.
const INFO_SLOTS: usize = 256;

/// The `Info` slot of `key`. `StateKey::slot` keeps a terrain key's section and palette entry
/// in separate bits, but its low bits overlap them (a section's later entries share slots with
/// the next section's first ones); a multiplicative mix spreads them over the slots.
fn info_slot(key: StateKey) -> usize {
    ((key.slot() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as usize & (INFO_SLOTS - 1)
}

/// One block as the spreader reads it.
#[derive(Clone, Copy, Default)]
struct Info {
    names: u32,
    /// `BlockFact` bits as `region.is` answers them: none outside the window.
    is: u32,
    /// `BlockFact` bits as `fact_in` answers them: `void_air`'s outside the window.
    fact_in: u32,
}

impl Info {
    fn has(self, bit: u32) -> bool {
        self.names & bit != 0
    }

    fn faces(self) -> u8 {
        (self.names >> I_FACE_SHIFT) as u8 & 0x3f
    }

    fn has_face(self, face: Dir) -> bool {
        self.faces() & face_bit(face) != 0
    }

    fn is(self, fact: BlockFact) -> bool {
        self.is & fact as u32 != 0
    }

    fn fact_in(self, fact: BlockFact) -> bool {
        self.fact_in & fact as u32 != 0
    }

    fn any_fluid(self) -> bool {
        self.fact_in(BlockFact::Water) || self.fact_in(BlockFact::Lava)
    }

    fn behaviour(self) -> Behaviour {
        if self.has(I_SCULK) {
            Behaviour::Sculk
        } else if self.has(I_VEIN) {
            Behaviour::Vein
        } else {
            Behaviour::Default
        }
    }
}

/// A block the cursor read and may write back changed later: a vein that is not canonical
/// keeps its state, which the writes then derive from as vanilla does.
struct Read {
    info: Info,
    state: Option<BlockState>,
}

/// What a spread writes: a pre-interned id, or (for a vein that is not canonical) a state.
enum Placement {
    Id(u32),
    State(BlockState),
}

const NO_ID: u32 = u32::MAX;

/// Region ids of the states the spreader writes, interned on first use.
struct Ids {
    sculk: Cell<u32>,
    water: Cell<u32>,
    air: Cell<u32>,
    /// Per growth (as `SculkPatchFeature::growths`), dry then waterlogged.
    growths: [[Cell<u32>; 2]; 2],
    /// Per vein: face mask, plus 64 when waterlogged.
    veins: [Cell<u32>; 128],
}

impl Default for Ids {
    fn default() -> Self {
        let none = || Cell::new(NO_ID);
        Self {
            sculk: none(),
            water: none(),
            air: none(),
            growths: std::array::from_fn(|_| std::array::from_fn(|_| none())),
            veins: std::array::from_fn(|_| none()),
        }
    }
}

fn face_index(face: Dir) -> usize {
    match face {
        Dir::Down => 0,
        Dir::Up => 1,
        Dir::North => 2,
        Dir::South => 3,
        Dir::West => 4,
        Dir::East => 5,
    }
}

fn face_bit(face: Dir) -> u8 {
    1 << face_index(face)
}

/// One `place` call's view of the feature: the block tags it tests, resolved once rather than
/// looked up by a freshly built id on every test, and what it has read of each state.
struct Spreader<'s, 'e> {
    feature: &'s SculkPatchFeature,
    fire: Option<&'e HashSet<ResourceLocation>>,
    replaceable: Option<&'e HashSet<ResourceLocation>>,
    replaceable_world_gen: Option<&'e HashSet<ResourceLocation>>,
    /// `None` when the pack has no `sculk_growth_inhibitors` tag (see the module doc).
    inhibitors: Option<&'e HashSet<ResourceLocation>>,
    /// Per `StateKey::slot`, the last state described there.
    infos: Box<[Cell<Option<(StateKey, Info)>>]>,
    /// The `Info` outside the window.
    outside: Info,
    ids: Ids,
}

impl Deref for Spreader<'_, '_> {
    type Target = SculkPatchFeature;

    fn deref(&self) -> &SculkPatchFeature {
        self.feature
    }
}

/// A block of the 3x3x3 around the source of one spread: `(z + 1) * 9 + (y + 1) * 3 + (x + 1)`
/// for its offset `(x, y, z)`, each coordinate in `-1..=1`.
type Local = usize;

const SOURCE: Local = 13;

/// Per face index (as `Dir::ALL`), the change in [`Local`] one step that way.
const STEP: [isize; 6] = [-3, 3, -9, 9, -1, 1];

/// The face index of the opposite face: `Dir::ALL` lists each face next to its opposite.
const fn opposite(face: usize) -> usize {
    face ^ 1
}

fn step(at: Local, face: usize) -> Local {
    at.wrapping_add_signed(STEP[face])
}

/// The offset `(x, y, z)` of a [`Local`].
const fn coords(at: Local) -> [i32; 3] {
    let at = at as i32;
    [at % 3 - 1, at / 3 % 3 - 1, at / 9 - 1]
}

/// Per [`Local`], its Manhattan distance from the source.
const MANHATTAN: [u8; 27] = {
    let mut table = [0; 27];
    let mut at = 0;
    while at < 27 {
        let [x, y, z] = coords(at);
        table[at] = (x.abs() + y.abs() + z.abs()) as u8;
        at += 1;
    }
    table
};

/// Every `(face, direction)` pair `spread_all` walks, faces then directions in `Dir::ALL`
/// order, each pair on two different axes.
const PAIRS: [(usize, usize); 24] = {
    let mut pairs = [(0, 0); 24];
    let mut n = 0;
    let mut face = 0;
    while face < 6 {
        let mut direction = 0;
        while direction < 6 {
            if direction >> 1 != face >> 1 {
                pairs[n] = (face, direction);
                n += 1;
            }
            direction += 1;
        }
        face += 1;
    }
    pairs
};

/// The 3x3x3 blocks around the source of one `spread_all`, read from the region as first
/// asked. Every block a spread looks at -- its targets, what they spread against, the faces they
/// attach to -- is the source, a face neighbour or an edge neighbour, and the spread itself is
/// the only writer while it runs; it forgets the block it writes.
struct Near {
    source: BlockPos,
    slots: [Info; 27],
    /// Bumped on every write, so an answer worked out before it is known to be stale.
    writes: u32,
    /// Bumped on every unknown-shape record, so an answer that recorded one is never reused.
    records: u32,
}

impl Near {
    fn new(source: BlockPos) -> Self {
        Self {
            source,
            slots: [Info::default(); 27],
            writes: 0,
            records: 0,
        }
    }

    fn pos(&self, at: Local) -> BlockPos {
        let [x, y, z] = coords(at);
        offset(self.source, x, y, z)
    }

    fn forget(&mut self, at: Local) {
        self.slots[at] = Info::default();
        self.writes += 1;
    }

    fn get(
        &mut self,
        spreader: &Spreader<'_, '_>,
        cx: &FeatureContext<'_, '_, '_>,
        at: Local,
    ) -> Info {
        if !self.slots[at].has(I_READ) {
            self.slots[at] = spreader.info(cx, self.pos(at));
        }
        self.slots[at]
    }
}

impl Spreader<'_, '_> {
    fn member(tag: Option<&HashSet<ResourceLocation>>, state: &BlockState) -> bool {
        tag.is_some_and(|members| members.contains(&state.name))
    }

    /// Whether `state` counts toward the cap on nearby sensors and shriekers.
    fn is_inhibitor(&self, state: &BlockState) -> bool {
        match self.inhibitors {
            Some(members) => members.contains(&state.name),
            None => state.name == self.growths[1].0.name || is_block(state, "sculk_shrieker"),
        }
    }

    /// Everything the spreader asks of `state`, worked out from scratch.
    fn describe(&self, facts: &BlockFacts, state: &BlockState) -> Info {
        let mut names = I_READ;
        if state.name == self.sculk.name {
            names |= I_SCULK;
        }
        if state.name == self.vein.name {
            names |= I_VEIN;
            let canonical = state.properties.len() == VEIN_KEYS.len()
                && VEIN_KEYS.iter().all(|key| {
                    state
                        .properties
                        .get(*key)
                        .is_some_and(|v| v == "true" || v == "false")
                });
            if canonical {
                names |= I_CANONICAL;
            }
        }
        if ["sculk", "sculk_catalyst", "moving_piston"]
            .iter()
            .any(|blocker| is_block(state, blocker))
        {
            names |= I_BLOCKER;
        }
        if is_block(state, "water") {
            names |= I_WATER;
        }
        for (tag, bit) in [
            (self.fire, I_FIRE),
            (self.replaceable, I_REPLACEABLE),
            (self.replaceable_world_gen, I_REPLACEABLE_WORLD_GEN),
        ] {
            if Self::member(tag, state) {
                names |= bit;
            }
        }
        if self.is_inhibitor(state) {
            names |= I_INHIBITOR;
        }
        let set = |key: &str| state.properties.get(key).is_some_and(|v| v == "true");
        if set("waterlogged") {
            names |= I_WATERLOGGED;
        }
        for (i, face) in Dir::ALL.into_iter().enumerate() {
            if set(face.name()) {
                names |= 1 << (I_FACE_SHIFT + i as u32);
            }
        }
        let bits = facts.bits(state);
        Info {
            names,
            is: bits,
            fact_in: bits,
        }
    }

    /// The block at `pos`, described once per state key.
    fn info(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> Info {
        let Some((key, state)) = cx.region.block_keyed(pos) else {
            return self.outside;
        };
        let slot = &self.infos[info_slot(key)];
        if let Some((known, info)) = slot.get() {
            if known == key {
                return info;
            }
        }
        let info = self.describe(cx.region.facts(), state);
        slot.set(Some((key, info)));
        info
    }

    /// [`Self::info`], keeping the state of a vein that is not canonical.
    fn read(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> Read {
        let info = self.info(cx, pos);
        let state =
            (info.has(I_VEIN) && !info.has(I_CANONICAL)).then(|| block_in(cx.region, pos).clone());
        Read { info, state }
    }

    fn id(cx: &mut FeatureContext<'_, '_, '_>, slot: &Cell<u32>, state: &BlockState) -> u32 {
        let id = slot.get();
        if id != NO_ID {
            return id;
        }
        let id = cx.region.intern(state);
        slot.set(id);
        id
    }

    /// The default vein with the faces in `faces` set, waterlogged if `wet`.
    fn vein_id(&self, cx: &mut FeatureContext<'_, '_, '_>, faces: u8, wet: bool) -> u32 {
        let slot = &self.ids.veins[usize::from(faces) | usize::from(wet) << 6];
        if slot.get() != NO_ID {
            return slot.get();
        }
        let mut state = self.vein.clone();
        for face in Dir::ALL {
            if faces & face_bit(face) != 0 {
                state = Self::with_face(state, face, true);
            }
        }
        if wet {
            state = waterlogged(state, true);
        }
        Self::id(cx, slot, &state)
    }

    fn write_id(cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, id: u32) -> bool {
        cx.region.set_block_id(pos, id)
    }

    fn has_face(state: &BlockState, face: Dir) -> bool {
        state
            .properties
            .get(face.name())
            .is_some_and(|value| value == "true")
    }

    fn with_face(state: BlockState, face: Dir, value: bool) -> BlockState {
        state.with_property(face.name(), value.to_string())
    }

    /// `SculkPatchFeature.place`.
    fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let mut cursors: Vec<Cursor> = Vec::new();
        for round in 0..self.spread_rounds + self.growth_rounds {
            for _ in 0..self.charge_count {
                let mut charge = self.amount_per_charge;
                while charge > 0 {
                    let current = charge.min(1000);
                    if cursors.len() < MAX_CURSORS {
                        cursors.push(Cursor {
                            pos: origin,
                            charge: current,
                            update_delay: 0,
                            decay_delay: 1,
                            facings: None,
                        });
                    }
                    charge -= current;
                }
            }
            let spread_veins = round < self.spread_rounds;
            for _ in 0..self.spread_attempts {
                cursors = self.update_cursors(cx, origin, cursors, spread_veins);
            }
            cursors.clear();
        }
        self.place_catalyst_and_shriekers(cx, origin);
        true
    }

    /// The tail of `SculkPatchFeature.place`; checked against the 26.2 bytecode (see the module doc).
    fn place_catalyst_and_shriekers(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos) {
        let below = offset(origin, 0, -1, 0);
        if cx.random.next_float() <= self.catalyst_chance
            && has(cx, below, BlockFact::FullCollision)
        {
            cx.region.set_block_ref(origin, &self.catalyst);
        }
        let extra = sample_int(&self.extra_rare_growths, cx.random);
        for _ in 0..extra {
            let dx = cx.random.next_int_bounded(5) - 2;
            let dz = cx.random.next_int_bounded(5) - 2;
            let pos = offset(origin, dx, 0, dz);
            let below = offset(pos, 0, -1, 0);
            if has(cx, pos, BlockFact::Air) && face_full(cx, below, Dir::Up, false) {
                cx.region.set_block_ref(pos, &self.growths[0].0);
            }
        }
    }

    /// `SculkSpreader.updateCursors`.
    fn update_cursors(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        origin: BlockPos,
        cursors: Vec<Cursor>,
        spread_veins: bool,
    ) -> Vec<Cursor> {
        let mut cursors = cursors;
        // World generation never merges cursors, so vanilla's map of mergeable cursors per
        // position is never read: the cursors that keep charge go on in their order.
        cursors.retain_mut(|cursor| {
            let distance = [
                cursor.pos.x.wrapping_sub(origin.x),
                cursor.pos.y.wrapping_sub(origin.y),
                cursor.pos.z.wrapping_sub(origin.z),
            ]
            .into_iter()
            .map(i32::wrapping_abs)
            .max()
            .unwrap_or(0);
            if distance > 1024 {
                return false;
            }
            self.update(cx, origin, cursor, spread_veins);
            cursor.charge > 0
        });
        cursors
    }

    /// `ChargeCursor.update`.
    fn update(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        origin: BlockPos,
        cursor: &mut Cursor,
        spread_veins: bool,
    ) {
        if cursor.charge <= 0 {
            return;
        }
        if cursor.update_delay > 0 {
            cursor.update_delay -= 1;
            return;
        }
        let mut here = self.read(cx, cursor.pos);
        let mut behaviour = here.info.behaviour();
        if spread_veins
            && self.attempt_spread_vein(cx, behaviour, cursor.pos, here.info, cursor.facings)
            && behaviour != Behaviour::Sculk
        {
            here = self.read(cx, cursor.pos);
            behaviour = here.info.behaviour();
        }
        cursor.charge = self.attempt_use_charge(cx, origin, behaviour, cursor, spread_veins);
        if cursor.charge <= 0 {
            self.on_discharged(cx, behaviour, &here, cursor.pos);
            return;
        }
        match self.valid_movement(cx, cursor.pos, origin) {
            Some(target) => {
                self.on_discharged(cx, behaviour, &here, cursor.pos);
                cursor.pos = target;
            }
            None => {
                self.on_discharged(cx, behaviour, &here, cursor.pos);
                cursor.charge = 0;
                return;
            }
        }
        let arrived = self.info(cx, cursor.pos);
        if arrived.behaviour() != Behaviour::Default {
            cursor.facings = Some(arrived.faces());
        }
        cursor.decay_delay = match behaviour {
            Behaviour::Default => (cursor.decay_delay - 1).max(0),
            _ => 1,
        };
        cursor.update_delay = 1;
    }

    /// `SculkBehaviour.attemptSpreadVein`.
    fn attempt_spread_vein(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        behaviour: Behaviour,
        pos: BlockPos,
        here: Info,
        facings: Option<u8>,
    ) -> bool {
        let source = here.has(I_VEIN).then(|| here.faces());
        // `here` is the block at `pos`: nothing has written since the cursor read it.
        if behaviour == Behaviour::Default {
            match facings {
                None => {
                    return self.spread_all(cx, source, pos, &[Spread::SamePosition]) > 0;
                }
                Some(faces) if faces != 0 => {
                    let water_source_here =
                        here.fact_in(BlockFact::Water) && here.fact_in(BlockFact::FluidSource);
                    if !here.fact_in(BlockFact::Air) && !water_source_here {
                        return false;
                    }
                    return self.regrow(cx, pos, here, faces);
                }
                Some(_) => {}
            }
        }
        self.spread_all(cx, source, pos, &ALL_SPREADS) > 0
    }

    /// `SculkVeinBlock.regrow`.
    fn regrow(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        here: Info,
        faces: u8,
    ) -> bool {
        let mut attached = 0;
        for face in Dir::ALL {
            if faces & face_bit(face) != 0 && self.can_attach_to(cx, pos, face) {
                attached |= face_bit(face);
            }
        }
        if attached == 0 {
            return false;
        }
        // The block at `pos` is still the one the cursor read.
        let id = self.vein_id(cx, attached, here.any_fluid());
        Self::write_id(cx, pos, id);
        true
    }

    /// `support::face_full` for `info` where its shape is known: `None` for a side face of a
    /// block neither empty nor a full cube, which `face_full` records as unsupported and fails.
    fn face_full_known(info: Info, face: Dir, collision: bool) -> Option<bool> {
        match face {
            Dir::Up | Dir::Down => {
                let sturdy = if face == Dir::Up {
                    BlockFact::SturdyTop
                } else {
                    BlockFact::SturdyBottom
                };
                Some(info.is(sturdy) || (collision && info.is(BlockFact::FullCollision)))
            }
            _ => {
                if info.is(BlockFact::Air) || info.is(BlockFact::NoCollision) {
                    return Some(false);
                }
                if info.is(BlockFact::FullCollision) {
                    return Some(collision || info.is(BlockFact::SturdyTop));
                }
                None
            }
        }
    }

    /// The unsupported-shape record of `face_full` for the block at `pos`.
    fn record_shape(cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos) {
        let name = block_in(cx.region, pos).name.to_string();
        cx.unsupported.record("shape", &name);
    }

    /// `support::face_full` for `info`, the block at `pos`.
    fn face_full_of(
        cx: &mut FeatureContext<'_, '_, '_>,
        info: Info,
        pos: BlockPos,
        face: Dir,
        collision: bool,
    ) -> bool {
        Self::face_full_known(info, face, collision).unwrap_or_else(|| {
            Self::record_shape(cx, pos);
            false
        })
    }

    /// `support::face_full` for the block at `pos`.
    fn face_full(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        face: Dir,
        collision: bool,
    ) -> bool {
        let info = self.info(cx, pos);
        Self::face_full_of(cx, info, pos, face, collision)
    }

    /// `MultifaceBlock.canAttachTo` for the block at `pos` looking at `direction`.
    fn can_attach_to(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        direction: Dir,
    ) -> bool {
        self.face_full(cx, relative(pos, direction, 1), direction.opposite(), true)
    }

    /// `MultifaceSpreader.spreadAll` with the sculk vein spreader's configuration, from a vein
    /// with faces `source`, or from another block when `None`. The blocks around it are read
    /// through one [`Near`].
    ///
    /// Every test reads blocks only through `near`, and only this spread writes while it runs,
    /// so a failed same-position test stays failed until the next write: it is looked up rather
    /// than worked out again, unless working it out recorded an unknown shape, which vanilla
    /// would record again.
    fn spread_all(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        source: Option<u8>,
        pos: BlockPos,
        types: &[Spread],
    ) -> i64 {
        let other_source = source.is_none();
        let faces = source.unwrap_or(0);
        let mut near = Near::new(pos);
        // Per direction, `near.writes + 1` when the same-position spread that way failed at
        // that count of writes, with no record.
        let mut same_position_failed = [0u32; 6];
        let mut count = 0;
        for (face, direction) in PAIRS {
            let on_face = faces & 1 << face != 0;
            if !(other_source || (on_face && faces & 1 << direction == 0)) {
                continue;
            }
            let mut target = None;
            for kind in types {
                let candidate = match kind {
                    Spread::SamePosition => {
                        if same_position_failed[direction] == near.writes + 1 {
                            continue;
                        }
                        let records = near.records;
                        if self.can_spread_into(cx, &mut near, SOURCE, direction) {
                            target = Some((SOURCE, direction));
                            break;
                        }
                        if near.records == records {
                            same_position_failed[direction] = near.writes + 1;
                        }
                        continue;
                    }
                    Spread::SamePlane => (step(SOURCE, direction), face),
                    Spread::WrapAround => (step(step(SOURCE, direction), face), opposite(direction)),
                };
                if self.can_spread_into(cx, &mut near, candidate.0, candidate.1) {
                    target = Some(candidate);
                    break;
                }
            }
            let Some((target, face_to)) = target else {
                continue;
            };
            let new = self.state_for_placement(cx, &mut near, target, Dir::ALL[face_to]);
            let at = near.pos(target);
            let written = match new {
                Placement::Id(id) => Self::write_id(cx, at, id),
                Placement::State(state) => cx.region.set_block(at, state),
            };
            near.forget(target);
            if written {
                count += 1;
            }
        }
        count
    }

    /// `support::face_full` for the block at `at`, read through `near`.
    fn near_face_full(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        near: &mut Near,
        at: Local,
        face: usize,
        collision: bool,
    ) -> bool {
        let info = near.get(self, cx, at);
        Self::face_full_known(info, Dir::ALL[face], collision).unwrap_or_else(|| {
            near.records += 1;
            Self::record_shape(cx, near.pos(at));
            false
        })
    }

    /// `SculkVeinBlock.getStateForPlacement` for the block now at `at`, which `can_spread_into`
    /// has just accepted with `face`. Its own tests -- no vein face there already, and a face to
    /// attach to -- are two of the ones `can_spread_into` passed on the same blocks, with nothing
    /// written since, so they pass again (and an attach test that passes records nothing).
    fn state_for_placement(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        near: &mut Near,
        at: Local,
        face: Dir,
    ) -> Placement {
        let existing = near.get(self, cx, at);
        let vein = existing.has(I_VEIN);
        debug_assert!(!(vein && existing.has_face(face)));
        if vein && !existing.has(I_CANONICAL) {
            let state = block_in(cx.region, near.pos(at)).clone();
            Placement::State(Self::with_face(state, face, true))
        } else if vein {
            Placement::Id(self.vein_id(
                cx,
                existing.faces() | face_bit(face),
                existing.has(I_WATERLOGGED),
            ))
        } else {
            let wet =
                existing.fact_in(BlockFact::Water) && existing.fact_in(BlockFact::FluidSource);
            Placement::Id(self.vein_id(cx, face_bit(face), wet))
        }
    }

    /// `MultifaceSpreader.canSpreadInto` with the sculk vein spreader's rules, from the source
    /// of `near` into the block at `target`, on its face `face` (a `Dir::ALL` index).
    fn can_spread_into(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        near: &mut Near,
        target: Local,
        face: usize,
    ) -> bool {
        let behind = step(target, face);
        if near.get(self, cx, behind).has(I_BLOCKER) {
            return false;
        }
        // `distManhattan(source, target)`.
        if MANHATTAN[target] == 2 {
            let neighbor = step(SOURCE, opposite(face));
            if self.near_face_full(cx, near, neighbor, face, false) {
                return false;
            }
        }
        let existing = near.get(self, cx, target);
        let water = existing.fact_in(BlockFact::Water);
        let any_fluid_here = water || existing.fact_in(BlockFact::Lava);
        let water_source_here = water && existing.fact_in(BlockFact::FluidSource);
        if any_fluid_here && !water_source_here {
            return false;
        }
        if existing.has(I_FIRE) {
            return false;
        }
        let vein = existing.has(I_VEIN);
        let replaceable = existing.fact_in(BlockFact::Replaceable)
            || existing.fact_in(BlockFact::Air)
            || vein
            || (existing.has(I_WATER) && water_source_here);
        // `MultifaceBlock.canAttachTo(target, face)`: the face of the block behind it.
        replaceable
            && !(vein && existing.faces() & 1 << face != 0)
            && self.near_face_full(cx, near, behind, opposite(face), true)
    }

    /// `SculkBehaviour.attemptUseCharge` for the three behaviours.
    fn attempt_use_charge(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        origin: BlockPos,
        behaviour: Behaviour,
        cursor: &Cursor,
        spread_veins: bool,
    ) -> i32 {
        let charge = cursor.charge;
        match behaviour {
            Behaviour::Default => {
                if cursor.decay_delay > 0 {
                    charge
                } else {
                    0
                }
            }
            Behaviour::Vein => {
                if spread_veins && self.place_sculk(cx, cursor.pos) {
                    charge - 1
                } else if cx.random.next_int_bounded(CHARGE_DECAY_RATE) == 0 {
                    (charge as f32 * 0.5f32).floor() as i32
                } else {
                    charge
                }
            }
            Behaviour::Sculk => {
                if charge == 0 || cx.random.next_int_bounded(CHARGE_DECAY_RATE) != 0 {
                    return charge;
                }
                let pos = cursor.pos;
                let d = |from: i32, to: i32| {
                    let delta = f64::from(from) - f64::from(to);
                    delta * delta
                };
                let dist_sqr = d(pos.x, origin.x) + d(pos.y, origin.y) + d(pos.z, origin.z);
                let close = dist_sqr < f64::from(NO_GROWTH_RADIUS * NO_GROWTH_RADIUS);
                if !close && self.can_place_growth(cx, pos) {
                    if cx.random.next_int_bounded(GROWTH_SPAWN_COST) < charge {
                        let above = offset(pos, 0, 1, 0);
                        let which = if cx.random.next_int_bounded(11) == 0 {
                            0
                        } else {
                            1
                        };
                        let (dry, wet) = &self.growths[which];
                        let slots = &self.ids.growths[which];
                        let id = match wet {
                            Some(wet) if self.info(cx, above).any_fluid() => {
                                Self::id(cx, &slots[1], wet)
                            }
                            _ => Self::id(cx, &slots[0], dry),
                        };
                        Self::write_id(cx, above, id);
                    }
                    return 0.max(charge - GROWTH_SPAWN_COST);
                }
                if cx.random.next_int_bounded(ADDITIONAL_DECAY_RATE) != 0 {
                    return charge;
                }
                let penalty = if close {
                    1
                } else {
                    let outer = (dist_sqr.sqrt() as f32 - NO_GROWTH_RADIUS as f32).powi(2);
                    let reach = (24 - NO_GROWTH_RADIUS) * (24 - NO_GROWTH_RADIUS);
                    let factor = 1.0f32.min(outer / reach as f32);
                    1.max((charge as f32 * factor * 0.5f32) as i32)
                };
                charge - penalty
            }
        }
    }

    /// `SculkBlock.canPlaceGrowth`.
    fn can_place_growth(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        let above = self.info(cx, offset(pos, 0, 1, 0));
        let open = above.fact_in(BlockFact::Air)
            || (above.has(I_WATER)
                && above.fact_in(BlockFact::Water)
                && above.fact_in(BlockFact::FluidSource));
        if !open {
            return false;
        }
        let mut matched = 0;
        // `BlockPos.betweenClosed` walks x fastest, then y, then z.
        for dz in -4..=4 {
            for dy in 0..=2 {
                for dx in -4..=4 {
                    if self.info(cx, offset(pos, dx, dy, dz)).has(I_INHIBITOR) {
                        matched += 1;
                        if matched >= 3 {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// `SculkVeinBlock.attemptPlaceSculk`: turns the block a vein clings to into sculk.
    fn place_sculk(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        let faces = self.info(cx, pos).faces();
        let mut directions = Dir::ALL;
        shuffle(&mut directions, cx.random);
        for support in directions {
            if faces & face_bit(support) == 0 {
                continue;
            }
            let support_pos = relative(pos, support, 1);
            if !self.info(cx, support_pos).has(I_REPLACEABLE_WORLD_GEN) {
                continue;
            }
            let sculk = Self::id(cx, &self.ids.sculk, &self.sculk);
            Self::write_id(cx, support_pos, sculk);
            self.spread_all(cx, None, support_pos, &ALL_SPREADS);
            let skip = support.opposite();
            for direction in Dir::ALL {
                if direction == skip {
                    continue;
                }
                let vein_pos = relative(support_pos, direction, 1);
                let possible = self.read(cx, vein_pos);
                if possible.info.has(I_VEIN) {
                    self.on_discharged(cx, Behaviour::Vein, &possible, vein_pos);
                }
            }
            return true;
        }
        false
    }

    /// `SculkVeinBlock.onDischarged`: drops the faces that now touch sculk, and the vein itself
    /// once it has none.
    fn on_discharged(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        behaviour: Behaviour,
        read: &Read,
        pos: BlockPos,
    ) {
        if behaviour != Behaviour::Vein || !read.info.has(I_VEIN) {
            return;
        }
        if let Some(state) = &read.state {
            return self.on_discharged_state(cx, state, pos);
        }
        let mut faces = read.info.faces();
        for direction in Dir::ALL {
            if faces & face_bit(direction) != 0
                && self.info(cx, relative(pos, direction, 1)).has(I_SCULK)
            {
                faces &= !face_bit(direction);
            }
        }
        let id = if faces == 0 {
            if self.info(cx, pos).any_fluid() {
                Self::id(cx, &self.ids.water, &self.water)
            } else {
                Self::id(cx, &self.ids.air, &self.air)
            }
        } else {
            self.vein_id(cx, faces, read.info.has(I_WATERLOGGED))
        };
        Self::write_id(cx, pos, id);
    }

    /// [`Self::on_discharged`] for a vein that is not canonical, worked on its state.
    fn on_discharged_state(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        state: &BlockState,
        pos: BlockPos,
    ) {
        let mut state = state.clone();
        for direction in Dir::ALL {
            if Self::has_face(&state, direction)
                && self.info(cx, relative(pos, direction, 1)).has(I_SCULK)
            {
                state = Self::with_face(state, direction, false);
            }
        }
        if !Dir::ALL
            .into_iter()
            .any(|direction| Self::has_face(&state, direction))
        {
            state = if self.info(cx, pos).any_fluid() {
                self.water.clone()
            } else {
                self.air.clone()
            };
        }
        cx.region.set_block(pos, state);
    }

    /// `ChargeCursor.getValidMovementPos`.
    fn valid_movement(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        origin: BlockPos,
    ) -> Option<BlockPos> {
        let mut offsets = NON_CORNER_NEIGHBOURS;
        shuffle(&mut offsets, cx.random);
        let mut best = pos;
        for (x, y, z) in offsets {
            let neighbour = offset(pos, x, y, z);
            let dx = origin.x.wrapping_sub(neighbour.x);
            let dz = origin.z.wrapping_sub(neighbour.z);
            if dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz)) > 144 {
                continue;
            }
            let transferee = self.info(cx, neighbour);
            if transferee.behaviour() == Behaviour::Default {
                continue;
            }
            if !self.unobstructed_move(cx, pos, neighbour) {
                continue;
            }
            best = neighbour;
            if transferee.has(I_VEIN) {
                let faces = transferee.faces();
                let substrate = Dir::ALL.into_iter().any(|direction| {
                    faces & face_bit(direction) != 0
                        && self
                            .info(cx, relative(neighbour, direction, 1))
                            .has(I_REPLACEABLE)
                });
                if substrate {
                    break;
                }
            }
        }
        (best != pos).then_some(best)
    }

    /// `ChargeCursor.isMovementUnobstructed`.
    fn unobstructed_move(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        from: BlockPos,
        to: BlockPos,
    ) -> bool {
        if dist_manhattan(from, to) == 1 {
            return true;
        }
        let (dx, dy, dz) = (
            to.x.wrapping_sub(from.x),
            to.y.wrapping_sub(from.y),
            to.z.wrapping_sub(from.z),
        );
        let pick =
            |value: i32, negative: Dir, positive: Dir| if value < 0 { negative } else { positive };
        let dir_x = pick(dx, Dir::West, Dir::East);
        let dir_y = pick(dy, Dir::Down, Dir::Up);
        let dir_z = pick(dz, Dir::North, Dir::South);
        let (a, b) = if dx == 0 {
            (dir_y, dir_z)
        } else if dy == 0 {
            (dir_x, dir_z)
        } else {
            (dir_x, dir_y)
        };
        self.unobstructed(cx, from, a) || self.unobstructed(cx, from, b)
    }

    fn unobstructed(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        from: BlockPos,
        direction: Dir,
    ) -> bool {
        !self.face_full(
            cx,
            relative(from, direction, 1),
            direction.opposite(),
            false,
        )
    }
}
