use oxide_core::{LegacyRandom, RandomSource};
use oxide_structures::starts::large_feature_seed;

/// Verifies that `large_feature_seed` matches Minecraft Java's `WorldgenRandom.setLargeFeatureSeed`.
///
/// In Java:
/// ```java
/// Random r = new Random(worldSeed);
/// long a = r.nextLong();
/// long b = r.nextLong();
/// long featureSeed = (long)chunkX * a ^ (long)chunkZ * b ^ worldSeed;
/// Random featureRandom = new Random(featureSeed);
/// ```
#[test]
fn test_large_feature_seed_parity() {
    let test_cases = [
        // (world_seed, chunk_x, chunk_z)
        (0i64, 0, 0),
        (42i64, 10, -5),
        (1234567890123456789i64, -100, 250),
        (-987654321098765432i64, 5555, -9999),
        (4489057056054590644i64, 625, 625), // Server's world seed
    ];

    for &(seed, cx, cz) in &test_cases {
        let mut rust_rng = large_feature_seed(seed, cx, cz);

        // Compute step-by-step with raw LegacyRandom to verify formula
        let mut base_rng = LegacyRandom::new(seed);
        let a = base_rng.next_long();
        let b = base_rng.next_long();
        let expected_seed = (cx as i64).wrapping_mul(a) ^ (cz as i64).wrapping_mul(b) ^ seed;
        let mut expected_rng = LegacyRandom::new(expected_seed);

        // Verify first 10 random integers match exactly
        for _ in 0..10 {
            assert_eq!(rust_rng.next_int(), expected_rng.next_int());
        }
    }
}
