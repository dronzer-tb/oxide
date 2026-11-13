//! `end_gateway`: a gateway portal block inside a bedrock frame.
//!
//! The 3x5x3 box around the origin is rewritten: the centre is the gateway, the layer through it
//! is air, and above and below it bedrock forms a column (a cross on the middle layers, a single
//! block on the two caps) with air elsewhere.
//!
//! A gateway with an `exit` also has that exit, and whether it teleports exactly, written into
//! its block entity. The region has none, so that case is tallied as
//! `block_entity:minecraft:end_gateway`; the delayed gateway (no exit) needs nothing beyond the
//! block.

use anyhow::Result;
use oxide_core::BlockState;
use oxide_datapack::end_feature::EndGatewayConfig;
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::{default_state, offset};

#[derive(Debug)]
pub struct EndGatewayFeature {
    has_exit: bool,
    air: BlockState,
    bedrock: BlockState,
    gateway: BlockState,
}

impl EndGatewayFeature {
    pub fn compile(config: &EndGatewayConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            has_exit: config.exit.is_some(),
            air: default_state(facts, "air")?,
            bedrock: default_state(facts, "bedrock")?,
            gateway: default_state(facts, "end_gateway")?,
        })
    }

    /// `EndGatewayFeature.place`, over `BlockPos.betweenClosed` order: x fastest, then y, then z.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        for dz in -1..=1 {
            for dy in -2..=2 {
                for dx in -1..=1 {
                    let pos = offset(origin, dx, dy, dz);
                    let (x, y, z) = (dx == 0, dy == 0, dz == 0);
                    let cap = dy.abs() == 2;
                    let state = if x && y && z {
                        &self.gateway
                    } else if y {
                        &self.air
                    } else if (cap && x && z) || (!cap && (x || z)) {
                        &self.bedrock
                    } else {
                        &self.air
                    };
                    cx.region.set_block(pos, state.clone());
                    if x && y && z && self.has_exit {
                        cx.unsupported.record("block_entity", "minecraft:end_gateway");
                    }
                }
            }
        }
        true
    }
}
