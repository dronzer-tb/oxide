//! Cross-checks `large_feature_seed` against real Minecraft 26.2 output.
//!
//! Vectors come from `tests/data/FeatureSeedOracle.java`, which runs the game's own
//! `WorldgenRandom.setLargeFeatureSeed` out of a Mojang-mapped 26.2 jar — not a transcription of
//! it — so a mistake in how the derivation was remembered shows up here as a mismatch. See that
//! file's header to regenerate.
//!
//! The bytecode it exercises (`javap -c net.minecraft.world.level.levelgen.WorldgenRandom`):
//! `setSeed(baseSeed)`, two `nextLong()` draws, then
//! `setSeed((long) x * first ^ (long) z * second ^ baseSeed)`.
//!
//! This replaces a test that recomputed the same formula inline and compared it to the function —
//! which could only fail if someone edited one copy and not the other, and would have passed
//! just as happily on a wrong formula.

use oxide_core::RandomSource;
use oxide_structures::starts::large_feature_seed;

#[test]
fn matches_java_for_every_vector() {
    let text = include_str!("data/feature_seed_vectors.txt");
    let mut checked = 0usize;

    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let seed: i64 = fields.next().unwrap().parse().unwrap();
        let chunk_x: i32 = fields.next().unwrap().parse().unwrap();
        let chunk_z: i32 = fields.next().unwrap().parse().unwrap();

        let mut rng = large_feature_seed(seed, chunk_x, chunk_z);
        let mut draws = 0usize;
        for expected in fields {
            let expected: i32 = expected.parse().unwrap();
            assert_eq!(
                rng.next_int(),
                expected,
                "line {}: seed {seed} chunk ({chunk_x}, {chunk_z}), draw {draws}",
                line_number + 1
            );
            draws += 1;
        }
        assert_eq!(draws, 8, "line {}: expected 8 draws", line_number + 1);
        checked += 1;
    }

    assert_eq!(checked, 72, "vector file is not the one this test was written for");
}
