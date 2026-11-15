//! The survival rules themselves, one per declaring class of `canSurvive`, and which block
//! uses which.
//!
//! [`classify`] maps a block to its [`Rule`]. A block that does not override `canSurvive` is
//! [`Rule::Always`]; one that does but whose rule is not ported is [`Rule::Unported`], which
//! answers "unknown" so the gap is tallied. The class of each block and the complete set of
//! overriding blocks were read from the game with `scripts/BlockSurvivalOracle.java`, and
//! `survival::tests` checks both against that output when it is present.

use oxide_core::BlockState;
use oxide_datapack::BlockFact;

use super::probe::{and, or, Dir, Probe};
use super::Tag;

/// Vanilla evaluates `LevelReader.getRawBrightness` for crops and mushrooms. Decoration runs
/// before the light stage, so every position reads as unlit.
// PARITY-CHECK: confirm WorldGenRegion.getRawBrightness is 0 during the features stage.
const DECORATION_BRIGHTNESS: i32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Rule {
    /// No `canSurvive` override: the default is `true`.
    Always,
    /// Overrides `canSurvive`; the rule is not ported.
    Unported,
    /// `VegetationBlock.canSurvive`: the block below is in the tag.
    Below(Tag),
    Seagrass,
    TallSeagrass,
    LilyPad,
    Mushroom,
    CactusFlower,
    Crop,
    PitcherCrop,
    DoublePlant,
    SmallDripleaf,
    BigDripleaf,
    BigDripleafStem,
    Cactus,
    SugarCane,
    Snow,
    SeaPickle,
    GrowingPlant {
        growth: Dir,
        head: &'static str,
        body: &'static str,
        kelp: bool,
    },
    SporeBlossom,
    HangingRoots,
    HangingMoss,
    Carpet,
    MossyCarpet,
    LeafLitter,
    Fire,
    Amethyst,
    Speleothem,
    MangrovePropagule,
    Cocoa,
    Multiface,
    Vine,
}

/// Name endings of overriding blocks that come in a family per wood, colour or metal.
const UNPORTED_SUFFIXES: [&str; 13] = [
    "_door",
    "_banner",
    "_sign",
    "_pressure_plate",
    "_button",
    "candle",
    "candle_cake",
    "_coral",
    "_coral_fan",
    "_coral_wall_fan",
    "torch",
    "copper_lantern",
    "rail",
];

/// The rule of the block `path` in the `minecraft` namespace.
pub(super) fn classify(path: &str) -> Rule {
    use Rule::*;
    use Tag::*;
    match path {
        "crimson_fungus" => Below(SupportsCrimsonFungus),
        "warped_fungus" => Below(SupportsWarpedFungus),
        "crimson_roots" => Below(SupportsCrimsonRoots),
        "warped_roots" => Below(SupportsWarpedRoots),
        "nether_sprouts" => Below(SupportsNetherSprouts),
        "nether_wart" => Below(SupportsNetherWart),
        "dead_bush" | "short_dry_grass" | "tall_dry_grass" => Below(SupportsDryVegetation),
        "azalea" | "flowering_azalea" => Below(SupportsAzalea),
        "wither_rose" => Below(SupportsWitherRose),
        // PARITY-CHECK: the attached stems' tag is not told apart from the stem's by the oracle
        // (both are farmland only).
        "melon_stem" | "attached_melon_stem" => Below(SupportsMelonStem),
        "pumpkin_stem" | "attached_pumpkin_stem" => Below(SupportsPumpkinStem),
        "bamboo" | "bamboo_sapling" => Below(SupportsBamboo),
        "soul_fire" => Below(SoulFireBaseBlocks),
        "allium" | "azure_bluet" | "blue_orchid" | "bush" | "closed_eyeblossom" | "cornflower"
        | "dandelion" | "fern" | "firefly_bush" | "golden_dandelion" | "lily_of_the_valley"
        | "open_eyeblossom" | "orange_tulip" | "oxeye_daisy" | "pink_petals" | "pink_tulip"
        | "poppy" | "red_tulip" | "short_grass" | "sweet_berry_bush" | "torchflower"
        | "white_tulip" | "wildflowers" => Below(SupportsVegetation),
        _ if path.ends_with("_sapling") && !path.starts_with("potted_") => {
            Below(SupportsVegetation)
        }
        "seagrass" => Seagrass,
        "tall_seagrass" => TallSeagrass,
        "lily_pad" => LilyPad,
        "brown_mushroom" | "red_mushroom" => Mushroom,
        "cactus_flower" => CactusFlower,
        "wheat" | "carrots" | "potatoes" | "beetroots" | "torchflower_crop" => Crop,
        "pitcher_crop" => PitcherCrop,
        "sunflower" | "lilac" | "rose_bush" | "peony" | "tall_grass" | "large_fern"
        | "pitcher_plant" => DoublePlant,
        "small_dripleaf" => SmallDripleaf,
        "big_dripleaf" => BigDripleaf,
        "big_dripleaf_stem" => BigDripleafStem,
        "cactus" => Cactus,
        "sugar_cane" => SugarCane,
        "snow" => Snow,
        "sea_pickle" => SeaPickle,
        "kelp" | "kelp_plant" => GrowingPlant {
            growth: Dir::Up,
            head: "kelp",
            body: "kelp_plant",
            kelp: true,
        },
        "twisting_vines" | "twisting_vines_plant" => GrowingPlant {
            growth: Dir::Up,
            head: "twisting_vines",
            body: "twisting_vines_plant",
            kelp: false,
        },
        "weeping_vines" | "weeping_vines_plant" => GrowingPlant {
            growth: Dir::Down,
            head: "weeping_vines",
            body: "weeping_vines_plant",
            kelp: false,
        },
        "cave_vines" | "cave_vines_plant" => GrowingPlant {
            growth: Dir::Down,
            head: "cave_vines",
            body: "cave_vines_plant",
            kelp: false,
        },
        "spore_blossom" => SporeBlossom,
        "hanging_roots" => HangingRoots,
        "pale_hanging_moss" => HangingMoss,
        "moss_carpet" => Carpet,
        _ if path.ends_with("_carpet") && path != "pale_moss_carpet" => Carpet,
        "pale_moss_carpet" => MossyCarpet,
        "leaf_litter" => LeafLitter,
        "fire" => Fire,
        "amethyst_cluster"
        | "large_amethyst_bud"
        | "medium_amethyst_bud"
        | "small_amethyst_bud" => Amethyst,
        "pointed_dripstone" | "sulfur_spike" => Speleothem,
        "mangrove_propagule" => MangrovePropagule,
        "cocoa" => Cocoa,
        "glow_lichen" | "sculk_vein" | "resin_clump" => Multiface,
        "vine" => Vine,
        "bell" | "cake" | "repeater" | "comparator" | "grindstone" | "ladder" | "lever"
        | "piston_head" | "redstone_wire" | "scaffolding" | "tripwire_hook" | "bubble_column"
        | "chorus_plant" | "chorus_flower" | "frogspawn" | "dirt_path" | "farmland" | "lantern"
        | "soul_lantern" => Unported,
        _ if UNPORTED_SUFFIXES
            .iter()
            .any(|suffix| path.ends_with(suffix))
            && !path.ends_with("_wall_hanging_sign") =>
        {
            Unported
        }
        _ => Always,
    }
}

/// Evaluates `rule` for the probe's block at its position.
pub(super) fn evaluate(rule: Rule, p: &Probe<'_, '_>) -> Option<bool> {
    let below = p.at(Dir::Down);
    match rule {
        Rule::Always => Some(true),
        Rule::Unported => None,
        Rule::Below(tag) => Some(p.tag(tag, below)),
        Rule::Seagrass => {
            Some(p.sturdy(below, Dir::Up)? && !p.tag(Tag::CannotSupportSeagrass, below))
        }
        Rule::TallSeagrass => tall_seagrass(p, below),
        Rule::LilyPad => Some(
            (p.has(below, BlockFact::Water) || p.tag(Tag::SupportsLilyPad, below))
                && !p.has_fluid(p.here()),
        ),
        Rule::Mushroom => Some(
            p.tag(Tag::OverridesMushroomLightRequirement, below)
                || (DECORATION_BRIGHTNESS < 13 && p.has(below, BlockFact::SolidRender)),
        ),
        Rule::CactusFlower => or(
            Some(p.tag(Tag::SupportOverrideCactusFlower, below)),
            p.center(below, Dir::Up),
        ),
        Rule::Crop => Some(DECORATION_BRIGHTNESS >= 8 && p.tag(Tag::SupportsCrops, below)),
        Rule::PitcherCrop => pitcher_crop(p, below),
        Rule::DoublePlant => double_plant(p, below, Tag::SupportsVegetation),
        Rule::SmallDripleaf => small_dripleaf(p, below),
        Rule::BigDripleaf => Some(is_dripleaf(below) || p.tag(Tag::SupportsBigDripleaf, below)),
        Rule::BigDripleafStem => Some(
            (below.name.path() == "big_dripleaf_stem" || p.tag(Tag::SupportsBigDripleaf, below))
                && is_dripleaf(p.at(Dir::Up)),
        ),
        Rule::Cactus => cactus(p, below),
        Rule::SugarCane => sugar_cane(p, below),
        Rule::Snow => snow(p, below),
        Rule::SeaPickle => or(p.sturdy(below, Dir::Up), p.collision_fills(below)),
        Rule::GrowingPlant {
            growth,
            head,
            body,
            kelp,
        } => growing_plant(p, growth, head, body, kelp),
        Rule::SporeBlossom => and(
            p.center(p.at(Dir::Up), Dir::Down),
            Some(!p.has(p.here(), BlockFact::Water)),
        ),
        Rule::HangingRoots => p.sturdy(p.at(Dir::Up), Dir::Down),
        Rule::HangingMoss => {
            let above = p.at(Dir::Up);
            or(p.attachable(above, Dir::Down), Some(p.same_block(above)))
        }
        Rule::Carpet => Some(!p.is_air(below)),
        Rule::MossyCarpet => Some(if Probe::prop_is(p.state, "bottom", "true") {
            !p.is_air(below)
        } else {
            p.same_block(below) && Probe::prop_is(below, "bottom", "true")
        }),
        Rule::LeafLitter => p.sturdy(below, Dir::Up),
        Rule::Fire => fire(p, below),
        Rule::Amethyst => {
            let facing = Dir::from_name(Probe::prop(p.state, "facing")?)?;
            p.sturdy(p.at(facing.opposite()), facing)
        }
        Rule::Speleothem => {
            let direction = Dir::from_name(Probe::prop(p.state, "vertical_direction")?)?;
            let neighbour = p.at(direction.opposite());
            or(
                p.sturdy(neighbour, direction),
                Some(
                    p.same_block(neighbour)
                        && Probe::prop(neighbour, "vertical_direction")
                            == Probe::prop(p.state, "vertical_direction"),
                ),
            )
        }
        Rule::MangrovePropagule => Some(if Probe::prop_is(p.state, "hanging", "true") {
            p.tag(Tag::SupportsHangingMangrovePropagule, p.at(Dir::Up))
        } else {
            p.tag(Tag::SupportsMangrovePropagule, below)
        }),
        Rule::Cocoa => {
            let facing = Dir::from_name(Probe::prop(p.state, "facing")?)?;
            Some(p.tag(Tag::SupportsCocoa, p.at(facing)))
        }
        Rule::Multiface => multiface(p),
        Rule::Vine => vine(p),
    }
}

fn is_dripleaf(state: &BlockState) -> bool {
    matches!(state.name.path(), "big_dripleaf" | "big_dripleaf_stem")
}

/// `DoublePlantBlock.canSurvive`: the lower half sits on `tag`, the upper half on its lower half.
fn double_plant(p: &Probe<'_, '_>, below: &BlockState, tag: Tag) -> Option<bool> {
    Some(if Probe::prop_is(p.state, "half", "upper") {
        p.same_block(below) && Probe::prop_is(below, "half", "lower")
    } else {
        p.tag(tag, below)
    })
}

fn tall_seagrass(p: &Probe<'_, '_>, below: &BlockState) -> Option<bool> {
    if Probe::prop_is(p.state, "half", "upper") {
        return double_plant(p, below, Tag::SupportsVegetation);
    }
    and(
        and(
            p.sturdy(below, Dir::Up),
            Some(!p.tag(Tag::CannotSupportSeagrass, below)),
        ),
        Some(p.is_full_water(p.here())),
    )
}

fn pitcher_crop(p: &Probe<'_, '_>, below: &BlockState) -> Option<bool> {
    if Probe::prop_is(p.state, "half", "upper") {
        return double_plant(p, below, Tag::SupportsCrops);
    }
    Some(DECORATION_BRIGHTNESS >= 8 && p.tag(Tag::SupportsCrops, below))
}

fn small_dripleaf(p: &Probe<'_, '_>, below: &BlockState) -> Option<bool> {
    if Probe::prop_is(p.state, "half", "upper") {
        return double_plant(p, below, Tag::SupportsSmallDripleaf);
    }
    let here = p.here();
    // `getFluidState(pos).getType() == Fluids.WATER` is the source fluid, not flowing water.
    let in_source_water = p.has(here, BlockFact::Water) && p.has(here, BlockFact::FluidSource);
    Some(
        p.tag(Tag::SupportsSmallDripleaf, below)
            || (in_source_water && p.tag(Tag::SupportsVegetation, below)),
    )
}

fn cactus(p: &Probe<'_, '_>, below: &BlockState) -> Option<bool> {
    for dir in Dir::HORIZONTAL {
        let side = p.at(dir);
        if p.has(side, BlockFact::Solid) || p.has(side, BlockFact::Lava) {
            return Some(false);
        }
    }
    if !p.same_block(below) && !p.tag(Tag::SupportsCactus, below) {
        return Some(false);
    }
    Some(!p.has(p.at(Dir::Up), BlockFact::Liquid))
}

fn sugar_cane(p: &Probe<'_, '_>, below: &BlockState) -> Option<bool> {
    if p.same_block(below) {
        return Some(true);
    }
    if !p.tag(Tag::SupportsSugarCane, below) {
        return Some(false);
    }
    let below_pos = Dir::Down.offset(p.pos);
    Some(Dir::HORIZONTAL.iter().any(|dir| {
        let side = p.block(dir.offset(below_pos));
        p.has(side, BlockFact::Water) || p.tag(Tag::SupportsSugarCaneAdjacently, side)
    }))
}

fn snow(p: &Probe<'_, '_>, below: &BlockState) -> Option<bool> {
    if p.tag(Tag::CannotSupportSnowLayer, below) {
        return Some(false);
    }
    if p.tag(Tag::SupportOverrideSnowLayer, below) {
        return Some(true);
    }
    or(
        p.collision_fills(below),
        Some(p.same_block(below) && Probe::prop_is(below, "layers", "8")),
    )
}

fn growing_plant(
    p: &Probe<'_, '_>,
    growth: Dir,
    head: &str,
    body: &str,
    kelp: bool,
) -> Option<bool> {
    let base = p.at(growth.opposite());
    if kelp && p.tag(Tag::CannotSupportKelp, base) {
        return Some(false);
    }
    if base.name.path() == head || base.name.path() == body {
        return Some(true);
    }
    p.sturdy(base, growth)
}

// PARITY-CHECK: fire also survives beside a flammable block; flammability is not in the facts,
// so a fire without a sturdy floor is unknown rather than false.
fn fire(p: &Probe<'_, '_>, below: &BlockState) -> Option<bool> {
    (p.sturdy(below, Dir::Up) == Some(true)).then_some(true)
}

/// The faces a multiface or vine state has set, by its `north`/`up`/... properties.
fn set_faces(state: &BlockState) -> Vec<Dir> {
    Dir::ALL
        .into_iter()
        .filter(|dir| Probe::prop_is(state, dir.name(), "true"))
        .collect()
}

fn multiface(p: &Probe<'_, '_>) -> Option<bool> {
    let faces = set_faces(p.state);
    let mut all = Some(!faces.is_empty());
    for face in faces {
        all = and(all, p.attachable(p.at(face), face.opposite()));
    }
    all
}

/// `VineBlock.canSurvive` is `hasFaces(getUpdatedState(..))`: some set face is held by its
/// neighbour, or the up face by the block above, or a side face by the same vine above.
fn vine(p: &Probe<'_, '_>) -> Option<bool> {
    let above = p.at(Dir::Up);
    let mut any = Some(false);
    for face in set_faces(p.state) {
        let held = match face {
            // PARITY-CHECK: `getUpdatedState` asks the block above for its Up face (it passes
            // Direction.DOWN to `canSupportAtFace`, which then flips it); verify against the game.
            Dir::Up => p.attachable(above, Dir::Up),
            Dir::Down => continue,
            side => or(
                p.attachable(p.at(side), side.opposite()),
                Some(p.same_block(above) && Probe::prop_is(above, side.name(), "true")),
            ),
        };
        any = or(any, held);
    }
    any
}
