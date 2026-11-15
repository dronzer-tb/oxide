//! `lake`: a pool of fluid in a shell of barrier blocks, carved into the ground.
//!
//! A lake is drawn in a 16 x 16 x 8 grid whose origin is the placement point moved by (-8, -4,
//! -8): four to seven random ellipsoids mark the cells that become the pool. The lower four
//! layers take the fluid, the upper four air, and every cell touching the pool from outside is
//! its edge. The edge is checked before anything is written -- a lake that would breach the
//! surface, spill into a hole or fail `can_place_feature` is abandoned whole -- then the shell is
//! laid, and a water lake under a cold biome gets a skin of ice.
//!
//! Vanilla's scheduled tick on the new air is not recorded (see the module doc of `ground`).
//!
//! PARITY-CHECK: the ice pass tests the layer at the placement height, which the pool itself
//! leaves as air, so it only ever freezes open water that already stood there beside the lake.
//! That is what the reference implementation does; confirm against a real 26.2 lake in a snowy
//! biome that this is the whole of vanilla's freezing.

use anyhow::{Context, Result};
use oxide_core::{BlockState, RandomSource};
use oxide_datapack::ground_feature::LakeConfig;
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::context::{block_in, FeatureContext};
use super::super::predicate::Predicate;
use super::super::state_provider::StateProvider;
use super::climate::should_freeze;
use super::{default_state, offset, provide};

#[derive(Debug)]
pub struct LakeFeature {
    fluid: StateProvider,
    barrier: StateProvider,
    can_place_feature: Predicate,
    can_replace_with_air_or_fluid: Predicate,
    can_replace_with_barrier: Predicate,
    cave_air: BlockState,
    ice: BlockState,
}

impl LakeFeature {
    pub fn compile(config: &LakeConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            fluid: StateProvider::compile(&config.fluid, facts).context("lake fluid")?,
            barrier: StateProvider::compile(&config.barrier, facts).context("lake barrier")?,
            can_place_feature: Predicate::compile(&config.can_place_feature, facts)
                .context("lake can_place_feature")?,
            can_replace_with_air_or_fluid: Predicate::compile(
                &config.can_replace_with_air_or_fluid,
                facts,
            )
            .context("lake can_replace_with_air_or_fluid")?,
            can_replace_with_barrier: Predicate::compile(&config.can_replace_with_barrier, facts)
                .context("lake can_replace_with_barrier")?,
            cave_air: default_state(facts, "cave_air")?,
            ice: default_state(facts, "ice")?,
        })
    }

    /// `LakeFeature.place`.
    ///
    /// The grid is one byte per (x, z) column, bit y set when the cell is inside the pool. The
    /// edge of a column is every cell that is not in the pool but has a pool neighbour in the
    /// column or in the four columns beside it, which is vanilla's test done for the whole column
    /// at once.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        if cx.origin.y <= cx.region.min_y() + 4 {
            return false;
        }
        let origin = offset(cx.origin, -8, -4, -8);
        let mut grid = [0u8; 256];
        let spots = cx.random.next_int_bounded(4) + 4;
        for _ in 0..spots {
            let xr = cx.random.next_double() * 6.0 + 3.0;
            let yr = cx.random.next_double() * 4.0 + 2.0;
            let zr = cx.random.next_double() * 6.0 + 3.0;
            let xp = cx.random.next_double() * (16.0 - xr - 2.0) + 1.0 + xr / 2.0;
            let yp = cx.random.next_double() * (8.0 - yr - 4.0) + 2.0 + yr / 2.0;
            let zp = cx.random.next_double() * (16.0 - zr - 2.0) + 1.0 + zr / 2.0;
            let square = |v: usize, p: f64, r: f64| {
                let d = (v as f64 - p) / (r / 2.0);
                d * d
            };
            let xs: [f64; 16] = std::array::from_fn(|v| square(v, xp, xr));
            let ys: [f64; 8] = std::array::from_fn(|v| square(v, yp, yr));
            let zs: [f64; 16] = std::array::from_fn(|v| square(v, zp, zr));
            for x in 1..15 {
                for z in 1..15 {
                    for y in 1..7 {
                        if xs[x] + ys[y] + zs[z] < 1.0 {
                            grid[x * 16 + z] |= 1 << y;
                        }
                    }
                }
            }
        }
        let edges: [u8; 256] = std::array::from_fn(|index| {
            let (x, z) = (index / 16, index % 16);
            let column = grid[index];
            let mut near = (column << 1) | (column >> 1);
            if x < 15 {
                near |= grid[index + 16];
            }
            if x > 0 {
                near |= grid[index - 16];
            }
            if z < 15 {
                near |= grid[index + 1];
            }
            if z > 0 {
                near |= grid[index - 1];
            }
            !column & near
        });
        let layers = |bits: u8| (0..8).filter(move |y| bits >> y & 1 != 0);

        let Some(fluid) = provide(cx, &self.fluid, origin) else {
            return false;
        };
        for x in 0..16 {
            for z in 0..16 {
                for y in layers(edges[(x * 16 + z) as usize]) {
                    let pos = offset(origin, x, y, z);
                    let state = block_in(cx.region, pos);
                    if y >= 4 && cx.env.facts.is(state, BlockFact::Liquid) {
                        return false;
                    }
                    if y < 4 && !cx.env.facts.is(state, BlockFact::Solid) && *state != fluid {
                        return false;
                    }
                    if !self.can_place_feature.test(cx, pos) {
                        return false;
                    }
                }
            }
        }
        for x in 0..16 {
            for z in 0..16 {
                for y in layers(grid[(x * 16 + z) as usize]) {
                    let pos = offset(origin, x, y, z);
                    if self.can_replace_with_air_or_fluid.test(cx, pos) {
                        let state = if y >= 4 {
                            self.cave_air.clone()
                        } else {
                            fluid.clone()
                        };
                        cx.region.set_block(pos, state);
                    }
                }
            }
        }
        let Some(barrier) = provide(cx, &self.barrier, origin) else {
            return true;
        };
        if !cx.env.facts.is(&barrier, BlockFact::Air) {
            for x in 0..16 {
                for z in 0..16 {
                    for y in layers(edges[(x * 16 + z) as usize]) {
                        if y < 4 || cx.random.next_int_bounded(2) != 0 {
                            let pos = offset(origin, x, y, z);
                            let solid = cx.region.is(pos, BlockFact::Solid);
                            if solid && self.can_replace_with_barrier.test(cx, pos) {
                                cx.region.set_block(pos, barrier.clone());
                            }
                        }
                    }
                }
            }
        }
        if cx.env.facts.is(&fluid, BlockFact::Water) {
            for x in 0..16 {
                for z in 0..16 {
                    let pos = offset(origin, x, 4, z);
                    if should_freeze(cx, pos, pos, false)
                        && self.can_replace_with_air_or_fluid.test(cx, pos)
                    {
                        cx.region.set_block(pos, self.ice.clone());
                    }
                }
            }
        }
        true
    }
}
