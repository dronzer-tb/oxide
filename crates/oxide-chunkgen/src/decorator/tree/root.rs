//! Root placers (`RootPlacer`): the mangrove tree's prop roots.
//!
//! The roots are simulated first, with the same random draws as placing them, and only written
//! if the whole simulation succeeds; a root that runs out of length or width aborts the tree
//! before anything is placed. Roots that land in mud become muddy mangrove roots.

use anyhow::Result;
use oxide_core::BlockPos;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::tree::RootPlacer;
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::predicate::IdSet;
use super::super::state_provider::StateProvider;
use super::{dist_manhattan, off, rel, with_property, Dir, Layer, Res, Run};

#[derive(Debug)]
pub(super) struct Root {
    pub trunk_offset_y: IntProvider,
    root: StateProvider,
    above: Option<(StateProvider, f32)>,
    can_grow_through: IdSet,
    muddy_roots_in: IdSet,
    muddy_roots: StateProvider,
    max_root_width: i32,
    max_root_length: i32,
    random_skew_chance: f32,
}

impl Root {
    pub fn compile(ir: &RootPlacer, facts: &BlockFacts) -> Result<Self> {
        let RootPlacer::Mangrove {
            trunk_offset_y,
            root_provider,
            above_root_placement,
            mangrove_root_placement: placement,
        } = ir;
        Ok(Self {
            trunk_offset_y: trunk_offset_y.clone(),
            root: StateProvider::compile(root_provider, facts)?,
            above: above_root_placement
                .as_ref()
                .map(|above| {
                    Ok::<_, anyhow::Error>((
                        StateProvider::compile(&above.above_root_provider, facts)?,
                        above.above_root_placement_chance,
                    ))
                })
                .transpose()?,
            can_grow_through: IdSet::compile(&placement.can_grow_through)?,
            muddy_roots_in: IdSet::compile(&placement.muddy_roots_in)?,
            muddy_roots: StateProvider::compile(&placement.muddy_roots_provider, facts)?,
            max_root_width: placement.max_root_width,
            max_root_length: placement.max_root_length,
            random_skew_chance: placement.random_skew_chance,
        })
    }

    fn can_place_root(&self, run: &Run<'_, '_, '_, '_>, p: BlockPos) -> bool {
        run.valid_tree_pos(p)
            || self
                .can_grow_through
                .contains_block(run.cx.env, &run.state(p).name)
    }

    /// `RootPlacer.getStateWithWaterlogging`: a root placed in water keeps it.
    fn waterlogged(
        run: &Run<'_, '_, '_, '_>,
        p: BlockPos,
        state: oxide_core::BlockState,
    ) -> oxide_core::BlockState {
        if state.properties.contains_key("waterlogged") {
            let water = run.fact(p, BlockFact::Water);
            with_property(state, "waterlogged", if water { "true" } else { "false" })
        } else {
            state
        }
    }

    /// `MangroveRootPlacer.placeRoot`.
    fn place_root(&self, run: &mut Run<'_, '_, '_, '_>, p: BlockPos) -> Res<()> {
        if self
            .muddy_roots_in
            .contains_block(run.cx.env, &run.state(p).name)
        {
            let state = run.sample(&self.muddy_roots, p)?;
            let state = Self::waterlogged(run, p, state);
            run.set(Layer::Root, p, state);
            return Ok(());
        }
        if !self.can_place_root(run, p) {
            return Ok(());
        }
        let state = run.sample(&self.root, p)?;
        let state = Self::waterlogged(run, p, state);
        run.set(Layer::Root, p, state);
        if let Some((provider, chance)) = &self.above {
            let above = off(p, 0, 1, 0);
            if run.float() < *chance && run.is_air(above) {
                let state = run.sample(provider, above)?;
                let state = Self::waterlogged(run, above, state);
                run.set(Layer::Root, above, state);
            }
        }
        Ok(())
    }

    /// `MangroveRootPlacer.placeRoots`: false when the column under the trunk or any root
    /// simulation is blocked.
    pub fn place_roots(
        &self,
        run: &mut Run<'_, '_, '_, '_>,
        origin: BlockPos,
        trunk_origin: BlockPos,
    ) -> Res<bool> {
        let mut column = origin;
        while column.y < trunk_origin.y {
            if !self.can_place_root(run, column) {
                return Ok(false);
            }
            column = off(column, 0, 1, 0);
        }
        let mut roots = vec![off(trunk_origin, 0, -1, 0)];
        for dir in Dir::HORIZONTAL {
            let start = rel(trunk_origin, dir, 1);
            let mut positions = Vec::new();
            if !self.simulate(run, start, dir, trunk_origin, &mut positions, 0) {
                return Ok(false);
            }
            roots.extend(positions);
            roots.push(start);
        }
        for root in roots {
            self.place_root(run, root)?;
        }
        Ok(true)
    }

    /// `MangroveRootPlacer.simulateRoots`.
    fn simulate(
        &self,
        run: &mut Run<'_, '_, '_, '_>,
        pos: BlockPos,
        dir: Dir,
        origin: BlockPos,
        positions: &mut Vec<BlockPos>,
        layer: i32,
    ) -> bool {
        if layer == self.max_root_length || positions.len() as i64 > i64::from(self.max_root_length)
        {
            return false;
        }
        for next in self.potential_positions(run, pos, dir, origin) {
            if self.can_place_root(run, next) {
                positions.push(next);
                if !self.simulate(run, next, dir, origin, positions, layer + 1) {
                    return false;
                }
            }
        }
        true
    }

    /// `MangroveRootPlacer.potentialRootPositions`. The branch order decides which draws happen.
    #[allow(clippy::if_same_then_else)]
    fn potential_positions(
        &self,
        run: &mut Run<'_, '_, '_, '_>,
        pos: BlockPos,
        dir: Dir,
        origin: BlockPos,
    ) -> Vec<BlockPos> {
        let below = off(pos, 0, -1, 0);
        let next_to = rel(pos, dir, 1);
        let width = dist_manhattan(pos, origin);
        let max = self.max_root_width;
        if width > max - 3 && width <= max {
            if run.float() < self.random_skew_chance {
                vec![below, off(next_to, 0, -1, 0)]
            } else {
                vec![below]
            }
        } else if width > max {
            vec![below]
        } else if run.float() < self.random_skew_chance {
            vec![below]
        } else if run.boolean() {
            vec![next_to]
        } else {
            vec![below]
        }
    }
}
