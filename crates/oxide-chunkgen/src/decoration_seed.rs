//! The two seed derivations every decorated block position ultimately comes from.
//!
//! Ported from `net.minecraft.world.level.levelgen.WorldgenRandom`. Both are exact integer
//! recipes with no room for interpretation, and everything downstream -- which features run,
//! where they land, what blocks they pick -- is drawn from the stream they leave behind, so a
//! single wrong operation here moves every tree in the world.

use oxide_core::RandomSource;

/// Seeds `random` for one chunk's decoration and returns the seed the per-feature derivation
/// starts from.
///
/// `WorldgenRandom#setDecorationSeed`. `origin_x`/`origin_z` are the chunk's minimum *block*
/// coordinates, not its chunk coordinates -- vanilla passes `SectionPos.origin()`, and passing
/// chunk coordinates instead would still produce a plausible-looking world seeded wrongly
/// everywhere.
pub fn set_decoration_seed<R: RandomSource>(
    random: &mut R,
    level_seed: i64,
    origin_x: i32,
    origin_z: i32,
) -> i64 {
    random.set_seed(level_seed);
    // `| 1` on both, so neither multiplier can be even and collapse the low bits.
    let x_scale = random.next_long() | 1;
    let z_scale = random.next_long() | 1;
    let decoration_seed = (origin_x as i64)
        .wrapping_mul(x_scale)
        .wrapping_add((origin_z as i64).wrapping_mul(z_scale))
        ^ level_seed;
    random.set_seed(decoration_seed);
    decoration_seed
}

/// Seeds `random` for one feature within a chunk's decoration.
///
/// `WorldgenRandom#setFeatureSeed`. `index` is the feature's position in the global ordering
/// built by [`crate::feature_order`], not its position within the biome -- see that module for
/// why the distinction decides whether the world matches vanilla.
pub fn set_feature_seed<R: RandomSource>(
    random: &mut R,
    decoration_seed: i64,
    index: i32,
    step: i32,
) {
    let seed = decoration_seed
        .wrapping_add(index as i64)
        .wrapping_add(10_000i64.wrapping_mul(step as i64));
    random.set_seed(seed);
}

/// `WorldgenRandom#setLargeFeatureSeed`, used by structure starts rather than decoration.
///
/// Note the differences from [`set_decoration_seed`]: no `| 1` on the multipliers, and the two
/// products are combined with `^` rather than `+`.
pub fn set_large_feature_seed<R: RandomSource>(
    random: &mut R,
    level_seed: i64,
    chunk_x: i32,
    chunk_z: i32,
) {
    random.set_seed(level_seed);
    let x_scale = random.next_long();
    let z_scale = random.next_long();
    let seed = (chunk_x as i64).wrapping_mul(x_scale)
        ^ (chunk_z as i64).wrapping_mul(z_scale)
        ^ level_seed;
    random.set_seed(seed);
}

/// `WorldgenRandom#setLargeFeatureWithSalt`, the salted per-structure derivation.
pub fn set_large_feature_with_salt<R: RandomSource>(
    random: &mut R,
    level_seed: i64,
    x: i32,
    z: i32,
    salt: i32,
) {
    let seed = (x as i64)
        .wrapping_mul(341_873_128_712i64)
        .wrapping_add((z as i64).wrapping_mul(132_897_987_541i64))
        .wrapping_add(level_seed)
        .wrapping_add(salt as i64);
    random.set_seed(seed);
}
