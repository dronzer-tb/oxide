//! `vegetation_patch` and `waterlogged_vegetation_patch`: a blob of ground blocks on a cave
//! floor or ceiling, with a nested feature scattered over it.
//!
//! The ground is laid column by column over a rectangle, edge columns thinned by chance and
//! corners dropped. The waterlogged variant then floods every ground block whose sides and
//! underside are all sturdy and plants through the flooded surface instead.
//!
//! The set of ground positions is iterated in Java `HashSet` order ([`PosSet`]) because each
//! element draws a vegetation roll.

use std::sync::Arc;

use anyhow::{Context, Result};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::feature::Holder;
use oxide_datapack::placement::{IntProvider, PlacedFeature};
use oxide_datapack::vegetation_feature::{CaveSurface, VegetationPatchConfig};
use oxide_datapack::BlockFacts;

use super::geometry::{offset, relative, Dir, PosSet};
use super::support::{face_full, is_air, place_nested, Shape};
use crate::decorator::context::{block_in, FeatureContext};
use crate::decorator::feature::Placed;
use crate::decorator::predicate::IdSet;
use crate::decorator::provider::sample_int;
use crate::decorator::state_provider::{StateProvider, Unsupported};
use crate::region::StateKey;

#[derive(Debug)]
pub struct PatchFeature {
    waterlogged: bool,
    /// The direction the ground grows into: down for a floor patch, up for a ceiling patch.
    inwards: Dir,
    depth: IntProvider,
    extra_bottom_block_chance: f32,
    vertical_range: i32,
    vegetation_chance: f32,
    xz_radius: IntProvider,
    extra_edge_column_chance: f32,
    replaceable: IdSet,
    ground: StateProvider,
    vegetation: Arc<Placed>,
    water: BlockState,
}

impl PatchFeature {
    pub fn compile(
        config: &VegetationPatchConfig,
        waterlogged: bool,
        facts: &BlockFacts,
        placed: &mut dyn FnMut(&Holder<PlacedFeature>) -> Result<Arc<Placed>>,
    ) -> Result<Self> {
        Ok(Self {
            waterlogged,
            inwards: match config.surface {
                CaveSurface::Floor => Dir::Down,
                CaveSurface::Ceiling => Dir::Up,
            },
            depth: config.depth.clone(),
            extra_bottom_block_chance: config.extra_bottom_block_chance,
            vertical_range: config.vertical_range,
            vegetation_chance: config.vegetation_chance,
            xz_radius: config.xz_radius.clone(),
            extra_edge_column_chance: config.extra_edge_column_chance,
            replaceable: IdSet::compile(&config.replaceable).context("replaceable")?,
            ground: StateProvider::compile(&config.ground_state, facts).context("ground_state")?,
            vegetation: placed(&config.vegetation_feature).context("vegetation_feature")?,
            water: facts.complete(&BlockState::new(ResourceLocation::minecraft("water"))),
        })
    }

    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let x_radius = sample_int(&self.xz_radius, &mut *cx.random).wrapping_add(1);
        let z_radius = sample_int(&self.xz_radius, &mut *cx.random).wrapping_add(1);
        let mut ground = GroundCache::default();
        let mut surface = self.ground_patch(cx, origin, x_radius, z_radius, &mut ground);
        if self.waterlogged {
            surface = self.flood(cx, &surface);
        }
        for pos in surface.iter_java_order() {
            if self.vegetation_chance > 0.0 && cx.random.next_float() < self.vegetation_chance {
                self.place_vegetation(cx, pos);
            }
        }
        !surface.is_empty()
    }

    /// `WaterloggedVegetationPatchFeature.distributeVegetation`'s first half: water replaces
    /// every ground block that has no exposed side or underside, and only those stay the surface.
    fn flood(&self, cx: &mut FeatureContext<'_, '_, '_>, surface: &PosSet) -> PosSet {
        let mut sealed = PosSet::default();
        for pos in surface.iter_java_order() {
            let exposed = [Dir::North, Dir::East, Dir::South, Dir::West, Dir::Down]
                .into_iter()
                .any(|dir| {
                    let neighbour = relative(pos, dir, 1);
                    !face_full(cx, neighbour, dir.opposite(), Shape::Support)
                });
            if !exposed {
                sealed.insert(pos);
            }
        }
        let mut water = None;
        for pos in sealed.iter_java_order() {
            // Interned on the first write that lands, so the region's state table fills in the
            // same order `set_block_ref` would fill it.
            if cx.region.contains(pos) {
                let id = *water.get_or_insert_with(|| cx.region.intern(&self.water));
                cx.region.set_block_id(pos, id);
            }
        }
        sealed
    }

    fn place_vegetation(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos) -> bool {
        let outwards = self.inwards.opposite();
        if !self.waterlogged {
            return place_nested(cx, &self.vegetation, relative(pos, outwards, 1));
        }
        // `WaterloggedVegetationPatchFeature` hands the superclass the position below.
        let below = offset(pos, 0, -1, 0);
        if !place_nested(cx, &self.vegetation, relative(below, outwards, 1)) {
            return false;
        }
        let placed = block_in(cx.region, pos);
        if placed
            .properties
            .get("waterlogged")
            .is_some_and(|value| value == "false")
        {
            let mut flooded = placed.clone();
            flooded
                .properties
                .insert("waterlogged".into(), "true".into());
            cx.region.set_block(pos, flooded);
        }
        true
    }

    fn ground_patch(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        origin: BlockPos,
        x_radius: i32,
        z_radius: i32,
        ground: &mut GroundCache,
    ) -> PosSet {
        let inwards = self.inwards;
        let outwards = inwards.opposite();
        let mut surface = PosSet::default();
        let mut dx = x_radius.wrapping_neg();
        while dx <= x_radius {
            let x_edge = dx == x_radius.wrapping_neg() || dx == x_radius;
            let mut dz = z_radius.wrapping_neg();
            while dz <= z_radius {
                let z_edge = dz == z_radius.wrapping_neg() || dz == z_radius;
                let edge = x_edge || z_edge;
                let corner = x_edge && z_edge;
                let edge_not_corner = edge && !corner;
                if !corner
                    && (!edge_not_corner
                        || (self.extra_edge_column_chance != 0.0
                            && cx.random.next_float() <= self.extra_edge_column_chance))
                {
                    // Vanilla asks `isAir` of the position each loop ends on once more in the
                    // next test; the answer is pure, so each position is read once.
                    let mut pos = offset(origin, dx, 0, dz);
                    let mut air = is_air(cx, pos);
                    let mut walked = 0;
                    while air && walked < self.vertical_range {
                        pos = relative(pos, inwards, 1);
                        walked += 1;
                        air = is_air(cx, pos);
                    }
                    let mut walked = 0;
                    while !air && walked < self.vertical_range {
                        pos = relative(pos, outwards, 1);
                        walked += 1;
                        air = is_air(cx, pos);
                    }
                    let below = relative(pos, inwards, 1);
                    if air && face_full(cx, below, outwards, Shape::Support) {
                        let extra = self.extra_bottom_block_chance > 0.0
                            && cx.random.next_float() < self.extra_bottom_block_chance;
                        let depth =
                            sample_int(&self.depth, &mut *cx.random).wrapping_add(i32::from(extra));
                        if self.place_ground(cx, below, depth, ground) {
                            surface.insert(below);
                        }
                    }
                }
                dz += 1;
            }
            dx += 1;
        }
        surface
    }

    /// `VegetationPatchFeature.placeGround`: stacks up to `depth` ground blocks inwards from
    /// `below`, stopping at the first block that may not be replaced.
    fn place_ground(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        mut below: BlockPos,
        depth: i32,
        ground: &mut GroundCache,
    ) -> bool {
        if let StateProvider::Simple(state) = &self.ground {
            return self.place_simple_ground(cx, below, depth, state, ground);
        }
        for step in 0..depth {
            let state = match self.ground.state_cow(cx, below) {
                Ok(Some(state)) => state,
                Ok(None) => return false,
                Err(Unsupported(why)) => {
                    cx.unsupported.record("provider", why);
                    return false;
                }
            };
            let existing = block_in(cx.region, below);
            if state.name != existing.name {
                if !self.replaceable.contains_block(cx.env, &existing.name) {
                    return step != 0;
                }
                cx.region.set_block_ref(below, &state);
                below = relative(below, self.inwards, 1);
            }
        }
        true
    }

    /// [`Self::place_ground`] for a constant ground state, which draws nothing: the name and
    /// `replaceable` tests are pure in the existing state, so they are cached by its key, and
    /// the state is interned once instead of cloned and hashed per write.
    fn place_simple_ground(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        mut below: BlockPos,
        depth: i32,
        state: &BlockState,
        ground: &mut GroundCache,
    ) -> bool {
        for step in 0..depth {
            let (same, replaceable) = match cx.region.probe(below) {
                Some(probe) => {
                    let slot = &mut ground.slots[probe.key.slot() & (GROUND_SLOTS - 1)];
                    match *slot {
                        Some((key, same, replaceable)) if key == probe.key => (same, replaceable),
                        _ => {
                            let same = state.name == probe.state.name;
                            let replaceable =
                                self.replaceable.contains_block(cx.env, &probe.state.name);
                            *slot = Some((probe.key, same, replaceable));
                            (same, replaceable)
                        }
                    }
                }
                None => {
                    let existing = block_in(cx.region, below);
                    (
                        state.name == existing.name,
                        self.replaceable.contains_block(cx.env, &existing.name),
                    )
                }
            };
            if !same {
                if !replaceable {
                    return step != 0;
                }
                // `set_block` writes nothing outside the window; intern only on a write that
                // lands, so the region's state table fills in the same order.
                if cx.region.contains(below) {
                    let id = *ground.id.get_or_insert_with(|| cx.region.intern(state));
                    cx.region.set_block_id(below, id);
                }
                below = relative(below, self.inwards, 1);
            }
        }
        true
    }
}

const GROUND_SLOTS: usize = 8;

/// Per `place` call: the ground state's interned id once written, and per existing state key
/// whether it has the ground's name and whether `replaceable` holds for it.
#[derive(Default)]
struct GroundCache {
    id: Option<u32>,
    slots: [Option<(StateKey, bool, bool)>; GROUND_SLOTS],
}
