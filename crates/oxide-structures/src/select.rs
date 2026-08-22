//! Weighted structure-set start selection: which `Structure` a `StructureSet`'s weighted
//! `structures` list picks at a placement chunk.

use oxide_core::RandomSource;
use oxide_datapack::StructureSetEntry;

/// Vanilla's `WeightedRandomList`-style pick: draws `[0, total_weight)` then walks entries
/// subtracting weight until the draw lands inside one. Negative weights (invalid data) are
/// treated as `0` rather than panicking.
///
/// PARITY-CHECK: which RNG this draw actually consumes in vanilla (a fresh per-chunk RNG vs.
/// the placement RNG continued) is not reconstructed here — this is a general-purpose weighted
/// picker; wiring it to the right RNG is the caller's job.
pub fn pick_weighted<'a>(
    rng: &mut impl RandomSource,
    entries: &'a [StructureSetEntry],
) -> Option<&'a StructureSetEntry> {
    let total: i32 = entries.iter().map(|e| e.weight.max(0)).sum();
    if total <= 0 {
        return None;
    }
    let mut draw = rng.next_int_bounded(total);
    for entry in entries {
        let w = entry.weight.max(0);
        if draw < w {
            return Some(entry);
        }
        draw -= w;
    }
    None // unreachable given the sum invariant above, but avoids a panic on a weird edge case
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::{LegacyRandom, ResourceLocation};

    fn entry(name: &str, weight: i32) -> StructureSetEntry {
        StructureSetEntry {
            structure: ResourceLocation::minecraft(name),
            weight,
        }
    }

    #[test]
    fn empty_list_picks_nothing() {
        let mut rng = LegacyRandom::new(1);
        assert!(pick_weighted(&mut rng, &[]).is_none());
    }

    #[test]
    fn all_zero_weight_picks_nothing() {
        let mut rng = LegacyRandom::new(1);
        let entries = vec![entry("a", 0), entry("b", 0)];
        assert!(pick_weighted(&mut rng, &entries).is_none());
    }

    #[test]
    fn single_positive_weight_always_wins() {
        let mut rng = LegacyRandom::new(1);
        let entries = vec![entry("only", 0), entry("winner", 5), entry("also_zero", 0)];
        let picked = pick_weighted(&mut rng, &entries).unwrap();
        assert_eq!(picked.structure.path(), "winner");
    }

    #[test]
    fn deterministic_for_same_seed() {
        let entries = vec![entry("a", 1), entry("b", 1), entry("c", 1)];
        let mut r1 = LegacyRandom::new(42);
        let mut r2 = LegacyRandom::new(42);
        let picks_a: Vec<_> = (0..20)
            .map(|_| {
                pick_weighted(&mut r1, &entries)
                    .unwrap()
                    .structure
                    .path()
                    .to_string()
            })
            .collect();
        let picks_b: Vec<_> = (0..20)
            .map(|_| {
                pick_weighted(&mut r2, &entries)
                    .unwrap()
                    .structure
                    .path()
                    .to_string()
            })
            .collect();
        assert_eq!(picks_a, picks_b);
    }

    #[test]
    fn every_entry_gets_picked_over_many_draws() {
        let entries = vec![entry("a", 1), entry("b", 1), entry("c", 1)];
        let mut rng = LegacyRandom::new(9);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            seen.insert(
                pick_weighted(&mut rng, &entries)
                    .unwrap()
                    .structure
                    .path()
                    .to_string(),
            );
        }
        assert_eq!(seen.len(), 3);
    }
}
