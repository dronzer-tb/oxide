//! `net.minecraft.util.Mth`'s trigonometry table.
//!
//! Worldgen does not use real trigonometry. `Mth.sin`/`Mth.cos` index a 65536-entry table of
//! `f32`, and carvers, structure placement and several features are shaped by the values that
//! table returns -- which are a coarse quantisation of a sine wave, not a sine wave. Calling a
//! platform `sin()` instead produces a visibly different cave, not a last-ulp difference: the
//! table has 65536 steps across a full turn and stores the result as `f32`.
//!
//! Ported from the decompiled 26.1.2 `Mth`.

use std::sync::LazyLock;

/// `65536 / (2 * PI)`, the constant vanilla multiplies an angle by to get a table index. Written
/// as the literal from the decompiled source rather than computed, because computing it can land
/// on a different `f64` and shift an index at the boundaries.
const RADIANS_TO_INDEX: f64 = 10430.378350470453;

static SIN_TABLE: LazyLock<[f32; 65536]> = LazyLock::new(|| {
    let mut table = [0.0f32; 65536];
    for (i, slot) in table.iter_mut().enumerate() {
        // Vanilla's initialiser verbatim: `(float) Math.sin(i / 10430.378350470453)`. The double
        // sine is computed at full precision and only then narrowed to f32, so narrowing earlier
        // would give a different table.
        *slot = (i as f64 / RADIANS_TO_INDEX).sin() as f32;
    }
    table
});

/// `Mth.sin`. Takes the angle in radians.
#[inline]
pub fn sin(radians: f64) -> f32 {
    // The cast to i64 before masking is vanilla's, and it matters: it truncates toward zero, so a
    // negative angle indexes differently than a floor would give.
    SIN_TABLE[((radians * RADIANS_TO_INDEX) as i64 & 65535) as usize]
}

/// `Mth.cos`. Takes the angle in radians.
#[inline]
pub fn cos(radians: f64) -> f32 {
    SIN_TABLE[((radians * RADIANS_TO_INDEX + 16384.0) as i64 & 65535) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is a quantisation, so it is measurably *not* the real sine -- which is the whole
    /// reason it has to be ported rather than substituted with `f64::sin`.
    #[test]
    fn differs_from_real_trigonometry() {
        let angle = std::f64::consts::FRAC_PI_4;
        let table = sin(angle) as f64;
        let real = angle.sin();
        assert!(
            (table - real).abs() > 1e-9,
            "table sin {table} unexpectedly equals real sin {real}; if these agree the table is              not being consulted"
        );
    }

    /// Indices wrap rather than panicking, including for large and negative angles.
    #[test]
    fn wraps_instead_of_panicking() {
        for angle in [-1.0e6, -1000.0, -1.0, 0.0, 1.0, 1000.0, 1.0e6] {
            let _ = sin(angle);
            let _ = cos(angle);
        }
    }
}
