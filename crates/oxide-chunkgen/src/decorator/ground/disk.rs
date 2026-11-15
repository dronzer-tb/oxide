//! `disk`: a circle of replacements, a few blocks deep, such as the sand and clay patches.
//!
//! Every column within the radius is walked from `half_height` above the origin down to
//! `half_height + 1` below it; each block the `target` predicate accepts is replaced by the
//! state provider's pick. The provider sees the world as the walk has left it, which is how the
//! rule-based grass disk reads the block above a position it has just converted.

use anyhow::{Context, Result};
use oxide_core::BlockPos;
use oxide_datapack::feature::DiskConfig;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::super::predicate::Predicate;
use super::super::provider::sample_int;
use super::super::state_provider::{StateProvider, Unsupported};

#[derive(Debug)]
pub struct DiskFeature {
    provider: StateProvider,
    target: Predicate,
    radius: IntProvider,
    half_height: i32,
}

impl DiskFeature {
    pub fn compile(config: &DiskConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            provider: StateProvider::compile(&config.state_provider, facts)
                .context("disk state provider")?,
            target: Predicate::compile(&config.target, facts).context("disk target")?,
            radius: config.radius.clone(),
            half_height: config.half_height,
        })
    }

    /// `DiskFeature.place`. The radius is drawn first; no other draw is its own.
    ///
    /// Vanilla also marks the two blocks above the first replaced block of each run for
    /// post-processing; that is not recorded here (see the module doc of `ground`).
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let top = origin.y + self.half_height;
        let bottom = origin.y - self.half_height - 1;
        let radius = sample_int(&self.radius, cx.random);
        let (x0, x1) = (origin.x - radius, origin.x + radius);
        let (z0, z1) = (origin.z - radius, origin.z + radius);
        let (min_x, min_z) = (x0.min(x1), z0.min(z1));
        let width = x0.max(x1) - min_x + 1;
        let depth = z0.max(z1) - min_z + 1;
        let mut placed = false;
        for index in 0..width * depth {
            let x = min_x + index % width;
            let z = min_z + index / width;
            let (dx, dz) = (x - origin.x, z - origin.z);
            if dx * dx + dz * dz > radius * radius {
                continue;
            }
            let mut y = top;
            while y > bottom {
                let pos = BlockPos::new(x, y, z);
                if self.target.test(cx, pos) {
                    match self.provider.state(cx, pos) {
                        Ok(Some(state)) => {
                            cx.region.set_block(pos, state);
                            placed = true;
                        }
                        Ok(None) => {}
                        Err(Unsupported(why)) => {
                            cx.unsupported.record("provider", why);
                            return placed;
                        }
                    }
                }
                y -= 1;
            }
        }
        placed
    }
}
