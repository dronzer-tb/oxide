//! `block_column`: runs of blocks laid in one direction, such as sugar cane, cactus, cave vines
//! and dripleaf.
//!
//! Every layer's height is drawn first. The column is then tested block by block against the
//! allowed-placement predicate; at the first block that fails, the surplus is cut from the layers
//! (see [`truncate`]) and only what fits is written.

use anyhow::{bail, Context, Result};
use oxide_core::BlockPos;
use oxide_datapack::ground_feature::BlockColumnConfig;
use oxide_datapack::placement::IntProvider;
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::super::predicate::Predicate;
use super::super::provider::sample_int;
use super::super::state_provider::StateProvider;
use super::provide;

#[derive(Debug)]
struct Layer {
    height: IntProvider,
    provider: StateProvider,
}

#[derive(Debug)]
pub struct BlockColumnFeature {
    layers: Vec<Layer>,
    step: [i32; 3],
    allowed_placement: Predicate,
    prioritize_tip: bool,
}

impl BlockColumnFeature {
    pub fn compile(config: &BlockColumnConfig, facts: &BlockFacts) -> Result<Self> {
        let layers = config
            .layers
            .iter()
            .map(|layer| {
                Ok(Layer {
                    height: layer.height.clone(),
                    provider: StateProvider::compile(&layer.provider, facts)?,
                })
            })
            .collect::<Result<Vec<_>>>()
            .context("block column layers")?;
        let step = match config.direction.as_str() {
            "down" => [0, -1, 0],
            "up" => [0, 1, 0],
            "north" => [0, 0, -1],
            "south" => [0, 0, 1],
            "west" => [-1, 0, 0],
            "east" => [1, 0, 0],
            other => bail!("unknown block column direction {other:?}"),
        };
        Ok(Self {
            layers,
            step,
            allowed_placement: Predicate::compile(&config.allowed_placement, facts)
                .context("block column allowed_placement")?,
            prioritize_tip: config.prioritize_tip,
        })
    }

    /// `BlockColumnFeature.place`.
    ///
    /// PARITY-CHECK: the allowed-placement test starts one block past the origin (the origin
    /// itself is where the first block goes), as in the reference implementation; confirm against
    /// a real 26.2 sugar cane or cave vine placed beside an obstacle.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        let mut heights: Vec<i32> = Vec::with_capacity(self.layers.len());
        let mut total = 0i32;
        for layer in &self.layers {
            let height = sample_int(&layer.height, cx.random);
            total = total.wrapping_add(height);
            heights.push(height);
        }
        if total == 0 {
            return false;
        }
        let advance = |pos: BlockPos| {
            BlockPos::new(
                pos.x + self.step[0],
                pos.y + self.step[1],
                pos.z + self.step[2],
            )
        };
        let mut query = advance(origin);
        for index in 0..total {
            if !self.allowed_placement.test(cx, query) {
                truncate(&mut heights, total, index, self.prioritize_tip);
                break;
            }
            query = advance(query);
        }
        let mut pos = origin;
        for (layer, height) in self.layers.iter().zip(heights) {
            for _ in 0..height {
                let Some(state) = provide(cx, &layer.provider, pos) else {
                    return false;
                };
                cx.region.set_block(pos, state);
                pos = advance(pos);
            }
        }
        true
    }
}

/// `BlockColumnFeature.truncate`: removes `total - allowed` blocks from the layers, starting
/// from the first layer when the tip is prioritised and from the last otherwise.
fn truncate(heights: &mut [i32], total: i32, allowed: i32, prioritize_tip: bool) {
    let mut remove = total - allowed;
    for offset in 0..heights.len() {
        if remove <= 0 {
            break;
        }
        let index = if prioritize_tip {
            offset
        } else {
            heights.len() - 1 - offset
        };
        let count = heights[index].min(remove);
        remove -= count;
        heights[index] -= count;
    }
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncation_cuts_from_the_first_layer_when_the_tip_is_prioritised_else_from_the_last() {
        let mut heights = [3, 2, 1];
        truncate(&mut heights, 6, 3, true);
        assert_eq!(heights, [0, 2, 1]);
        let mut heights = [3, 2, 1];
        truncate(&mut heights, 6, 3, false);
        assert_eq!(heights, [3, 0, 0]);
        let mut heights = [3, 2, 1];
        truncate(&mut heights, 6, 0, false);
        assert_eq!(heights, [0, 0, 0]);
    }
}
