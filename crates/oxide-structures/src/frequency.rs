//! `StructurePlacement.FrequencyReductionMethod`: the second gate a candidate chunk must pass.
//!
//! A structure set's placement picks one candidate chunk per region; frequency reduction then
//! discards some of them, which is how a structure ends up rarer than its spacing implies. All
//! four methods are ported from the decompiled `StructurePlacement`.
//!
//! Three of the four look like the same function and are not. The differences are the whole
//! reason this is a separate module rather than one `if`:
//!
//!   - `default` calls `setLargeFeatureWithSalt(seed, salt, sourceX, sourceZ)` -- against a
//!     signature of `(seed, x, z, salt)`. The arguments really are shifted by one in vanilla, so
//!     the salt lands in `x` and the chunk coordinates land in `z` and `salt`. Passing them in
//!     the sane order is the obvious mistake and produces a different world.
//!   - `legacy_type_2` calls the same function with the arguments in the expected order and a
//!     fixed salt.
//!   - `legacy_type_3` uses `setLargeFeatureSeed`, a different derivation entirely, and compares
//!     against `nextDouble` rather than `nextFloat`.
//!   - `legacy_type_1` ignores all of that, shifts its inputs right by 4 as if they were block
//!     coordinates, discards one `nextInt()`, and tests `nextInt(1 / probability) == 0`.

use oxide_core::{LegacyRandom, RandomSource};
use oxide_datapack::FrequencyReductionMethod;

/// `StructurePlacement.HIGHLY_ARBITRARY_RANDOM_SALT`. Vanilla's name, kept.
const HIGHLY_ARBITRARY_RANDOM_SALT: i32 = 10_387_320;

/// `WorldgenRandom.setLargeFeatureWithSalt`, seeded from a fresh legacy source.
///
/// Always `LegacyRandomSource`, whatever RNG flavour the world's noise generator uses.
fn large_feature_with_salt(seed: i64, x: i32, z: i32, salt: i32) -> LegacyRandom {
    LegacyRandom::new(
        (x as i64)
            .wrapping_mul(341_873_128_712)
            .wrapping_add((z as i64).wrapping_mul(132_897_987_541))
            .wrapping_add(seed)
            .wrapping_add(salt as i64),
    )
}

/// `WorldgenRandom.setLargeFeatureSeed`.
fn large_feature_seed(seed: i64, x: i32, z: i32) -> LegacyRandom {
    let mut rng = LegacyRandom::new(seed);
    let x_scale = rng.next_long();
    let z_scale = rng.next_long();
    LegacyRandom::new((x as i64).wrapping_mul(x_scale) ^ (z as i64).wrapping_mul(z_scale) ^ seed)
}

/// Whether a candidate chunk survives frequency reduction.
///
/// `salt_override` is Paper's per-structure-set seed configuration; `None` is vanilla.
pub fn should_generate(
    method: FrequencyReductionMethod,
    level_seed: i64,
    salt: i32,
    chunk_x: i32,
    chunk_z: i32,
    probability: f32,
    salt_override: Option<i32>,
) -> bool {
    match method {
        FrequencyReductionMethod::Default => {
            // Argument order is vanilla's, not a transcription slip -- see the module doc.
            let mut rng = large_feature_with_salt(level_seed, salt, chunk_x, chunk_z);
            rng.next_float() < probability
        }
        FrequencyReductionMethod::LegacyType1 => {
            // Shifts as though the inputs were block coordinates, and burns one draw.
            let cx = chunk_x >> 4;
            let cz = chunk_z >> 4;
            let mut rng = LegacyRandom::new(cx as i64 ^ ((cz as i64) << 4) ^ level_seed);
            rng.next_int();
            rng.next_int_bounded((1.0f32 / probability) as i32) == 0
        }
        FrequencyReductionMethod::LegacyType2 => {
            let mut rng = large_feature_with_salt(
                level_seed,
                chunk_x,
                chunk_z,
                salt_override.unwrap_or(HIGHLY_ARBITRARY_RANDOM_SALT),
            );
            rng.next_float() < probability
        }
        FrequencyReductionMethod::LegacyType3 => {
            let mut rng = match salt_override {
                None => large_feature_seed(level_seed, chunk_x, chunk_z),
                Some(over) => large_feature_with_salt(level_seed, chunk_x, chunk_z, over),
            };
            // nextDouble, not nextFloat -- the only method that differs here.
            rng.next_double() < probability as f64
        }
    }
}

/// Vanilla's `applyAdditionalChunkRestrictions`: reduction only applies below frequency 1.
pub fn passes_frequency(
    method: Option<FrequencyReductionMethod>,
    level_seed: i64,
    salt: i32,
    chunk_x: i32,
    chunk_z: i32,
    frequency: Option<f32>,
    salt_override: Option<i32>,
) -> bool {
    let probability = frequency.unwrap_or(1.0);
    if probability >= 1.0 {
        return true;
    }
    should_generate(
        method.unwrap_or(FrequencyReductionMethod::Default),
        level_seed,
        salt,
        chunk_x,
        chunk_z,
        probability,
        salt_override,
    )
}
