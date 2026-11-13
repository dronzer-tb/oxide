//! `vines`: one vine block on the first face, in vanilla's order, that has something to cling to.

use anyhow::Result;
use oxide_core::{BlockState, ResourceLocation};
use oxide_datapack::BlockFacts;

use super::geometry::Dir;
use super::support::{can_attach_to, is_air};
use crate::decorator::context::FeatureContext;

/// The faces tried, in the order `VinesFeature` tries them; a vine never hangs from below.
const FACES: [Dir; 5] = [Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East];

#[derive(Debug)]
pub struct VinesFeature {
    vine: BlockState,
}

impl VinesFeature {
    pub fn compile(facts: &BlockFacts) -> Result<Self> {
        Ok(Self { vine: facts.complete(&BlockState::new(ResourceLocation::minecraft("vine"))) })
    }

    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = cx.origin;
        if !is_air(cx, origin) {
            return false;
        }
        for face in FACES {
            if can_attach_to(cx, origin, face) {
                let mut state = self.vine.clone();
                state.properties.insert(face.name().into(), "true".into());
                return cx.region.set_block(origin, state);
            }
        }
        false
    }
}
