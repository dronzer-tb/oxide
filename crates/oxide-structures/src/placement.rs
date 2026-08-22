//! `minecraft:random_spread` structure-set placement: which chunks get a structure-start
//! attempt, matching vanilla's `RandomSpreadStructurePlacement`.
//!
//! Scope cut, documented not fabricated: `minecraft:concentric_rings` (used almost exclusively
//! for strongholds) is a distinct, much rarer algorithm and is not implemented here.

use oxide_core::{ChunkPos, LegacyRandom, RandomSource};
use oxide_datapack::SpreadType;

/// Verified 2026-08-22 against decompiled `RandomSpreadType.evaluate`: `Linear` draws one
/// `nextInt(limit)`; `Triangular` averages two independent draws (`(a + b) / 2`, integer
/// division) — a tighter, bell-shaped spread around the region center.
fn evaluate_spread(spread_type: SpreadType, rng: &mut LegacyRandom, limit: i32) -> i32 {
    match spread_type {
        SpreadType::Linear => rng.next_int_bounded(limit),
        SpreadType::Triangular => (rng.next_int_bounded(limit) + rng.next_int_bounded(limit)) / 2,
    }
}

/// Vanilla's `WorldgenRandom.setLargeFeatureWithSalt`: seeds a legacy RNG from the world seed,
/// a region-grid coordinate, and a structure-set-specific salt. Verified 2026-08-22 against
/// decompiled `WorldgenRandom`/`StructurePlacement`/`RandomSpreadStructurePlacement` — always
/// `new WorldgenRandom(new LegacyRandomSource(0L))`, regardless of the world's noise-generator
/// RNG flavor.
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
    spread_type: SpreadType,
    chunk_x: i32,
    chunk_z: i32,
) -> ChunkPos {
    let region_x = chunk_x.div_euclid(spacing);
    let region_z = chunk_z.div_euclid(spacing);
    let mut rng = set_large_feature_seed(world_seed, region_x, region_z, salt);
    let range = (spacing - separation).max(1);
    let offset_x = evaluate_spread(spread_type, &mut rng, range);
    let offset_z = evaluate_spread(spread_type, &mut rng, range);
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
    spread_type: SpreadType,
    chunk_x: i32,
    chunk_z: i32,
) -> bool {
    let candidate = potential_structure_chunk(
        world_seed,
        spacing,
        separation,
        salt,
        spread_type,
        chunk_x,
        chunk_z,
    );
    candidate.x == chunk_x && candidate.z == chunk_z
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured 2026-08-22 from a standalone Java transcription of
    /// `RandomSpreadStructurePlacement.getPotentialStructureChunk` (decompiled from the real
    /// Minecraft 26.2 server jar) run on real OpenJDK 25 — see the session's scratchpad
    /// `decomp/PlacementRef.java`.
    #[test]
    fn potential_structure_chunk_matches_real_java() {
        let c = potential_structure_chunk(42, 32, 8, 12345, SpreadType::Linear, 3, -5);
        assert_eq!(c, ChunkPos::new(17, -10));

        let c2 = potential_structure_chunk(777, 24, 6, 4, SpreadType::Linear, 0, 0);
        assert_eq!(c2, ChunkPos::new(0, 12));

        let tri = potential_structure_chunk(42, 64, 16, 7, SpreadType::Triangular, 0, 0);
        assert_eq!(tri, ChunkPos::new(22, 19));
    }

    #[test]
    fn deterministic_for_same_inputs() {
        let a = potential_structure_chunk(42, 32, 8, 12345, SpreadType::Linear, 3, -5);
        let b = potential_structure_chunk(42, 32, 8, 12345, SpreadType::Linear, 3, -5);
        assert_eq!(a, b);
    }

    #[test]
    fn candidate_stays_within_its_region() {
        let spacing = 32;
        for chunk_x in -3..3 {
            for chunk_z in -3..3 {
                let c = potential_structure_chunk(
                    999,
                    spacing,
                    8,
                    1,
                    SpreadType::Linear,
                    chunk_x,
                    chunk_z,
                );
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
        let c =
            potential_structure_chunk(seed, spacing, separation, salt, SpreadType::Linear, 0, 0);
        assert!(is_random_spread_chunk(
            seed,
            spacing,
            separation,
            salt,
            SpreadType::Linear,
            c.x,
            c.z
        ));
    }

    #[test]
    fn a_non_candidate_chunk_reports_false() {
        let seed = 777;
        let (spacing, separation, salt) = (24, 6, 4);
        let c =
            potential_structure_chunk(seed, spacing, separation, salt, SpreadType::Linear, 0, 0);
        // Offset by one block from the true candidate must not also read as placed (the
        // candidate is a single specific chunk per region, not a range).
        assert!(!is_random_spread_chunk(
            seed,
            spacing,
            separation,
            salt,
            SpreadType::Linear,
            c.x + 1,
            c.z
        ));
    }

    #[test]
    fn different_salts_give_different_candidates() {
        let a = potential_structure_chunk(1, 32, 8, 1, SpreadType::Linear, 0, 0);
        let b = potential_structure_chunk(1, 32, 8, 2, SpreadType::Linear, 0, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn triangular_spread_differs_from_linear() {
        let (seed, spacing, separation, salt) = (42, 64, 16, 7);
        let linear =
            potential_structure_chunk(seed, spacing, separation, salt, SpreadType::Linear, 0, 0);
        let triangular = potential_structure_chunk(
            seed,
            spacing,
            separation,
            salt,
            SpreadType::Triangular,
            0,
            0,
        );
        assert_ne!(linear, triangular);
    }

    #[test]
    fn triangular_spread_stays_within_its_region() {
        let spacing = 32;
        for chunk_x in -3..3 {
            for chunk_z in -3..3 {
                let c = potential_structure_chunk(
                    999,
                    spacing,
                    8,
                    1,
                    SpreadType::Triangular,
                    chunk_x,
                    chunk_z,
                );
                let region_x = chunk_x.div_euclid(spacing);
                let region_z = chunk_z.div_euclid(spacing);
                assert!(c.x >= region_x * spacing && c.x < (region_x + 1) * spacing);
                assert!(c.z >= region_z * spacing && c.z < (region_z + 1) * spacing);
            }
        }
    }
}
