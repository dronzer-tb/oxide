//! `minecraft:random_spread` structure-set placement: which chunks get a structure-start
//! attempt, matching vanilla's `RandomSpreadStructurePlacement`.
//!
//! Scope cut, documented not fabricated: `minecraft:concentric_rings` (used almost exclusively
//! for strongholds) is a distinct, much rarer algorithm and is not implemented here.

use oxide_core::{ChunkPos, LegacyRandom, RandomSource};

/// Vanilla's `WorldgenRandom.setLargeFeatureWithSalt`: seeds a legacy RNG from the world seed,
/// a region-grid coordinate, and a structure-set-specific salt. This exact formula is one of
/// the most widely reimplemented pieces of vanilla worldgen (Amidst, Chunk-Base, cubiomes, ...)
/// and has been stable since the region-based placement rewrite — higher confidence than this
/// crate's other `// PARITY-CHECK`s, but still unverified against decompiled 26.2 source.
fn set_large_feature_seed(
    world_seed: i64,
    region_x: i32,
    region_z: i32,
    salt: i32,
) -> LegacyRandom {
    let seed = (region_x as i64)
        .wrapping_mul(341_873_128_712)
        .wrapping_add((region_z as i64).wrapping_mul(132_897_987_541))
        .wrapping_add(world_seed)
        .wrapping_add(salt as i64);
    LegacyRandom::new(seed)
}

/// The one deterministic candidate chunk within the `spacing`-sized region containing
/// `(chunk_x, chunk_z)` — vanilla places a structure-start attempt there and nowhere else in
/// the region.
pub fn potential_structure_chunk(
    world_seed: i64,
    spacing: i32,
    separation: i32,
    salt: i32,
    chunk_x: i32,
    chunk_z: i32,
) -> ChunkPos {
    let region_x = chunk_x.div_euclid(spacing);
    let region_z = chunk_z.div_euclid(spacing);
    let mut rng = set_large_feature_seed(world_seed, region_x, region_z, salt);
    let range = (spacing - separation).max(1);
    let offset_x = rng.next_int_bounded(range);
    let offset_z = rng.next_int_bounded(range);
    ChunkPos::new(region_x * spacing + offset_x, region_z * spacing + offset_z)
}

/// `(chunk_x, chunk_z)` is a structure-start attempt chunk for this placement iff it's the
/// region's one deterministic candidate — minimum separation between adjacent regions' picks
/// falls out of `spacing`/`separation` bounding the offset range, no extra distance check
/// needed.
///
/// Scope cut: frequency reduction (`frequency < 1.0`, `frequency_reduction_method`) and
/// exclusion zones (skip if within `chunk_count` of another structure set's placement) are not
/// applied — both need either a hash formula this crate doesn't reconstruct with confidence,
/// or cross-referencing other structure sets. A caller treating every `true` here as a firm
/// placement over-places relative to vanilla on structure sets that use either field.
pub fn is_random_spread_chunk(
    world_seed: i64,
    spacing: i32,
    separation: i32,
    salt: i32,
    chunk_x: i32,
    chunk_z: i32,
) -> bool {
    let candidate =
        potential_structure_chunk(world_seed, spacing, separation, salt, chunk_x, chunk_z);
    candidate.x == chunk_x && candidate.z == chunk_z
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_inputs() {
        let a = potential_structure_chunk(42, 32, 8, 12345, 3, -5);
        let b = potential_structure_chunk(42, 32, 8, 12345, 3, -5);
        assert_eq!(a, b);
    }

    #[test]
    fn candidate_stays_within_its_region() {
        let spacing = 32;
        for chunk_x in -3..3 {
            for chunk_z in -3..3 {
                let c = potential_structure_chunk(999, spacing, 8, 1, chunk_x, chunk_z);
                let region_x = chunk_x.div_euclid(spacing);
                let region_z = chunk_z.div_euclid(spacing);
                assert!(c.x >= region_x * spacing && c.x < (region_x + 1) * spacing);
                assert!(c.z >= region_z * spacing && c.z < (region_z + 1) * spacing);
            }
        }
    }

    #[test]
    fn the_candidate_chunk_itself_reports_true() {
        let seed = 777;
        let (spacing, separation, salt) = (24, 6, 4);
        let c = potential_structure_chunk(seed, spacing, separation, salt, 0, 0);
        assert!(is_random_spread_chunk(
            seed, spacing, separation, salt, c.x, c.z
        ));
    }

    #[test]
    fn a_non_candidate_chunk_reports_false() {
        let seed = 777;
        let (spacing, separation, salt) = (24, 6, 4);
        let c = potential_structure_chunk(seed, spacing, separation, salt, 0, 0);
        // Offset by one block from the true candidate must not also read as placed (the
        // candidate is a single specific chunk per region, not a range).
        assert!(!is_random_spread_chunk(
            seed,
            spacing,
            separation,
            salt,
            c.x + 1,
            c.z
        ));
    }

    #[test]
    fn different_salts_give_different_candidates() {
        let a = potential_structure_chunk(1, 32, 8, 1, 0, 0);
        let b = potential_structure_chunk(1, 32, 8, 2, 0, 0);
        assert_ne!(a, b);
    }
}
