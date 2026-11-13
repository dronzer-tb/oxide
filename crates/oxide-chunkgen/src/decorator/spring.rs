//! `spring_feature`: a single fluid source set into a rock wall with exactly the right cavity.
//!
//! A spring needs `rock_count` of its five neighbours (the four sides and the one below) to be
//! valid rock and `hole_count` of them to be air; the stock springs want four rocks and one hole,
//! which is a one-block-wide gap in a cave wall. Ported from vanilla's `SpringFeature`.
//!
//! PARITY-CHECK: vanilla also schedules a fluid tick at the new block so the spring starts
//! flowing. A region holds block writes only and has no tick queue, so the tick is not recorded;
//! the source block is written, which is the part the chunk stores.
//!
//! PARITY-CHECK: the block written is the liquid block at `level` 0, or 8 when the configured
//! fluid is falling (`FlowingFluid.createLegacyBlock`). The reference implementation has no
//! equivalent to diff; confirm the water and lava springs against a real 26.2 world.

use anyhow::{bail, Result};
use oxide_core::{BlockPos, BlockState, ResourceLocation};
use oxide_datapack::feature::SpringConfig;
use oxide_datapack::{BlockFact, BlockFacts};

use super::context::{block_in, FeatureContext};
use super::predicate::IdSet;

#[derive(Debug, Clone)]
pub struct SpringFeature {
    state: BlockState,
    requires_block_below: bool,
    rock_count: i32,
    hole_count: i32,
    valid_blocks: IdSet,
}

impl SpringFeature {
    /// `Ok(None)` for a fluid this port cannot turn into a block (anything but a source water or
    /// lava), so the caller can record it as unsupported rather than fail the whole datapack.
    pub fn compile(config: &SpringConfig, facts: &BlockFacts) -> Result<Option<Self>> {
        let name = config.state.name.as_str();
        let block = match name {
            "minecraft:water" => "water",
            "minecraft:lava" => "lava",
            _ => return Ok(None),
        };
        let falling = config.state.properties.get("falling").is_some_and(|v| v == "true");
        // `FlowingFluid.createLegacyBlock`: the liquid block's `level` is 0 for a still source,
        // plus 8 when the fluid is falling.
        let mut state = BlockState::new(ResourceLocation::minecraft(block));
        state.properties.insert("level".into(), if falling { "8" } else { "0" }.into());
        let state = facts.complete(&state);
        if !facts.knows(&state) {
            bail!("spring block state {state} is not in the exported block facts");
        }
        Ok(Some(Self {
            state,
            requires_block_below: config.requires_block_below,
            rock_count: config.rock_count,
            hole_count: config.hole_count,
            valid_blocks: IdSet::compile(&config.valid_blocks)?,
        }))
    }

    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let pos = cx.origin;
        let at = |dx: i32, dy: i32, dz: i32| {
            BlockPos::new(pos.x.wrapping_add(dx), pos.y.wrapping_add(dy), pos.z.wrapping_add(dz))
        };
        let valid = |cx: &FeatureContext<'_, '_, '_>, p: BlockPos| {
            self.valid_blocks.contains_block(cx.env, &block_in(cx.region, p).name)
        };
        if !valid(cx, at(0, 1, 0)) {
            return false;
        }
        if self.requires_block_below && !valid(cx, at(0, -1, 0)) {
            return false;
        }
        let here = block_in(cx.region, pos);
        if !cx.env.facts.is(here, BlockFact::Air) && !self.valid_blocks.contains_block(cx.env, &here.name)
        {
            return false;
        }
        let neighbours = [at(-1, 0, 0), at(1, 0, 0), at(0, 0, -1), at(0, 0, 1), at(0, -1, 0)];
        let rocks = neighbours.iter().filter(|p| valid(cx, **p)).count() as i32;
        let holes = neighbours
            .iter()
            .filter(|p| cx.env.facts.is(block_in(cx.region, **p), BlockFact::Air))
            .count() as i32;
        if rocks != self.rock_count || holes != self.hole_count {
            return false;
        }
        cx.region.set_block(pos, self.state.clone())
    }
}
