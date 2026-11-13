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

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::cave_feature::SculkPatchConfig;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::{BlockFact, BlockFacts};

use super::support::{
    any_fluid, can_attach_to, dist_manhattan, face_full, has, is_block, offset, relative, shuffle, state_has,
    water_source, waterlogged, Dir,
};
use crate::decorator::context::{block_in, FeatureContext};
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
    facings: Option<Vec<Dir>>,
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
        &oxide_datapack::placement::BlockStateSpec { name: format!("minecraft:{name}"), properties: Default::default() },
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
        state.properties.get(face.name()).is_some_and(|value| value == "true")
    }

    fn with_face(state: BlockState, face: Dir, value: bool) -> BlockState {
        state.with_property(face.name(), value.to_string())
    }

    fn state_at(cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> BlockState {
        block_in(cx.region, pos).clone()
    }

    fn write(cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, state: BlockState) -> bool {
        cx.region.set_block(pos, state)
    }

    fn in_tag(cx: &FeatureContext<'_, '_, '_>, tag: &str, state: &BlockState) -> bool {
        cx.env.block_tags.contains(&ResourceLocation::minecraft(tag), &state.name)
    }

    /// `SculkPatchFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
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
                        cursors.push(Cursor { pos: origin, charge: current, update_delay: 0, decay_delay: 1, facings: None });
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
        if cx.random.next_float() <= self.catalyst_chance && has(cx, below, BlockFact::FullCollision) {
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
        let start = Self::state_at(cx, origin);
        if self.behaviour(&start) != Behaviour::Default {
            return true;
        }
        let water_source_block = is_block(&start, "water") && state_has(cx, &start, BlockFact::FluidSource);
        if !state_has(cx, &start, BlockFact::Air) && !water_source_block {
            return false;
        }
        Dir::ALL.into_iter().any(|direction| has(cx, relative(origin, direction, 1), BlockFact::FullCollision))
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
    fn update(&self, cx: &mut FeatureContext<'_, '_, '_>, origin: BlockPos, cursor: &mut Cursor, spread_veins: bool) {
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
            && self.attempt_spread_vein(cx, behaviour, cursor.pos, &state, cursor.facings.as_deref())
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
                state = Self::state_at(cx, target);
            }
            None => {
                self.on_discharged(cx, behaviour, &state, cursor.pos);
                cursor.charge = 0;
                return;
            }
        }
        if self.behaviour(&state) != Behaviour::Default {
            cursor.facings = Some(Dir::ALL.into_iter().filter(|face| Self::has_face(&state, *face)).collect());
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
        facings: Option<&[Dir]>,
    ) -> bool {
        if behaviour == Behaviour::Default {
            match facings {
                None => {
                    let current = Self::state_at(cx, pos);
                    return self.spread_all(cx, &current, pos, &[Spread::SamePosition]) > 0;
                }
                Some(faces) if !faces.is_empty() => {
                    if !state_has(cx, state, BlockFact::Air) && !water_source(cx, state) {
                        return false;
                    }
                    return self.regrow(cx, pos, state, faces);
                }
                Some(_) => {}
            }
        }
        self.spread_all(cx, state, pos, &ALL_SPREADS) > 0
    }

    /// `SculkVeinBlock.regrow`.
    fn regrow(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, existing: &BlockState, faces: &[Dir]) -> bool {
        let mut state = self.vein.clone();
        let mut any = false;
        for &face in faces {
            if can_attach_to(cx, pos, face) {
                state = Self::with_face(state, face, true);
                any = true;
            }
        }
        if !any {
            return false;
        }
        if any_fluid(cx, existing) {
            state = waterlogged(state, true);
        }
        Self::write(cx, pos, state);
        true
    }

    /// `MultifaceSpreader.spreadAll` with the sculk vein spreader's configuration.
    fn spread_all(&self, cx: &mut FeatureContext<'_, '_, '_>, state: &BlockState, pos: BlockPos, types: &[Spread]) -> i64 {
        let other_source = !self.is_vein(state);
        let mut count = 0;
        for face in Dir::ALL {
            if !(other_source || Self::has_face(state, face)) {
                continue;
            }
            for direction in Dir::ALL {
                if direction.axis() == face.axis() {
                    continue;
                }
                if !(other_source || (Self::has_face(state, face) && !Self::has_face(state, direction))) {
                    continue;
                }
                let mut target = None;
                for kind in types {
                    let candidate = match kind {
                        Spread::SamePosition => (pos, direction),
                        Spread::SamePlane => (relative(pos, direction, 1), face),
                        Spread::WrapAround => (relative(relative(pos, direction, 1), face, 1), direction.opposite()),
                    };
                    if self.can_spread_into(cx, pos, candidate.0, candidate.1) {
                        target = Some(candidate);
                        break;
                    }
                }
                let Some((target, face_to)) = target else {
                    continue;
                };
                let old = Self::state_at(cx, target);
                let Some(new) = self.state_for_placement(cx, &old, target, face_to) else {
                    continue;
                };
                if Self::write(cx, target, new) {
                    count += 1;
                }
            }
        }
        count
    }

    fn valid_for_placement(&self, cx: &mut FeatureContext<'_, '_, '_>, old: &BlockState, pos: BlockPos, face: Dir) -> bool {
        if self.is_vein(old) && Self::has_face(old, face) {
            return false;
        }
        can_attach_to(cx, pos, face)
    }

    /// `SculkVeinBlock.getStateForPlacement`.
    fn state_for_placement(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        old: &BlockState,
        pos: BlockPos,
        face: Dir,
    ) -> Option<BlockState> {
        if !self.valid_for_placement(cx, old, pos, face) {
            return None;
        }
        let base = if self.is_vein(old) {
            old.clone()
        } else if water_source(cx, old) {
            waterlogged(self.vein.clone(), true)
        } else {
            self.vein.clone()
        };
        Some(Self::with_face(base, face, true))
    }

    /// `MultifaceSpreader.canSpreadInto` with the sculk vein spreader's rules.
    fn can_spread_into(&self, cx: &mut FeatureContext<'_, '_, '_>, source: BlockPos, target: BlockPos, face: Dir) -> bool {
        let existing = Self::state_at(cx, target);
        let against = Self::state_at(cx, relative(target, face, 1));
        if ["sculk", "sculk_catalyst", "moving_piston"].iter().any(|blocker| is_block(&against, blocker)) {
            return false;
        }
        if dist_manhattan(source, target) == 2 {
            let neighbor = relative(source, face.opposite(), 1);
            if face_full(cx, neighbor, face, false) {
                return false;
            }
        }
        if any_fluid(cx, &existing) && !water_source(cx, &existing) {
            return false;
        }
        if Self::in_tag(cx, "fire", &existing) {
            return false;
        }
        let replaceable = state_has(cx, &existing, BlockFact::Replaceable)
            || state_has(cx, &existing, BlockFact::Air)
            || self.is_vein(&existing)
            || (is_block(&existing, "water") && water_source(cx, &existing));
        replaceable && self.valid_for_placement(cx, &existing, target, face)
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
                        if growth.properties.contains_key("waterlogged") && any_fluid(cx, block_in(cx.region, above)) {
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
    fn is_inhibitor(&self, cx: &FeatureContext<'_, '_, '_>, state: &BlockState) -> bool {
        let tag = ResourceLocation::minecraft("sculk_growth_inhibitors");
        if cx.env.block_tags.get(&tag).is_some() {
            return cx.env.block_tags.contains(&tag, &state.name);
        }
        state.name == self.sensor.name || is_block(state, "sculk_shrieker")
    }

    /// `SculkBlock.canPlaceGrowth`.
    fn can_place_growth(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        let above = block_in(cx.region, offset(pos, 0, 1, 0));
        let open = state_has(cx, above, BlockFact::Air) || (is_block(above, "water") && water_source(cx, above));
        if !open {
            return false;
        }
        let mut matched = 0;
        // `BlockPos.betweenClosed` walks x fastest, then y, then z.
        for dz in -4..=4 {
            for dy in 0..=2 {
                for dx in -4..=4 {
                    if self.is_inhibitor(cx, block_in(cx.region, offset(pos, dx, dy, dz))) {
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
        let state = Self::state_at(cx, pos);
        let mut directions = Dir::ALL;
        shuffle(&mut directions, cx.random);
        for support in directions {
            if !Self::has_face(&state, support) {
                continue;
            }
            let support_pos = relative(pos, support, 1);
            let support_state = block_in(cx.region, support_pos);
            if !(Self::in_tag(cx, "sculk_replaceable_world_gen", support_state)) {
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
                let possible = Self::state_at(cx, vein_pos);
                if self.is_vein(&possible) {
                    self.on_discharged(cx, Behaviour::Vein, &possible, vein_pos);
                }
            }
            return true;
        }
        false
    }

    /// `SculkVeinBlock.onDischarged`: drops the faces that now touch sculk, and the vein itself
    /// once it has none.
    fn on_discharged(&self, cx: &mut FeatureContext<'_, '_, '_>, behaviour: Behaviour, state: &BlockState, pos: BlockPos) {
        if behaviour != Behaviour::Vein || !self.is_vein(state) {
            return;
        }
        let mut state = state.clone();
        for direction in Dir::ALL {
            if Self::has_face(&state, direction) && block_in(cx.region, relative(pos, direction, 1)).name == self.sculk.name {
                state = Self::with_face(state, direction, false);
            }
        }
        if !Dir::ALL.into_iter().any(|direction| Self::has_face(&state, direction)) {
            let here = block_in(cx.region, pos);
            state = if any_fluid(cx, here) { self.water.clone() } else { self.air.clone() };
        }
        Self::write(cx, pos, state);
    }

    /// `ChargeCursor.getValidMovementPos`.
    fn valid_movement(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos, origin: BlockPos) -> Option<BlockPos> {
        // `NON_CORNER_NEIGHBOURS` in `BlockPos.betweenClosed` order: x fastest, then y, then z.
        let mut offsets = Vec::with_capacity(18);
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    if (x == 0 || y == 0 || z == 0) && (x, y, z) != (0, 0, 0) {
                        offsets.push((x, y, z));
                    }
                }
            }
        }
        shuffle(&mut offsets, cx.random);
        let mut best = pos;
        for (x, y, z) in offsets {
            let neighbour = offset(pos, x, y, z);
            let dx = origin.x.wrapping_sub(neighbour.x);
            let dz = origin.z.wrapping_sub(neighbour.z);
            if dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz)) > 144 {
                continue;
            }
            let transferee = Self::state_at(cx, neighbour);
            if self.behaviour(&transferee) == Behaviour::Default || !self.unobstructed_move(cx, pos, neighbour) {
                continue;
            }
            best = neighbour;
            if self.is_vein(&transferee) {
                let substrate = Dir::ALL.into_iter().any(|direction| {
                    Self::has_face(&transferee, direction)
                        && Self::in_tag(cx, "sculk_replaceable", block_in(cx.region, relative(neighbour, direction, 1)))
                });
                if substrate {
                    break;
                }
            }
        }
        (best != pos).then_some(best)
    }

    /// `ChargeCursor.isMovementUnobstructed`.
    fn unobstructed_move(&self, cx: &mut FeatureContext<'_, '_, '_>, from: BlockPos, to: BlockPos) -> bool {
        if dist_manhattan(from, to) == 1 {
            return true;
        }
        let (dx, dy, dz) = (to.x.wrapping_sub(from.x), to.y.wrapping_sub(from.y), to.z.wrapping_sub(from.z));
        let pick = |value: i32, negative: Dir, positive: Dir| if value < 0 { negative } else { positive };
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
        !face_full(cx, relative(from, direction, 1), direction.opposite(), false)
    }
}
