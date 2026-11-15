//! `BlockState.canSurvive` for the blocks decoration places.
//!
//! Whether a block can stay where it is placed is per-block Java in vanilla, so the exported
//! data cannot answer it. [`SurvivalRules`] is the [`BlockBehaviours`] that does: each block is
//! mapped to the rule of the class that declares its `canSurvive` (`rules`), and the rule reads
//! the neighbourhood through a `probe`. What vanilla decides with a block tag (`supports_*`,
//! `cannot_support_*`) is decided here with the same tag from [`Environment::block_tags`], not
//! with a list of blocks, so a data pack that edits the tag edits the rule.
//!
//! A block that does not override `canSurvive` always survives. A block whose class does
//! override it but is not ported answers `None`, and a rule that needs a shape the block facts
//! do not carry answers `None` for that neighbour (see `probe`). The caller tallies both; nothing
//! is guessed.
//!
//! Each rule was verified against the real game: `scripts/BlockSurvivalOracle.java` asks
//! `BlockState.canSurvive` for every state of every overriding block over a set of grounds and
//! neighbourhoods, and `tests` replays that output against these rules when it is present.

mod probe;
mod rules;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use oxide_core::{BlockPos, BlockState, ResourceLocation};
use oxide_datapack::TagSet;

use super::context::BlockBehaviours;
use crate::region::DecorationRegion;
use probe::Probe;

/// The block tags a survival rule reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tag {
    CannotSupportKelp,
    CannotSupportSeagrass,
    CannotSupportSnowLayer,
    OverridesMushroomLightRequirement,
    SoulFireBaseBlocks,
    SupportOverrideCactusFlower,
    SupportOverrideSnowLayer,
    SupportsAzalea,
    SupportsBamboo,
    SupportsBigDripleaf,
    SupportsCactus,
    SupportsCocoa,
    SupportsCrimsonFungus,
    SupportsCrimsonRoots,
    SupportsCrops,
    SupportsDryVegetation,
    SupportsHangingMangrovePropagule,
    SupportsLilyPad,
    SupportsMangrovePropagule,
    SupportsMelonStem,
    SupportsNetherSprouts,
    SupportsNetherWart,
    SupportsPumpkinStem,
    SupportsSmallDripleaf,
    SupportsSugarCane,
    SupportsSugarCaneAdjacently,
    SupportsVegetation,
    SupportsWarpedFungus,
    SupportsWarpedRoots,
    SupportsWitherRose,
    UnstableBottomCenter,
}

impl Tag {
    const ALL: [Tag; 31] = [
        Tag::CannotSupportKelp,
        Tag::CannotSupportSeagrass,
        Tag::CannotSupportSnowLayer,
        Tag::OverridesMushroomLightRequirement,
        Tag::SoulFireBaseBlocks,
        Tag::SupportOverrideCactusFlower,
        Tag::SupportOverrideSnowLayer,
        Tag::SupportsAzalea,
        Tag::SupportsBamboo,
        Tag::SupportsBigDripleaf,
        Tag::SupportsCactus,
        Tag::SupportsCocoa,
        Tag::SupportsCrimsonFungus,
        Tag::SupportsCrimsonRoots,
        Tag::SupportsCrops,
        Tag::SupportsDryVegetation,
        Tag::SupportsHangingMangrovePropagule,
        Tag::SupportsLilyPad,
        Tag::SupportsMangrovePropagule,
        Tag::SupportsMelonStem,
        Tag::SupportsNetherSprouts,
        Tag::SupportsNetherWart,
        Tag::SupportsPumpkinStem,
        Tag::SupportsSmallDripleaf,
        Tag::SupportsSugarCane,
        Tag::SupportsSugarCaneAdjacently,
        Tag::SupportsVegetation,
        Tag::SupportsWarpedFungus,
        Tag::SupportsWarpedRoots,
        Tag::SupportsWitherRose,
        Tag::UnstableBottomCenter,
    ];

    /// The tag's id under `minecraft:`.
    fn path(self) -> &'static str {
        match self {
            Tag::CannotSupportKelp => "cannot_support_kelp",
            Tag::CannotSupportSeagrass => "cannot_support_seagrass",
            Tag::CannotSupportSnowLayer => "cannot_support_snow_layer",
            Tag::OverridesMushroomLightRequirement => "overrides_mushroom_light_requirement",
            Tag::SoulFireBaseBlocks => "soul_fire_base_blocks",
            Tag::SupportOverrideCactusFlower => "support_override_cactus_flower",
            Tag::SupportOverrideSnowLayer => "support_override_snow_layer",
            Tag::SupportsAzalea => "supports_azalea",
            Tag::SupportsBamboo => "supports_bamboo",
            Tag::SupportsBigDripleaf => "supports_big_dripleaf",
            Tag::SupportsCactus => "supports_cactus",
            Tag::SupportsCocoa => "supports_cocoa",
            Tag::SupportsCrimsonFungus => "supports_crimson_fungus",
            Tag::SupportsCrimsonRoots => "supports_crimson_roots",
            Tag::SupportsCrops => "supports_crops",
            Tag::SupportsDryVegetation => "supports_dry_vegetation",
            Tag::SupportsHangingMangrovePropagule => "supports_hanging_mangrove_propagule",
            Tag::SupportsLilyPad => "supports_lily_pad",
            Tag::SupportsMangrovePropagule => "supports_mangrove_propagule",
            Tag::SupportsMelonStem => "supports_melon_stem",
            Tag::SupportsNetherSprouts => "supports_nether_sprouts",
            Tag::SupportsNetherWart => "supports_nether_wart",
            Tag::SupportsPumpkinStem => "supports_pumpkin_stem",
            Tag::SupportsSmallDripleaf => "supports_small_dripleaf",
            Tag::SupportsSugarCane => "supports_sugar_cane",
            Tag::SupportsSugarCaneAdjacently => "supports_sugar_cane_adjacently",
            Tag::SupportsVegetation => "supports_vegetation",
            Tag::SupportsWarpedFungus => "supports_warped_fungus",
            Tag::SupportsWarpedRoots => "supports_warped_roots",
            Tag::SupportsWitherRose => "supports_wither_rose",
            Tag::UnstableBottomCenter => "unstable_bottom_center",
        }
    }
}

/// The survival rules of vanilla's blocks, backed by the block tags they consult.
pub struct SurvivalRules {
    /// The members of each tag in [`Tag::ALL`] order, copied out of the tag set so a lookup
    /// is one hash probe with no id built.
    members: Vec<HashSet<ResourceLocation>>,
}

impl SurvivalRules {
    /// Reads the tags the rules need from `block_tags`. A tag the data does not define has no
    /// members, which is what vanilla does with an unbound tag.
    pub fn new(block_tags: &TagSet) -> Self {
        let members = Tag::ALL
            .iter()
            .map(|tag| {
                block_tags
                    .get(&ResourceLocation::minecraft(tag.path()))
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        Self { members }
    }

    fn in_tag(&self, tag: Tag, state: &BlockState) -> bool {
        // `Tag::ALL` lists the variants in declaration order, so the discriminant is the index.
        self.members
            .get(tag as usize)
            .is_some_and(|set| set.contains(&state.name))
    }
}

impl BlockBehaviours for SurvivalRules {
    fn can_survive(
        &self,
        state: &BlockState,
        region: &DecorationRegion<'_>,
        pos: BlockPos,
    ) -> Option<bool> {
        if state.name.namespace() != "minecraft" {
            return None;
        }
        let rule = rules::classify(state.name.path());
        rules::evaluate(
            rule,
            &Probe {
                rules: self,
                region,
                state,
                pos,
            },
        )
    }
}
