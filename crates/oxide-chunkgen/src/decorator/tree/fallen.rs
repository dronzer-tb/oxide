//! `minecraft:fallen_tree`: a stump with an optional fallen log lying next to it.
//!
//! The stump is always placed. The log is placed only if its whole run is free and mostly
//! supported (no more than two unsupported blocks in a row), after sliding the start down to the
//! ground. Each part runs its own decorator list over the logs it placed.

use anyhow::Result;
use oxide_core::BlockPos;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::tree::FallenTreeConfig;
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::context::{block_in, FeatureContext};
use super::super::state_provider::{StateProvider, Unsupported};
use super::decorator::{Decorator, Lists};
use super::pos_set::PosSet;
use super::{off, rel, Axis, Dir, FeatureLookup, Layer, Res, Run};

#[derive(Debug)]
pub struct FallenTreeFeature {
    trunk: StateProvider,
    log_length: IntProvider,
    stump_decorators: Vec<Decorator>,
    log_decorators: Vec<Decorator>,
    /// A component this generator cannot complete; the tree then places nothing.
    unsupported: Option<String>,
}

impl FallenTreeFeature {
    pub fn compile(config: &FallenTreeConfig, facts: &BlockFacts, lookup: &mut FeatureLookup<'_>) -> Result<Self> {
        let (stump_decorators, stump_unsupported) = Decorator::compile_list(&config.stump_decorators, facts, lookup)?;
        let (log_decorators, log_unsupported) = Decorator::compile_list(&config.log_decorators, facts, lookup)?;
        Ok(Self {
            trunk: StateProvider::compile(&config.trunk_provider, facts)?,
            log_length: config.log_length.clone(),
            stump_decorators,
            log_decorators,
            unsupported: stump_unsupported.or(log_unsupported),
        })
    }

    /// `FallenTreeFeature.place`. Always reports success once it starts, as vanilla does.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        if let Some(component) = &self.unsupported {
            cx.unsupported.record("tree", component);
            return false;
        }
        let origin = cx.origin;
        let mut run = Run::new(cx, false);
        match self.place_fallen(&mut run, origin) {
            Ok(()) => true,
            Err(Unsupported(why)) => {
                run.cx.unsupported.record("provider", why);
                false
            }
        }
    }

    fn place_fallen(&self, run: &mut Run<'_, '_, '_, '_>, origin: BlockPos) -> Res<()> {
        let stump = self.place_log(run, origin, None)?;
        decorate(run, &[stump], &self.stump_decorators)?;
        let dir = run.horizontal();
        let length = run.int(&self.log_length) - 2;
        let mut start = rel(origin, dir, 2 + run.bounded(2));
        start = off(start, 0, 1, 0);
        for _ in 0..6 {
            if may_place_on(run, start) {
                break;
            }
            start = off(start, 0, -1, 0);
        }
        if can_place_log(run, length, start, dir) {
            let mut logs = PosSet::default();
            let mut p = start;
            for _ in 0..length {
                logs.insert(self.place_log(run, p, Some(dir.axis()))?);
                p = rel(p, dir, 1);
            }
            decorate(run, &logs.to_vec(), &self.log_decorators)?;
        }
        Ok(())
    }

    fn place_log(&self, run: &mut Run<'_, '_, '_, '_>, p: BlockPos, axis: Option<Axis>) -> Res<BlockPos> {
        let mut state = run.sample(&self.trunk, p)?;
        if let Some(axis) = axis {
            state = run.with_axis(state, axis);
        }
        run.set(Layer::Trunk, p, state);
        Ok(p)
    }
}

fn decorate(run: &mut Run<'_, '_, '_, '_>, logs: &[BlockPos], decorators: &[Decorator]) -> Res<()> {
    if decorators.is_empty() {
        return Ok(());
    }
    let lists = Lists::new(logs.to_vec(), Vec::new(), Vec::new());
    for decorator in decorators {
        decorator.place(run, &lists)?;
    }
    Ok(())
}

/// `FallenTreeFeature.isOverSolidGround`: the block below has a sturdy top face.
///
/// PARITY-CHECK: `SturdyTop` is taken to be `isFaceSturdy(level, pos, Direction.UP)` with the
/// full-face support type; confirm against a fallen log over a slab or a leaf block.
fn over_solid_ground(run: &Run<'_, '_, '_, '_>, p: BlockPos) -> bool {
    let below = block_in(run.cx.region, off(p, 0, -1, 0));
    run.cx.env.facts.is(below, BlockFact::SturdyTop)
}

fn may_place_on(run: &Run<'_, '_, '_, '_>, p: BlockPos) -> bool {
    run.valid_tree_pos(p) && over_solid_ground(run, p)
}

fn can_place_log(run: &Run<'_, '_, '_, '_>, length: i32, start: BlockPos, dir: Dir) -> bool {
    let mut gap = 0;
    let mut p = start;
    for _ in 0..length {
        if !run.valid_tree_pos(p) {
            return false;
        }
        if over_solid_ground(run, p) {
            gap = 0;
        } else {
            gap += 1;
            if gap > 2 {
                return false;
            }
        }
        p = rel(p, dir, 1);
    }
    true
}
