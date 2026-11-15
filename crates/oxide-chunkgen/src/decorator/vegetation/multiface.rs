//! `multiface_growth`: glow lichen and sculk veins, which cling to the faces of neighbouring
//! blocks and spread along them.
//!
//! Placement shuffles the faces it may attach to, tries the origin itself and then searches
//! outwards along each face; a block that goes down may spread onto further faces by chance.
//! Sculk veins additionally refuse to grow into sculk, fire or non-source fluid and use the
//! block-state rules of `SculkVeinBlock`'s spreader.
//!
//! Vanilla marks every written block for post-processing (so a waterlogged or unsupported vein
//! settles after the chunk is generated). The decoration region keeps only block writes, so that
//! scheduled settling is not reproduced; the written states are the same.
//!
//! PARITY-CHECK: the three sculk blockers (`sculk`, `sculk_catalyst`, `moving_piston`) are named
//! blocks in vanilla's spreader, not a tag; confirm the set against 26.2.

use anyhow::{bail, Context, Result};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::vegetation_feature::MultifaceGrowthConfig;
use oxide_datapack::{BlockFact, BlockFacts};

use super::geometry::{dist_manhattan, relative, shuffle, Dir};
use super::support::{can_attach_to, face_full, is_air, Shape};
use crate::decorator::context::{block_in, FeatureContext};
use crate::decorator::predicate::IdSet;

#[derive(Debug)]
pub struct MultifaceFeature {
    block: BlockState,
    sculk: bool,
    search_range: i32,
    directions: Vec<Dir>,
    chance_of_spreading: f32,
    can_be_placed_on: IdSet,
    water: ResourceLocation,
    sculk_blockers: [ResourceLocation; 3],
    sculk_vein: ResourceLocation,
    fire: ResourceLocation,
}

/// Where a spread lands relative to the block it comes from.
#[derive(Clone, Copy)]
enum Spread {
    SamePosition,
    SamePlane,
    WrapAround,
}

const SPREAD_ORDER: [Spread; 3] = [Spread::SamePosition, Spread::SamePlane, Spread::WrapAround];

impl MultifaceFeature {
    /// Compiles the config; `Ok(None)` for a block vanilla's codec would not accept here (only
    /// glow lichen and sculk veins are spreadable multiface blocks).
    pub fn compile(config: &MultifaceGrowthConfig, facts: &BlockFacts) -> Result<Option<Self>> {
        let name: ResourceLocation = match config.block.parse() {
            Ok(name) => name,
            Err(_) => bail!("invalid block id {:?}", config.block),
        };
        let sculk = match name.to_string().as_str() {
            "minecraft:glow_lichen" => false,
            "minecraft:sculk_vein" => true,
            _ => return Ok(None),
        };
        let mut directions = Vec::new();
        if config.can_place_on_ceiling {
            directions.push(Dir::Up);
        }
        if config.can_place_on_floor {
            directions.push(Dir::Down);
        }
        if config.can_place_on_wall {
            directions.extend(Dir::HORIZONTAL);
        }
        let named = |path: &str| ResourceLocation::minecraft(path);
        Ok(Some(Self {
            block: facts.complete(&BlockState::new(name)),
            sculk,
            search_range: config.search_range,
            directions,
            chance_of_spreading: config.chance_of_spreading,
            can_be_placed_on: IdSet::compile(&config.can_be_placed_on)
                .context("can_be_placed_on")?,
            water: named("water"),
            sculk_blockers: [
                named("sculk"),
                named("sculk_catalyst"),
                named("moving_piston"),
            ],
            sculk_vein: named("sculk_vein"),
            fire: named("fire"),
        }))
    }

    fn is_water(&self, state: &BlockState) -> bool {
        state.name == self.water
    }

    fn is_block(&self, state: &BlockState) -> bool {
        state.name == self.block.name
    }

    fn has_face(state: &BlockState, face: Dir) -> bool {
        state
            .properties
            .get(face.name())
            .is_some_and(|value| value == "true")
    }

    fn air_or_water(&self, cx: &FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        is_air(cx, pos) || self.is_water(block_in(cx.region, pos))
    }

    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !self.air_or_water(cx, origin) {
            return false;
        }
        let mut directions = self.directions.clone();
        shuffle(&mut directions, &mut *cx.random);
        let here = block_in(cx.region, origin).clone();
        if self.place_growth(cx, origin, &here, &directions) {
            return true;
        }
        for &search in &directions {
            let mut placement: Vec<Dir> = self
                .directions
                .iter()
                .copied()
                .filter(|&dir| dir != search.opposite())
                .collect();
            shuffle(&mut placement, &mut *cx.random);
            for _ in 0..self.search_range {
                // Vanilla's `setWithOffset(origin, search)` restarts from the origin every
                // iteration, so the search looks at the same neighbour `search_range` times.
                let pos = relative(origin, search, 1);
                let state = block_in(cx.region, pos).clone();
                if !self.air_or_water(cx, pos) && !self.is_block(&state) {
                    break;
                }
                if self.place_growth(cx, pos, &state, &placement) {
                    return true;
                }
            }
        }
        false
    }

    fn place_growth(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        old: &BlockState,
        directions: &[Dir],
    ) -> bool {
        for &direction in directions {
            let neighbour = block_in(cx.region, relative(pos, direction, 1));
            if !self
                .can_be_placed_on
                .contains_block(cx.env, &neighbour.name)
            {
                continue;
            }
            let Some(state) = self.state_for_placement(cx, old, pos, direction) else {
                return false;
            };
            cx.region.set_block(pos, state.clone());
            if cx.random.next_float() < self.chance_of_spreading {
                let mut all = Dir::ALL;
                shuffle(&mut all, &mut *cx.random);
                for spread in all {
                    if self.spread(cx, &state, pos, direction, spread) {
                        break;
                    }
                }
            }
            return true;
        }
        false
    }

    fn valid_for_placement(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        old: &BlockState,
        pos: BlockPos,
        direction: Dir,
    ) -> bool {
        if self.is_block(old) && Self::has_face(old, direction) {
            return false;
        }
        can_attach_to(cx, pos, direction)
    }

    /// `MultifaceBlock.getStateForPlacement`: the existing vein with one more face, or a fresh
    /// one (waterlogged when it replaces a water source).
    fn state_for_placement(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        old: &BlockState,
        pos: BlockPos,
        direction: Dir,
    ) -> Option<BlockState> {
        if !self.valid_for_placement(cx, old, pos, direction) {
            return None;
        }
        let mut state = if self.is_block(old) {
            old.clone()
        } else {
            let mut fresh = self.block.clone();
            let water_source = cx.env.facts.is(old, BlockFact::Water)
                && cx.env.facts.is(old, BlockFact::FluidSource);
            if water_source && fresh.properties.contains_key("waterlogged") {
                fresh.properties.insert("waterlogged".into(), "true".into());
            }
            fresh
        };
        if !state.properties.contains_key(direction.name()) {
            cx.unsupported.record(
                "feature",
                "minecraft:multiface_growth (block without face properties)",
            );
            return None;
        }
        state
            .properties
            .insert(direction.name().into(), "true".into());
        Some(state)
    }

    /// `MultifaceSpreader.spreadFromFaceTowardDirection`, post-processing aside.
    fn spread(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        state: &BlockState,
        pos: BlockPos,
        from: Dir,
        direction: Dir,
    ) -> bool {
        if direction.axis() == from.axis() {
            return false;
        }
        let other_source = self.sculk && state.name != self.sculk_vein;
        if !(other_source || (Self::has_face(state, from) && !Self::has_face(state, direction))) {
            return false;
        }
        for kind in SPREAD_ORDER {
            let (target, face) = match kind {
                Spread::SamePosition => (pos, direction),
                Spread::SamePlane => (relative(pos, direction, 1), from),
                Spread::WrapAround => (
                    relative(relative(pos, direction, 1), from, 1),
                    direction.opposite(),
                ),
            };
            if self.can_spread_into(cx, pos, target, face) {
                let old = block_in(cx.region, target).clone();
                let Some(new) = self.state_for_placement(cx, &old, target, face) else {
                    return false;
                };
                return cx.region.set_block(target, new);
            }
        }
        false
    }

    fn can_spread_into(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        source: BlockPos,
        target: BlockPos,
        face: Dir,
    ) -> bool {
        let existing = block_in(cx.region, target).clone();
        let replaceable = if self.sculk {
            let against = block_in(cx.region, relative(target, face, 1));
            if self.sculk_blockers.contains(&against.name) {
                false
            } else {
                let mut blocked = false;
                if dist_manhattan(source, target) == 2 {
                    let neighbour = relative(source, face.opposite(), 1);
                    blocked = face_full(cx, neighbour, face, Shape::Support);
                }
                let facts = &cx.env.facts;
                let fluid =
                    facts.is(&existing, BlockFact::Water) || facts.is(&existing, BlockFact::Lava);
                let water_source = facts.is(&existing, BlockFact::Water)
                    && facts.is(&existing, BlockFact::FluidSource);
                if blocked
                    || (fluid && !water_source)
                    || cx.env.block_tags.contains(&self.fire, &existing.name)
                {
                    false
                } else {
                    facts.is(&existing, BlockFact::Replaceable)
                        || self.default_replaceable(cx, &existing)
                }
            }
        } else {
            self.default_replaceable(cx, &existing)
        };
        replaceable && self.valid_for_placement(cx, &existing, target, face)
    }

    /// `MultifaceSpreader.DEFAULT_SPREAD_CONFIG`'s replace test: air, the vein itself, or a
    /// water source block.
    fn default_replaceable(&self, cx: &FeatureContext<'_, '_, '_>, existing: &BlockState) -> bool {
        let facts = &cx.env.facts;
        facts.is(existing, BlockFact::Air)
            || self.is_block(existing)
            || (existing.name == self.water && facts.is(existing, BlockFact::FluidSource))
    }
}
