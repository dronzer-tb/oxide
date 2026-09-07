//! Whether a chunk is a structure-start chunk for a whole structure *set*, rather than for a
//! bare placement.
//!
//! Vanilla's `StructurePlacement.isStructureChunk` is three gates in sequence, and all three
//! have to pass:
//!
//! 1. `isPlacementChunk` -- the placement's own candidate test (`random_spread` here);
//! 2. `applyAdditionalChunkRestrictions` -- frequency reduction;
//! 3. `applyInteractionsWithOtherStructures` -- the exclusion zone.
//!
//! Ported from the decompiled `StructurePlacement` and `ChunkGeneratorStructureState`.

use oxide_datapack::{StructurePlacement, StructureSet};

use crate::frequency::passes_frequency;
use crate::placement::{is_concentric_rings_chunk, is_random_spread_chunk};

/// Looks up another structure set by id. Exclusion zones name a set, so testing one requires
/// resolving it -- the caller owns the registry, so it supplies the lookup.
pub trait StructureSetLookup {
    fn get(&self, id: &oxide_core::ResourceLocation) -> Option<&StructureSet>;
}

/// Whether `(chunk_x, chunk_z)` is a structure-start chunk for `set`.
///
/// `salt_override` is Paper's per-set seed configuration; `None` is vanilla behaviour.
pub fn is_structure_chunk(
    set: &StructureSet,
    lookup: &dyn StructureSetLookup,
    world_seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    salt_override: Option<i32>,
) -> bool {
    match &set.placement {
        StructurePlacement::RandomSpread {
            spacing,
            separation,
            salt,
            frequency_reduction_method,
            frequency,
            exclusion_zone,
            spread_type,
            ..
        } => {
            if !is_random_spread_chunk(
                world_seed,
                *spacing,
                *separation,
                *salt,
                spread_type.unwrap_or_default(),
                chunk_x,
                chunk_z,
            ) {
                return false;
            }
            if !passes_frequency(
                *frequency_reduction_method,
                world_seed,
                *salt,
                chunk_x,
                chunk_z,
                *frequency,
                salt_override,
            ) {
                return false;
            }
            match exclusion_zone {
                None => true,
                Some(zone) => !is_placement_forbidden(
                    zone.other_set.clone(),
                    zone.chunk_count,
                    lookup,
                    world_seed,
                    chunk_x,
                    chunk_z,
                ),
            }
        }
        StructurePlacement::ConcentricRings {
            distance,
            spread,
            count,
            ..
        } => is_concentric_rings_chunk(world_seed, *distance, *spread, *count, chunk_x, chunk_z),
    }
}

/// Whether this set's placement is one this crate can actually decide.
pub fn is_structure_chunk_supported(_set: &StructureSet) -> bool {
    true
}

/// `ExclusionZone.isPlacementForbidden` -> `hasStructureChunkInRange`.
///
/// Scans the square of chunks within `chunk_count` of the candidate and asks whether the *other*
/// set would place in any of them. Note this is a full `isStructureChunk` on the other set, so
/// that set's own frequency reduction and exclusion zone apply -- an exclusion zone against a
/// set that itself gets frequency-reduced away does not exclude anything.
fn is_placement_forbidden(
    other_set_id: oxide_core::ResourceLocation,
    chunk_count: i32,
    lookup: &dyn StructureSetLookup,
    world_seed: i64,
    chunk_x: i32,
    chunk_z: i32,
) -> bool {
    let Some(other) = lookup.get(&other_set_id) else {
        // A dangling id excludes nothing, matching a datapack whose reference did not resolve.
        return false;
    };
    for test_x in (chunk_x - chunk_count)..=(chunk_x + chunk_count) {
        for test_z in (chunk_z - chunk_count)..=(chunk_z + chunk_count) {
            // No salt override: vanilla passes one only for a KeyedRandomSpreadStructurePlacement,
            // which is Paper's per-set seed configuration and not part of the vanilla path.
            if is_structure_chunk(other, lookup, world_seed, test_x, test_z, None) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::ResourceLocation;
    use std::collections::HashMap;

    struct Registry(HashMap<String, StructureSet>);

    impl StructureSetLookup for Registry {
        fn get(&self, id: &ResourceLocation) -> Option<&StructureSet> {
            self.0.get(&id.to_string())
        }
    }

    fn spread_set(
        spacing: i32,
        separation: i32,
        salt: i32,
        frequency: Option<f32>,
        exclusion_zone: Option<oxide_datapack::ExclusionZone>,
    ) -> StructureSet {
        StructureSet {
            structures: Vec::new(),
            placement: StructurePlacement::RandomSpread {
                spacing,
                separation,
                salt,
                frequency_reduction_method: None,
                frequency,
                locate_offset: None,
                exclusion_zone,
                spread_type: None,
            },
        }
    }

    fn empty_registry() -> Registry {
        Registry(HashMap::new())
    }

    /// Baseline: the set places somewhere in a region, and the chunks it places in are exactly
    /// the ones the bare placement picks.
    #[test]
    fn without_extras_it_matches_the_bare_placement() {
        let set = spread_set(32, 8, 165745296, None, None);
        let registry = empty_registry();
        let mut agreements = 0;
        for cx in -40..40 {
            for cz in -40..40 {
                let bare = is_random_spread_chunk(
                    42,
                    32,
                    8,
                    165745296,
                    oxide_datapack::SpreadType::Linear,
                    cx,
                    cz,
                );
                let full = is_structure_chunk(&set, &registry, 42, cx, cz, None);
                assert_eq!(bare, full, "chunk {cx},{cz}");
                if bare {
                    agreements += 1;
                }
            }
        }
        assert!(
            agreements > 0,
            "the placement never fired; test proves nothing"
        );
    }

    /// Frequency reduction can only ever remove placements, never add them.
    #[test]
    fn frequency_reduction_is_a_subset() {
        let full = spread_set(32, 8, 165745296, None, None);
        let reduced = spread_set(32, 8, 165745296, Some(0.2), None);
        let registry = empty_registry();
        let mut kept = 0;
        let mut dropped = 0;
        for cx in -60..60 {
            for cz in -60..60 {
                if !is_structure_chunk(&full, &registry, 7, cx, cz, None) {
                    assert!(
                        !is_structure_chunk(&reduced, &registry, 7, cx, cz, None),
                        "reduction added a placement at {cx},{cz}"
                    );
                    continue;
                }
                if is_structure_chunk(&reduced, &registry, 7, cx, cz, None) {
                    kept += 1;
                } else {
                    dropped += 1;
                }
            }
        }
        assert!(kept > 0 && dropped > 0, "kept {kept}, dropped {dropped}");
    }

    /// An exclusion zone against a set that always places nearby removes every placement.
    #[test]
    fn an_exclusion_zone_can_forbid_everything() {
        // spacing 1 means every chunk is that set's candidate chunk.
        let blocker = spread_set(1, 0, 12345, None, None);
        let mut map = HashMap::new();
        map.insert("minecraft:blocker".to_string(), blocker);
        let registry = Registry(map);

        let excluded = spread_set(
            32,
            8,
            165745296,
            None,
            Some(oxide_datapack::ExclusionZone {
                other_set: "minecraft:blocker".parse().unwrap(),
                chunk_count: 1,
            }),
        );
        for cx in -40..40 {
            for cz in -40..40 {
                assert!(
                    !is_structure_chunk(&excluded, &registry, 42, cx, cz, None),
                    "chunk {cx},{cz} placed despite a blanket exclusion zone"
                );
            }
        }
    }

    /// A zone naming a set the pack does not define excludes nothing, rather than excluding
    /// everything -- a dangling reference must not silently delete a structure from the world.
    #[test]
    fn a_dangling_exclusion_zone_excludes_nothing() {
        let set = spread_set(
            32,
            8,
            165745296,
            None,
            Some(oxide_datapack::ExclusionZone {
                other_set: "minecraft:does_not_exist".parse().unwrap(),
                chunk_count: 4,
            }),
        );
        let registry = empty_registry();
        let mut placements = 0;
        for cx in -40..40 {
            for cz in -40..40 {
                if is_structure_chunk(&set, &registry, 42, cx, cz, None) {
                    placements += 1;
                }
            }
        }
        assert!(
            placements > 0,
            "a dangling exclusion zone removed every placement"
        );
    }

    /// Concentric rings structure sets are supported and generate candidate chunks.
    #[test]
    fn concentric_rings_generates_expected_count() {
        let rings = StructureSet {
            structures: Vec::new(),
            placement: StructurePlacement::ConcentricRings {
                distance: 32,
                spread: 3,
                count: 128,
                preferred_biomes: oxide_datapack::BiomeFilter::Tag(
                    "#minecraft:stronghold_biased_to".into(),
                ),
                frequency_reduction_method: None,
                frequency: None,
                locate_offset: None,
                exclusion_zone: None,
            },
        };
        assert!(is_structure_chunk_supported(&rings));
        let chunks = crate::placement::concentric_rings_chunks(42, 32, 3, 128);
        assert_eq!(chunks.len(), 128);
        assert!(is_structure_chunk(
            &rings,
            &empty_registry(),
            42,
            chunks[0].x,
            chunks[0].z,
            None
        ));

        let spread = spread_set(32, 8, 1, None, None);
        assert!(is_structure_chunk_supported(&spread));
    }
}
