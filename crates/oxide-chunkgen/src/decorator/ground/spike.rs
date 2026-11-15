//! `spike`: an ice spike, a cone that may be tall and a root column that digs back into the
//! ground.
//!
//! The origin first sinks through air to a surface the config accepts. The spike is then a stack
//! of layers whose radius shrinks to a point; layers above the base are mirrored below it when the
//! base is wide. Finally up to nine thin roots run down from the base, skipping through
//! replaceable material in random-length stretches until they meet something that is neither
//! replaceable nor the spike's own block.

use anyhow::{Context, Result};
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::ground_feature::SpikeConfig;
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::context::{block_in, FeatureContext};
use super::super::predicate::Predicate;
use super::super::state_provider::state_from_spec;
use super::{is_air, offset};

#[derive(Debug)]
pub struct SpikeFeature {
    state: BlockState,
    can_place_on: Predicate,
    can_replace: Predicate,
}

impl SpikeFeature {
    pub fn compile(config: &SpikeConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            state: state_from_spec(&config.state, facts).context("spike state")?,
            can_place_on: Predicate::compile(&config.can_place_on, facts)
                .context("spike can_place_on")?,
            can_replace: Predicate::compile(&config.can_replace, facts)
                .context("spike can_replace")?,
        })
    }

    /// `IceSpikeFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let min_y = cx.region.min_y();
        let mut origin = cx.origin;
        while is_air(cx, origin) && origin.y > min_y + 2 {
            origin = offset(origin, 0, -1, 0);
        }
        if !self.can_place_on.test(cx, origin) {
            return false;
        }
        origin = offset(origin, 0, cx.random.next_int_bounded(4), 0);
        let height = cx.random.next_int_bounded(4) + 7;
        let base = height / 4 + cx.random.next_int_bounded(2);
        if base > 1 && cx.random.next_int_bounded(60) == 0 {
            origin = offset(origin, 0, 10 + cx.random.next_int_bounded(30), 0);
        }
        for layer in 0..height {
            let radius = (1.0 - layer as f32 / height as f32) * base as f32;
            let ceil = radius.ceil() as i32;
            for x in -ceil..=ceil {
                let fx = x.abs() as f32 - 0.25;
                for z in -ceil..=ceil {
                    let fz = z.abs() as f32 - 0.25;
                    if (x != 0 || z != 0) && fx * fx + fz * fz > radius * radius {
                        continue;
                    }
                    if (x == -ceil || x == ceil || z == -ceil || z == ceil)
                        && cx.random.next_float() > 0.75
                    {
                        continue;
                    }
                    self.place_part(cx, offset(origin, x, layer, z));
                    if layer != 0 && ceil > 1 {
                        self.place_part(cx, offset(origin, x, -layer, z));
                    }
                }
            }
        }
        let roots = (base - 1).clamp(0, 1);
        for x in -roots..=roots {
            for z in -roots..=roots {
                let mut pos = offset(origin, x, -1, z);
                let mut remaining = 50;
                if x.abs() == 1 && z.abs() == 1 {
                    remaining = cx.random.next_int_bounded(5);
                }
                while pos.y > 50 {
                    let here = block_in(cx.region, pos);
                    let air = cx.env.facts.is(here, BlockFact::Air);
                    let own = *here == self.state;
                    if !air && !self.can_replace.test(cx, pos) && !own {
                        break;
                    }
                    cx.region.set_block_ref(pos, &self.state);
                    pos = BlockPos::new(pos.x, pos.y - 1, pos.z);
                    remaining -= 1;
                    if remaining <= 0 {
                        pos = BlockPos::new(
                            pos.x,
                            pos.y - (cx.random.next_int_bounded(5) + 1),
                            pos.z,
                        );
                        remaining = cx.random.next_int_bounded(5);
                    }
                }
            }
        }
        true
    }

    /// Writes the spike block at `pos` when it is air or replaceable.
    fn place_part(&self, cx: &mut FeatureContext<'_, '_, '_>, pos: BlockPos) {
        if is_air(cx, pos) || self.can_replace.test(cx, pos) {
            cx.region.set_block_ref(pos, &self.state);
        }
    }
}
