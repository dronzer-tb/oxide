//! Integer and height providers, and the vertical anchors the latter resolve.
//!
//! Every provider here draws from the random stream in vanilla's order, because a placement
//! modifier that draws one value too many shifts every later draw of the feature. The data
//! types are the parsed IR from `oxide-datapack`; this module only samples them.
//!
//! Float providers are not duplicated here: `oxide_datapack::FloatProvider::sample` is already
//! a port of the same classes, and no decoration code path in the 26.2 data samples one outside
//! the carvers that already use it.
//!
//! The height providers deliberately do not reuse `oxide_datapack::HeightProvider::sample`. That
//! one was written for the carvers, which only use the uniform and trapezoid shapes, and its
//! biased-to-bottom arms are not the algorithm placement needs.

use oxide_core::RandomSource;
use oxide_datapack::carver::TaggedHeightProvider;
use oxide_datapack::placement::{IntProvider, TaggedIntProvider};
use oxide_datapack::{HeightProvider, VerticalAnchor};

/// The vertical extent anchors resolve against: vanilla's `WorldGenerationContext`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldBounds {
    pub min_y: i32,
    /// Number of blocks from the lowest to the highest block, inclusive.
    pub height: i32,
}

/// `VerticalAnchor.resolveY`. Arithmetic wraps like Java's `int`.
pub fn resolve_anchor(anchor: &VerticalAnchor, bounds: WorldBounds) -> i32 {
    match anchor {
        VerticalAnchor::Absolute { absolute } => *absolute,
        VerticalAnchor::AboveBottom { above_bottom } => bounds.min_y.wrapping_add(*above_bottom),
        VerticalAnchor::BelowTop { below_top } => bounds
            .height
            .wrapping_sub(1)
            .wrapping_add(bounds.min_y)
            .wrapping_sub(*below_top),
    }
}

/// `RandomSource.nextInt(bound)`, with a non-positive bound drawing nothing and giving 0.
///
/// Vanilla throws for such a bound. The codecs reject every provider that could produce one, so
/// loaded data never reaches this; it exists so a malformed provider cannot panic the server.
pub fn next_bounded(random: &mut impl RandomSource, bound: i32) -> i32 {
    if bound <= 0 {
        0
    } else {
        random.next_int_bounded(bound)
    }
}

/// `Mth.randomBetweenInclusive`: `nextInt(max - min + 1) + min`. An empty range gives `min`.
pub fn between_inclusive(random: &mut impl RandomSource, min: i32, max: i32) -> i32 {
    min.wrapping_add(next_bounded(random, max.wrapping_sub(min).wrapping_add(1)))
}

/// `IntProvider.sample`.
pub fn sample_int(provider: &IntProvider, random: &mut impl RandomSource) -> i32 {
    let tagged = match provider {
        IntProvider::Constant(value) => return *value,
        IntProvider::Tagged(tagged) => &**tagged,
    };
    match tagged {
        TaggedIntProvider::Constant { value } => *value,
        TaggedIntProvider::Uniform {
            min_inclusive,
            max_inclusive,
        } => between_inclusive(random, *min_inclusive, *max_inclusive),
        TaggedIntProvider::BiasedToBottom {
            min_inclusive,
            max_inclusive,
        } => {
            let mut value = next_bounded(
                random,
                max_inclusive.wrapping_sub(*min_inclusive).wrapping_add(1),
            );
            value = next_bounded(random, value.wrapping_add(1));
            min_inclusive.wrapping_add(value)
        }
        TaggedIntProvider::Clamped {
            source,
            min_inclusive,
            max_inclusive,
        } => {
            let value = sample_int(source, random);
            if value < *min_inclusive {
                *min_inclusive
            } else {
                value.min(*max_inclusive)
            }
        }
        // `Mth.normal` then a float clamp and a truncating, saturating cast to int.
        TaggedIntProvider::ClampedNormal {
            mean,
            deviation,
            min_inclusive,
            max_inclusive,
        } => {
            let value = mean + random.next_gaussian() as f32 * deviation;
            let (min, max) = (*min_inclusive as f32, *max_inclusive as f32);
            (if value < min { min } else { value.min(max) }) as i32
        }
        // PARITY-CHECK: the `plateau == 0 && max == -min` arm is the reference implementation's
        // (it targets 26.3). It draws `nextInt(max + 1) - nextInt(max + 1)`, which is the same
        // distribution as the general arm but a different value for the same two draws. Check
        // against a real 26.2 `random_offset` before trusting any vegetation position.
        TaggedIntProvider::Trapezoid { min, max, plateau } => {
            if *plateau == 0 && *max == min.wrapping_neg() {
                let left = next_bounded(random, max.wrapping_add(1));
                let right = next_bounded(random, max.wrapping_add(1));
                return left.wrapping_sub(right);
            }
            let range = max.wrapping_sub(*min);
            if *plateau == range {
                return between_inclusive(random, *min, *max);
            }
            let slope = range.wrapping_sub(*plateau) / 2;
            let middle = range.wrapping_sub(slope);
            let left = between_inclusive(random, 0, middle);
            let right = between_inclusive(random, 0, slope);
            min.wrapping_add(left).wrapping_add(right)
        }
        // `WeightedList.getRandom`: one bounded draw even for a single entry, then the chosen
        // provider's own draws. An all-zero list gives 0, where vanilla's codec rejects it.
        TaggedIntProvider::WeightedList { distribution } => {
            let total = distribution
                .iter()
                .fold(0i32, |sum, entry| sum.wrapping_add(entry.weight));
            if total <= 0 {
                return 0;
            }
            let mut remaining = random.next_int_bounded(total);
            for entry in distribution {
                remaining -= entry.weight;
                if remaining < 0 {
                    return sample_int(&entry.data, random);
                }
            }
            0
        }
    }
}

/// `HeightProvider.sample`.
pub fn sample_height(
    provider: &HeightProvider,
    random: &mut impl RandomSource,
    bounds: WorldBounds,
) -> i32 {
    let tagged = match provider {
        HeightProvider::Anchor(anchor) => return resolve_anchor(anchor, bounds),
        HeightProvider::Tagged(tagged) => &**tagged,
    };
    match tagged {
        TaggedHeightProvider::Constant { value } => resolve_anchor(value, bounds),
        TaggedHeightProvider::Uniform {
            min_inclusive,
            max_inclusive,
        } => {
            let (min, max) = (
                resolve_anchor(min_inclusive, bounds),
                resolve_anchor(max_inclusive, bounds),
            );
            if min > max {
                min
            } else {
                between_inclusive(random, min, max)
            }
        }
        TaggedHeightProvider::BiasedToBottom {
            min_inclusive,
            max_inclusive,
            inner,
        } => {
            let (min, max) = (
                resolve_anchor(min_inclusive, bounds),
                resolve_anchor(max_inclusive, bounds),
            );
            let range = max.wrapping_sub(min).wrapping_sub(*inner).wrapping_add(1);
            if range <= 0 {
                return min;
            }
            let first = random.next_int_bounded(range);
            next_bounded(random, first.wrapping_add(*inner)).wrapping_add(min)
        }
        // PARITY-CHECK: `next_or_min` skips the draw when its range is empty or a single value;
        // the reference does this for the very-biased provider only. Vanilla's
        // `Mth.randomBetweenInclusive` would draw `nextInt(1)` for a single-value range. The two
        // 26.2 placed features that use this provider have wide ranges, so the arm is not
        // reached, but a real 26.2 run should confirm it.
        TaggedHeightProvider::VeryBiasedToBottom {
            min_inclusive,
            max_inclusive,
            inner,
        } => {
            let (min, max) = (
                resolve_anchor(min_inclusive, bounds),
                resolve_anchor(max_inclusive, bounds),
            );
            let range = max.wrapping_sub(min).wrapping_sub(*inner).wrapping_add(1);
            if range <= 0 {
                return min;
            }
            let first = next_or_min(random, min.wrapping_add(*inner), max);
            let second = next_or_min(random, min, first.wrapping_sub(1));
            next_or_min(random, min, second.wrapping_sub(1).wrapping_add(*inner))
        }
        TaggedHeightProvider::Trapezoid {
            min_inclusive,
            max_inclusive,
            plateau,
        } => {
            let (min, max) = (
                resolve_anchor(min_inclusive, bounds),
                resolve_anchor(max_inclusive, bounds),
            );
            if min > max {
                return min;
            }
            let range = max.wrapping_sub(min);
            if *plateau >= range {
                return between_inclusive(random, min, max);
            }
            let slope = range.wrapping_sub(*plateau) / 2;
            let middle = range.wrapping_sub(slope);
            let left = between_inclusive(random, 0, middle);
            let right = between_inclusive(random, 0, slope);
            min.wrapping_add(left).wrapping_add(right)
        }
    }
}

fn next_or_min(random: &mut impl RandomSource, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        between_inclusive(random, min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::Xoroshiro128PlusPlus;

    const BOUNDS: WorldBounds = WorldBounds { min_y: -64, height: 384 };

    fn int(json: &str) -> IntProvider {
        serde_json::from_str(json).expect("int provider json")
    }

    fn height(json: &str) -> HeightProvider {
        serde_json::from_str(json).expect("height provider json")
    }

    #[test]
    fn anchors_resolve_against_the_world_extent() {
        let anchor = |json: &str| -> i32 {
            resolve_anchor(&serde_json::from_str(json).unwrap(), BOUNDS)
        };
        assert_eq!(anchor(r#"{"absolute": 12}"#), 12);
        assert_eq!(anchor(r#"{"above_bottom": 8}"#), -56);
        // The topmost block is 319, so `below_top: 0` is 319, not 320.
        assert_eq!(anchor(r#"{"below_top": 0}"#), 319);
        assert_eq!(anchor(r#"{"below_top": 10}"#), 309);
    }

    #[test]
    fn constants_draw_nothing() {
        let mut random = Xoroshiro128PlusPlus::new(7);
        let mut untouched = Xoroshiro128PlusPlus::new(7);
        assert_eq!(sample_int(&int("5"), &mut random), 5);
        assert_eq!(sample_int(&int(r#"{"type":"minecraft:constant","value":9}"#), &mut random), 9);
        assert_eq!(random.next_long(), untouched.next_long());
    }

    #[test]
    fn uniform_is_one_bounded_draw_in_range() {
        let provider = int(r#"{"type":"minecraft:uniform","min_inclusive":3,"max_inclusive":6}"#);
        let mut random = Xoroshiro128PlusPlus::new(11);
        let mut mirror = Xoroshiro128PlusPlus::new(11);
        for _ in 0..200 {
            let value = sample_int(&provider, &mut random);
            assert_eq!(value, 3 + mirror.next_int_bounded(4));
            assert!((3..=6).contains(&value));
        }
    }

    #[test]
    fn symmetric_trapezoid_is_the_difference_of_two_draws() {
        let provider =
            int(r#"{"type":"minecraft:trapezoid","min":-7,"max":7,"plateau":0}"#);
        let mut random = Xoroshiro128PlusPlus::new(5);
        let mut mirror = Xoroshiro128PlusPlus::new(5);
        for _ in 0..200 {
            let value = sample_int(&provider, &mut random);
            assert_eq!(value, mirror.next_int_bounded(8) - mirror.next_int_bounded(8));
            assert!((-7..=7).contains(&value));
        }
    }

    #[test]
    fn general_trapezoid_adds_two_ramps_to_the_minimum() {
        let provider = int(r#"{"type":"minecraft:trapezoid","min":2,"max":12,"plateau":4}"#);
        let mut random = Xoroshiro128PlusPlus::new(21);
        let mut mirror = Xoroshiro128PlusPlus::new(21);
        // range 10, slope (10 - 4) / 2 = 3, middle 7.
        let value = sample_int(&provider, &mut random);
        assert_eq!(value, 2 + mirror.next_int_bounded(8) + mirror.next_int_bounded(4));
    }

    #[test]
    fn clamped_bounds_its_source() {
        let provider = int(
            r#"{"type":"minecraft:clamped","min_inclusive":4,"max_inclusive":5,
                "source":{"type":"minecraft:uniform","min_inclusive":0,"max_inclusive":20}}"#,
        );
        let mut random = Xoroshiro128PlusPlus::new(3);
        for _ in 0..100 {
            assert!((4..=5).contains(&sample_int(&provider, &mut random)));
        }
    }

    #[test]
    fn weighted_list_draws_once_then_samples_the_entry() {
        let provider = int(
            r#"{"type":"minecraft:weighted_list","distribution":[
                {"data":1,"weight":1},{"data":2,"weight":3}]}"#,
        );
        let mut random = Xoroshiro128PlusPlus::new(13);
        let mut mirror = Xoroshiro128PlusPlus::new(13);
        for _ in 0..100 {
            let expected = if mirror.next_int_bounded(4) < 1 { 1 } else { 2 };
            assert_eq!(sample_int(&provider, &mut random), expected);
        }
    }

    #[test]
    fn height_providers_resolve_anchors_before_sampling() {
        let uniform = height(
            r#"{"type":"minecraft:uniform","min_inclusive":{"above_bottom":0},
                "max_inclusive":{"absolute":10}}"#,
        );
        let mut random = Xoroshiro128PlusPlus::new(1);
        for _ in 0..100 {
            assert!((-64..=10).contains(&sample_height(&uniform, &mut random, BOUNDS)));
        }
        // A bare anchor is a constant that draws nothing.
        let mut untouched = Xoroshiro128PlusPlus::new(1);
        let mut random = Xoroshiro128PlusPlus::new(1);
        assert_eq!(sample_height(&height(r#"{"absolute": 40}"#), &mut random, BOUNDS), 40);
        assert_eq!(random.next_long(), untouched.next_long());
    }

    #[test]
    fn biased_to_bottom_height_follows_its_two_draws() {
        let provider = height(
            r#"{"type":"minecraft:biased_to_bottom","min_inclusive":{"absolute":0},
                "max_inclusive":{"absolute":20},"inner":4}"#,
        );
        let mut random = Xoroshiro128PlusPlus::new(17);
        let mut mirror = Xoroshiro128PlusPlus::new(17);
        // range = 20 - 0 - 4 + 1 = 17.
        let first = mirror.next_int_bounded(17);
        assert_eq!(sample_height(&provider, &mut random, BOUNDS), mirror.next_int_bounded(first + 4));
    }

    #[test]
    fn trapezoid_height_with_a_wide_plateau_is_uniform() {
        let provider = height(
            r#"{"type":"minecraft:trapezoid","min_inclusive":{"absolute":0},
                "max_inclusive":{"absolute":10},"plateau":10}"#,
        );
        let mut random = Xoroshiro128PlusPlus::new(2);
        let mut mirror = Xoroshiro128PlusPlus::new(2);
        assert_eq!(sample_height(&provider, &mut random, BOUNDS), mirror.next_int_bounded(11));
    }
}
