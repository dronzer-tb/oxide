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

use std::collections::HashSet;
use std::ops::Deref;

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::cave_feature::SculkPatchConfig;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::{BlockFact, BlockFacts};

use super::support::{
    any_fluid, can_attach_to, dist_manhattan, face_full, has, is_block, offset, relative, shuffle,
    state_has, water_source, waterlogged, Dir,
};
use crate::decorator::context::{block_in, fact_in, FeatureContext};
use crate::decorator::provider::sample_int;
use crate::decorator::state_provider::state_from_spec;

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
    /// The faces of the vein or sculk the cursor last moved onto, as a [`face_mask`].
    facings: Option<u8>,
}

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
    vein: BlockState,
    catalyst: BlockState,
    shrieker: BlockState,
    sensor: BlockState,
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
        Ok(Some(Self {
            charge_count: config.charge_count,
            amount_per_charge: config.amount_per_charge,
            spread_attempts: config.spread_attempts,
            growth_rounds: config.growth_rounds,
            spread_rounds: config.spread_rounds,
            extra_rare_growths: config.extra_rare_growths.clone(),
            catalyst_chance: config.catalyst_chance,
            sculk: default_of("sculk", facts)?,
            vein: default_of("sculk_vein", facts)?,
            catalyst: default_of("sculk_catalyst", facts)?,
            shrieker: default_of("sculk_shrieker", facts)?.with_property("can_summon", "true"),
            sensor: default_of("sculk_sensor", facts)?,
            water: default_of("water", facts)?,
            air: default_of("air", facts)?,
        }))
    }

    /// `SculkPatchFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let tags = &cx.env.block_tags;
        let tag = |path: &str| tags.get(&ResourceLocation::minecraft(path));
        let spreader = Spreader {
            feature: self,
            fire: tag("fire"),
            replaceable: tag("sculk_replaceable"),
            replaceable_world_gen: tag("sculk_replaceable_world_gen"),
            inhibitors: tag("sculk_growth_inhibitors"),
        };
        spreader.place(cx)
    }
}

/// One `place` call's view of the feature: the block tags it tests, resolved once rather than
/// looked up by a freshly built id on every test.
struct Spreader<'s, 'e> {
    feature: &'s SculkPatchFeature,
    fire: Option<&'e HashSet<ResourceLocation>>,
    replaceable: Option<&'e HashSet<ResourceLocation>>,
    replaceable_world_gen: Option<&'e HashSet<ResourceLocation>>,
    /// `None` when the pack has no `sculk_growth_inhibitors` tag (see the module doc).
    inhibitors: Option<&'e HashSet<ResourceLocation>>,
}

impl Deref for Spreader<'_, '_> {
    type Target = SculkPatchFeature;

    fn deref(&self) -> &SculkPatchFeature {
        self.feature
    }
}

/// The faces `state` has set, in `Dir::ALL` order.
fn faces_of(state: &BlockState) -> [bool; 6] {
    Dir::ALL.map(|face| Spreader::has_face(state, face))
}

/// The faces `state` has set, bit `i` for `Dir::ALL[i]`.
fn face_mask(state: &BlockState) -> u8 {
    faces_of(state)
        .into_iter()
        .enumerate()
        .fold(0, |mask, (i, set)| mask | (u8::from(set) << i))
}

/// A block's offset from the source of one spread, each coordinate in `-1..=1`.
type Local = [i32; 3];

const SOURCE: Local = [0, 0, 0];

fn step(at: Local, direction: Dir) -> Local {
    let (dx, dy, dz) = direction.step();
    [at[0] + dx, at[1] + dy, at[2] + dz]
}

// What one read of a block tells a spread, as `Near` keeps it.
const NEAR_READ: u16 = 1 << 0;
/// `sculk`, `sculk_catalyst` or `moving_piston`: what a vein may not spread against.
const NEAR_BLOCKER: u16 = 1 << 1;
const NEAR_VEIN: u16 = 1 << 2;
/// `minecraft:water`, whatever its properties.
const NEAR_WATER: u16 = 1 << 3;
const NEAR_FIRE: u16 = 1 << 4;
/// A vein's six faces from here, in `Dir::ALL` order.
const NEAR_FACE_SHIFT: u32 = 5;

/// What a spread has read of one block: its name bits and the facts asked so far.
#[derive(Clone, Copy, Default)]
struct Slot {
    names: u16,
    /// The block is outside the region's window, where `fact_at` has no answer.
    outside: bool,
    asked: u32,
    facts: u32,
}

/// The 3x3x3 blocks around the source of one `spread_all`, read from the region as first
/// asked. Every block a spread looks at -- its targets, what they spread against, the faces they
/// attach to -- is the source, a face neighbour or an edge neighbour, and the spread itself is
/// the only writer while it runs; it forgets the block it writes.
struct Near {
    source: BlockPos,
    slots: [Slot; 27],
}

impl Near {
    fn new(source: BlockPos) -> Self {
        Self {
            source,
            slots: [Slot::default(); 27],
        }
    }

    fn pos(&self, at: Local) -> BlockPos {
        offset(self.source, at[0], at[1], at[2])
    }

    fn index(at: Local) -> usize {
        debug_assert!(at.iter().all(|c| (-1..=1).contains(c)));
        ((at[2] + 1) * 9 + (at[1] + 1) * 3 + (at[0] + 1)) as usize
    }

    fn forget(&mut self, at: Local) {
        self.slots[Self::index(at)] = Slot::default();
    }

    /// `DecorationRegion::fact_at` for the block at `at`.
    fn fact_at(
        &mut self,
        cx: &FeatureContext<'_, '_, '_>,
        at: Local,
        fact: BlockFact,
    ) -> Option<bool> {
        let pos = self.pos(at);
        let slot = &mut self.slots[Self::index(at)];
        let bit = fact as u32;
        if slot.outside {
            return None;
        }
        if slot.asked & bit == 0 {
            let Some(answer) = cx.region.fact_at(pos, fact) else {
                slot.outside = true;
                return None;
            };
            slot.asked |= bit;
            if answer {
                slot.facts |= bit;
            }
        }
        Some(slot.facts & bit != 0)
    }

    /// `region.is`: nothing holds outside the window.
    fn is(&mut self, cx: &FeatureContext<'_, '_, '_>, at: Local, fact: BlockFact) -> bool {
        self.fact_at(cx, at, fact).unwrap_or(false)
    }

    /// `fact_in`: outside the window the block is `void_air`.
    fn fact_in(&mut self, cx: &FeatureContext<'_, '_, '_>, at: Local, fact: BlockFact) -> bool {
        match self.fact_at(cx, at, fact) {
            Some(answer) => answer,
            None => fact_in(cx.region, self.pos(at), fact),
        }
    }
}

fn face_index(face: Dir) -> usize {
    Dir::ALL
        .iter()
        .position(|&d| d == face)
        .expect("a direction")
}

impl Spreader<'_, '_> {
    fn behaviour(&self, state: &BlockState) -> Behaviour {
        if state.name == self.sculk.name {
            Behaviour::Sculk
        } else if state.name == self.vein.name {
            Behaviour::Vein
        } else {
            Behaviour::Default
        }
    }

    fn is_vein(&self, state: &BlockState) -> bool {
        state.name == self.vein.name
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

    fn state_at(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> BlockState {
        block_in(cx.region, pos).clone()
    }

    fn member(tag: Option<&HashSet<ResourceLocation>>, state: &BlockState) -> bool {
        tag.is_some_and(|members| members.contains(&state.name))
    }

    fn write(cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, state: BlockState) -> bool {
        cx.region.set_block(pos, state)
    }

    /// The `NEAR_*` name bits of the block at `at`, read once per slot.
    fn names(&self, cx: &FeatureContext<'_, '_, '_>, near: &mut Near, at: Local) -> u16 {
        let index = Near::index(at);
        if near.slots[index].names & NEAR_READ == 0 {
            let state = block_in(cx.region, near.pos(at));
            let mut names = NEAR_READ;
            if ["sculk", "sculk_catalyst", "moving_piston"]
                .iter()
                .any(|blocker| is_block(state, blocker))
            {
                names |= NEAR_BLOCKER;
            }
            if self.is_vein(state) {
                names |= NEAR_VEIN | (u16::from(face_mask(state)) << NEAR_FACE_SHIFT);
            }
            if is_block(state, "water") {
                names |= NEAR_WATER;
            }
            if Self::member(self.fire, state) {
                names |= NEAR_FIRE;
            }
            near.slots[index].names = names;
        }
        near.slots[index].names
    }

    /// `SculkPatchFeature.place`.
    fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !self.can_spread_from(cx, origin) {
            return false;
        }
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
            Self::write(cx, origin, self.catalyst.clone());
        }
        let extra = sample_int(&self.extra_rare_growths, cx.random);
        for _ in 0..extra {
            let dx = cx.random.next_int_bounded(5) - 2;
            let dz = cx.random.next_int_bounded(5) - 2;
            let pos = offset(origin, dx, 0, dz);
            let below = offset(pos, 0, -1, 0);
            if has(cx, pos, BlockFact::Air) && face_full(cx, below, Dir::Up, false) {
                Self::write(cx, pos, self.shrieker.clone());
            }
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

    /// `SculkSpreader.updateCursors`.
    fn update_cursors(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        origin: BlockPos,
        cursors: Vec<Cursor>,
        spread_veins: bool,
    ) -> Vec<Cursor> {
        let mut processed: Vec<Cursor> = Vec::new();
        // The mergeable cursor charge per position, as an index into `processed`.
        let mut mergeable: Vec<(BlockPos, usize, i32)> = Vec::new();
        for mut cursor in cursors {
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
                continue;
            }
            self.update(cx, origin, &mut cursor, spread_veins);
            if cursor.charge <= 0 {
                continue;
            }
            match mergeable.iter_mut().find(|(pos, _, _)| *pos == cursor.pos) {
                None => {
                    mergeable.push((cursor.pos, processed.len(), cursor.charge));
                    processed.push(cursor);
                }
                Some(entry) => {
                    // World generation never merges cursors.
                    if cursor.charge < entry.2 {
                        *entry = (cursor.pos, processed.len(), cursor.charge);
                    }
                    processed.push(cursor);
                }
            }
        }
        processed
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
        let mut state = Self::state_at(cx, cursor.pos);
        let mut behaviour = self.behaviour(&state);
        if spread_veins
            && self.attempt_spread_vein(
                cx,
                behaviour,
                cursor.pos,
                &state,
                cursor.facings,
            )
            && behaviour != Behaviour::Sculk
        {
            state = Self::state_at(cx, cursor.pos);
            behaviour = self.behaviour(&state);
        }
        cursor.charge = self.attempt_use_charge(cx, origin, behaviour, cursor, spread_veins);
        if cursor.charge <= 0 {
            self.on_discharged(cx, behaviour, &state, cursor.pos);
            return;
        }
        match self.valid_movement(cx, cursor.pos, origin) {
            Some(target) => {
                self.on_discharged(cx, behaviour, &state, cursor.pos);
                cursor.pos = target;
            }
            None => {
                self.on_discharged(cx, behaviour, &state, cursor.pos);
                cursor.charge = 0;
                return;
            }
        }
        let arrived = block_in(cx.region, cursor.pos);
        if self.behaviour(arrived) != Behaviour::Default {
            cursor.facings = Some(face_mask(arrived));
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
        state: &BlockState,
        facings: Option<u8>,
    ) -> bool {
        // `state` is the block at `pos`: nothing has written since the cursor read it, so the
        // region's cached facts at `pos` answer for it.
        if behaviour == Behaviour::Default {
            match facings {
                None => {
                    return self.spread_all(cx, state, pos, &[Spread::SamePosition]) > 0;
                }
                Some(faces) if faces != 0 => {
                    let water_source_here =
                        has(cx, pos, BlockFact::Water) && has(cx, pos, BlockFact::FluidSource);
                    if !has(cx, pos, BlockFact::Air) && !water_source_here {
                        return false;
                    }
                    return self.regrow(cx, pos, faces);
                }
                Some(_) => {}
            }
        }
        self.spread_all(cx, state, pos, &ALL_SPREADS) > 0
    }

    /// `SculkVeinBlock.regrow`.
    fn regrow(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        faces: u8,
    ) -> bool {
        let mut state = self.vein.clone();
        let mut any = false;
        for (i, face) in Dir::ALL.into_iter().enumerate() {
            if faces & (1 << i) != 0 && can_attach_to(cx, pos, face) {
                state = Self::with_face(state, face, true);
                any = true;
            }
        }
        if !any {
            return false;
        }
        // The block at `pos` is still the one the cursor read.
        if has(cx, pos, BlockFact::Water) || has(cx, pos, BlockFact::Lava) {
            state = waterlogged(state, true);
        }
        Self::write(cx, pos, state);
        true
    }

    /// `MultifaceSpreader.spreadAll` with the sculk vein spreader's configuration. `state` gives
    /// the source's veinness and faces; the blocks around it are read through one [`Near`].
    fn spread_all(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        state: &BlockState,
        pos: BlockPos,
        types: &[Spread],
    ) -> i64 {
        let other_source = !self.is_vein(state);
        let faces = faces_of(state);
        let mut near = Near::new(pos);
        let mut count = 0;
        for (i, face) in Dir::ALL.into_iter().enumerate() {
            if !(other_source || faces[i]) {
                continue;
            }
            for (j, direction) in Dir::ALL.into_iter().enumerate() {
                if direction.axis() == face.axis() {
                    continue;
                }
                if !(other_source || (faces[i] && !faces[j])) {
                    continue;
                }
                let mut target = None;
                for kind in types {
                    let candidate = match kind {
                        Spread::SamePosition => (SOURCE, direction),
                        Spread::SamePlane => (step(SOURCE, direction), face),
                        Spread::WrapAround => (
                            step(step(SOURCE, direction), face),
                            direction.opposite(),
                        ),
                    };
                    if self.can_spread_into(cx, &mut near, candidate.0, candidate.1) {
                        target = Some(candidate);
                        break;
                    }
                }
                let Some((target, face_to)) = target else {
                    continue;
                };
                let Some(new) = self.state_for_placement(cx, &mut near, target, face_to) else {
                    continue;
                };
                let written = Self::write(cx, near.pos(target), new);
                near.forget(target);
                if written {
                    count += 1;
                }
            }
        }
        count
    }

    /// `support::face_full` for the block at `at`, its facts read through `near`.
    fn near_face_full(
        cx: &mut FeatureContext<'_, '_, '_>,
        near: &mut Near,
        at: Local,
        face: Dir,
        collision: bool,
    ) -> bool {
        match face {
            Dir::Up | Dir::Down => {
                let sturdy = if face == Dir::Up {
                    BlockFact::SturdyTop
                } else {
                    BlockFact::SturdyBottom
                };
                near.is(cx, at, sturdy) || (collision && near.is(cx, at, BlockFact::FullCollision))
            }
            _ => {
                if near.is(cx, at, BlockFact::Air) || near.is(cx, at, BlockFact::NoCollision) {
                    return false;
                }
                if near.is(cx, at, BlockFact::FullCollision) {
                    return collision || near.is(cx, at, BlockFact::SturdyTop);
                }
                let name = block_in(cx.region, near.pos(at)).name.to_string();
                cx.unsupported.record("shape", &name);
                false
            }
        }
    }

    /// `MultifaceBlock.canAttachTo` for the block at `at` looking at `direction`.
    fn near_can_attach_to(
        cx: &mut FeatureContext<'_, '_, '_>,
        near: &mut Near,
        at: Local,
        direction: Dir,
    ) -> bool {
        Self::near_face_full(cx, near, step(at, direction), direction.opposite(), true)
    }

    /// `SculkVeinBlock.getStateForPlacement` for the block now at `at`. `can_spread_into` has
    /// just read it and nothing has written since.
    fn state_for_placement(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        near: &mut Near,
        at: Local,
        face: Dir,
    ) -> Option<BlockState> {
        let names = self.names(cx, near, at);
        let vein = names & NEAR_VEIN != 0;
        let has_face = names & (1 << (NEAR_FACE_SHIFT + face_index(face) as u32)) != 0;
        if (vein && has_face) || !Self::near_can_attach_to(cx, near, at, face) {
            return None;
        }
        let base = if vein {
            block_in(cx.region, near.pos(at)).clone()
        } else if near.fact_in(cx, at, BlockFact::Water)
            && near.fact_in(cx, at, BlockFact::FluidSource)
        {
            waterlogged(self.vein.clone(), true)
        } else {
            self.vein.clone()
        };
        Some(Self::with_face(base, face, true))
    }

    /// `MultifaceSpreader.canSpreadInto` with the sculk vein spreader's rules, from the source
    /// of `near` into the block at `target`.
    fn can_spread_into(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        near: &mut Near,
        target: Local,
        face: Dir,
    ) -> bool {
        if self.names(cx, near, step(target, face)) & NEAR_BLOCKER != 0 {
            return false;
        }
        // `distManhattan(source, target)`: exact for offsets this small.
        if target.iter().map(|c| c.abs()).sum::<i32>() == 2 {
            let neighbor = step(SOURCE, face.opposite());
            if Self::near_face_full(cx, near, neighbor, face, false) {
                return false;
            }
        }
        let existing = self.names(cx, near, target);
        let water = near.fact_in(cx, target, BlockFact::Water);
        let any_fluid_here = water || near.fact_in(cx, target, BlockFact::Lava);
        let water_source_here = water && near.fact_in(cx, target, BlockFact::FluidSource);
        if any_fluid_here && !water_source_here {
            return false;
        }
        if existing & NEAR_FIRE != 0 {
            return false;
        }
        let replaceable = near.fact_in(cx, target, BlockFact::Replaceable)
            || near.fact_in(cx, target, BlockFact::Air)
            || existing & NEAR_VEIN != 0
            || (existing & NEAR_WATER != 0 && water_source_here);
        let has_face = existing & (1 << (NEAR_FACE_SHIFT + face_index(face) as u32)) != 0;
        replaceable
            && !(existing & NEAR_VEIN != 0 && has_face)
            && Self::near_can_attach_to(cx, near, target, face)
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
                        let mut growth = if cx.random.next_int_bounded(11) == 0 {
                            self.shrieker.clone()
                        } else {
                            self.sensor.clone()
                        };
                        if growth.properties.contains_key("waterlogged")
                            && any_fluid(cx, block_in(cx.region, above))
                        {
                            growth = waterlogged(growth, true);
                        }
                        Self::write(cx, above, growth);
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

    /// Whether `state` counts toward the cap on nearby sensors and shriekers.
    fn is_inhibitor(&self, state: &BlockState) -> bool {
        match self.inhibitors {
            Some(members) => members.contains(&state.name),
            None => state.name == self.sensor.name || is_block(state, "sculk_shrieker"),
        }
    }

    /// `SculkBlock.canPlaceGrowth`.
    fn can_place_growth(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        let above = block_in(cx.region, offset(pos, 0, 1, 0));
        let open = state_has(cx, above, BlockFact::Air)
            || (is_block(above, "water") && water_source(cx, above));
        if !open {
            return false;
        }
        let mut matched = 0;
        // `BlockPos.betweenClosed` walks x fastest, then y, then z.
        for dz in -4..=4 {
            for dy in 0..=2 {
                for dx in -4..=4 {
                    if self.is_inhibitor(block_in(cx.region, offset(pos, dx, dy, dz))) {
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
        let faces = faces_of(block_in(cx.region, pos));
        let mut directions = Dir::ALL;
        shuffle(&mut directions, cx.random);
        for support in directions {
            if !faces[face_index(support)] {
                continue;
            }
            let support_pos = relative(pos, support, 1);
            let support_state = block_in(cx.region, support_pos);
            if !Self::member(self.replaceable_world_gen, support_state) {
                continue;
            }
            Self::write(cx, support_pos, self.sculk.clone());
            self.spread_all(cx, &self.sculk, support_pos, &ALL_SPREADS);
            let skip = support.opposite();
            for direction in Dir::ALL {
                if direction == skip {
                    continue;
                }
                let vein_pos = relative(support_pos, direction, 1);
                if self.is_vein(block_in(cx.region, vein_pos)) {
                    let possible = Self::state_at(cx, vein_pos);
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
        state: &BlockState,
        pos: BlockPos,
    ) {
        if behaviour != Behaviour::Vein || !self.is_vein(state) {
            return;
        }
        let mut state = state.clone();
        for direction in Dir::ALL {
            if Self::has_face(&state, direction)
                && block_in(cx.region, relative(pos, direction, 1)).name == self.sculk.name
            {
                state = Self::with_face(state, direction, false);
            }
        }
        if !Dir::ALL
            .into_iter()
            .any(|direction| Self::has_face(&state, direction))
        {
            let here = block_in(cx.region, pos);
            state = if any_fluid(cx, here) {
                self.water.clone()
            } else {
                self.air.clone()
            };
        }
        Self::write(cx, pos, state);
    }

    /// `ChargeCursor.getValidMovementPos`.
    fn valid_movement(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        origin: BlockPos,
    ) -> Option<BlockPos> {
        // `NON_CORNER_NEIGHBOURS` in `BlockPos.betweenClosed` order: x fastest, then y, then z.
        let mut offsets = [(0, 0, 0); 18];
        let mut n = 0;
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    if (x == 0 || y == 0 || z == 0) && (x, y, z) != (0, 0, 0) {
                        offsets[n] = (x, y, z);
                        n += 1;
                    }
                }
            }
        }
        debug_assert_eq!(n, 18);
        shuffle(&mut offsets, cx.random);
        let mut best = pos;
        for (x, y, z) in offsets {
            let neighbour = offset(pos, x, y, z);
            let dx = origin.x.wrapping_sub(neighbour.x);
            let dz = origin.z.wrapping_sub(neighbour.z);
            if dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz)) > 144 {
                continue;
            }
            let transferee = block_in(cx.region, neighbour);
            if self.behaviour(transferee) == Behaviour::Default {
                continue;
            }
            let is_vein = self.is_vein(transferee);
            let faces = faces_of(transferee);
            if !self.unobstructed_move(cx, pos, neighbour) {
                continue;
            }
            best = neighbour;
            if is_vein {
                let substrate = Dir::ALL.into_iter().enumerate().any(|(i, direction)| {
                    faces[i]
                        && Self::member(
                            self.replaceable,
                            block_in(cx.region, relative(neighbour, direction, 1)),
                        )
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
        Self::unobstructed(cx, from, a) || Self::unobstructed(cx, from, b)
    }

    fn unobstructed(cx: &mut FeatureContext<'_, '_, '_>, from: BlockPos, direction: Dir) -> bool {
        !face_full(
            cx,
            relative(from, direction, 1),
            direction.opposite(),
            false,
        )
    }
}
