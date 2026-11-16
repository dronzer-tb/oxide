//! Block state providers: how a feature picks the state it places at a position.
//!
//! Covers every provider type in the 26.2 configured-feature data except `rotated_block_provider`.
//! That one's draw order is not in the reference implementation (which targets a later version
//! whose provider takes a direction), so it answers [`Unsupported`] instead of a made-up block.
//! The only 26.2 feature that uses it is `block_pile`, which is not placed yet.
//!
//! The three noise providers are complete: they sample an `oxide_noise::NormalNoise` seeded
//! from a legacy random source with the provider's own `seed`, exactly as vanilla constructs it.

use std::borrow::Cow;
use std::str::FromStr;

use anyhow::{anyhow, bail, Result};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::feature::{BlockStateProvider, NoiseProviderParameters};
use oxide_datapack::placement::{BlockStateSpec, IntProvider};
use oxide_datapack::BlockFacts;
use oxide_noise::{NormalNoise, WorldRandom};

use super::context::FeatureContext;
use super::predicate::Predicate;
use super::provider::sample_int;

/// A provider that cannot produce a state, and why. Callers tally it and fail the placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unsupported(pub &'static str);

/// Builds the full state a data file names: the block's default state with the written
/// properties applied, which is what the game's codec produces for `{"Name": ...}`.
pub fn state_from_spec(spec: &BlockStateSpec, facts: &BlockFacts) -> Result<BlockState> {
    let name = ResourceLocation::from_str(&spec.name)
        .map_err(|_| anyhow!("invalid block id {:?}", spec.name))?;
    let mut state = BlockState::new(name);
    state.properties = spec.properties.clone();
    let state = facts.complete(&state);
    if !facts.knows(&state) {
        bail!("block state {state} is not in the exported block facts");
    }
    Ok(state)
}

#[derive(Debug, Clone)]
pub struct NoiseSource {
    noise: NormalNoise,
    scale: f32,
}

impl NoiseSource {
    fn new(seed: i64, parameters: &NoiseProviderParameters, scale: f32) -> Self {
        // `NormalNoise.create(new WorldgenRandom(new LegacyRandomSource(seed)), parameters)`.
        let mut random = WorldRandom::new(seed, true);
        Self {
            noise: NormalNoise::create(
                &mut random,
                parameters.first_octave,
                parameters.amplitudes.clone(),
            ),
            scale,
        }
    }

    /// `NoiseBasedStateProvider.getNoiseValue(pos, scale)`: the scale is a float field widened
    /// to double, so the multiplication happens in double precision.
    fn value(&self, pos: BlockPos) -> f64 {
        let scale = f64::from(self.scale);
        self.noise.get_value(
            f64::from(pos.x) * scale,
            f64::from(pos.y) * scale,
            f64::from(pos.z) * scale,
        )
    }

    /// `DualNoiseProvider.getSlowNoiseValue`: here the multiplication is `int * float`, done in
    /// single precision and only then widened.
    fn slow_value(&self, pos: BlockPos) -> f64 {
        self.noise.get_value(
            f64::from(pos.x as f32 * self.scale),
            f64::from(pos.y as f32 * self.scale),
            f64::from(pos.z as f32 * self.scale),
        )
    }
}

/// `NoiseBasedStateProvider.getRandomState`: maps noise in `[-1, 1]` onto the list.
///
/// PARITY-CHECK: computed in double precision. The reference implementation does this in single
/// precision, which would only matter within a rounding error of an index boundary; confirm
/// against a real 26.2 `noise_provider` before relying on exact flower placement.
fn pick_by_noise(len: usize, noise: f64) -> usize {
    let fraction = ((1.0 + noise) / 2.0).clamp(0.0, 0.9999);
    (fraction * len as f64) as usize
}

#[derive(Debug, Clone)]
pub enum StateProvider {
    Simple(BlockState),
    Weighted {
        entries: Vec<(BlockState, i32)>,
        total: i32,
    },
    RandomizedInt {
        source: Box<StateProvider>,
        property: String,
        values: IntProvider,
    },
    RuleBased {
        fallback: Option<Box<StateProvider>>,
        rules: Vec<(Predicate, StateProvider)>,
    },
    Noise {
        source: NoiseSource,
        states: Vec<BlockState>,
    },
    DualNoise {
        fast: NoiseSource,
        slow: NoiseSource,
        states: Vec<BlockState>,
        variety: [i32; 2],
    },
    NoiseThreshold {
        source: NoiseSource,
        threshold: f32,
        high_chance: f32,
        default: BlockState,
        low: Vec<BlockState>,
        high: Vec<BlockState>,
    },
    Unsupported(&'static str),
}

fn compile_states(specs: &[BlockStateSpec], facts: &BlockFacts) -> Result<Vec<BlockState>> {
    specs
        .iter()
        .map(|spec| state_from_spec(spec, facts))
        .collect()
}

impl StateProvider {
    pub fn compile(ir: &BlockStateProvider, facts: &BlockFacts) -> Result<Self> {
        Ok(match ir {
            BlockStateProvider::Simple { state } => Self::Simple(state_from_spec(state, facts)?),
            BlockStateProvider::RotatedBlock { .. } => Self::Unsupported("rotated_block_provider"),
            BlockStateProvider::Weighted { entries } => {
                let mut compiled = Vec::with_capacity(entries.len());
                let mut total = 0i32;
                for entry in entries {
                    compiled.push((state_from_spec(&entry.data, facts)?, entry.weight));
                    total = total.wrapping_add(entry.weight);
                }
                Self::Weighted {
                    entries: compiled,
                    total,
                }
            }
            BlockStateProvider::RandomizedInt {
                source,
                property,
                values,
            } => Self::RandomizedInt {
                source: Box::new(Self::compile(source, facts)?),
                property: property.clone(),
                values: values.clone(),
            },
            BlockStateProvider::RuleBased { fallback, rules } => Self::RuleBased {
                fallback: fallback
                    .as_ref()
                    .map(|fallback| Self::compile(fallback, facts).map(Box::new))
                    .transpose()?,
                rules: rules
                    .iter()
                    .map(|rule| {
                        Ok((
                            Predicate::compile(&rule.if_true, facts)?,
                            Self::compile(&rule.then, facts)?,
                        ))
                    })
                    .collect::<Result<_>>()?,
            },
            BlockStateProvider::Noise {
                seed,
                noise,
                scale,
                states,
            } => Self::Noise {
                source: NoiseSource::new(*seed, noise, *scale),
                states: compile_states(states, facts)?,
            },
            BlockStateProvider::DualNoise {
                seed,
                noise,
                scale,
                states,
                variety,
                slow_noise,
                slow_scale,
            } => Self::DualNoise {
                fast: NoiseSource::new(*seed, noise, *scale),
                slow: NoiseSource::new(*seed, slow_noise, *slow_scale),
                states: compile_states(states, facts)?,
                variety: *variety,
            },
            BlockStateProvider::NoiseThreshold {
                seed,
                noise,
                scale,
                threshold,
                high_chance,
                default_state,
                low_states,
                high_states,
            } => Self::NoiseThreshold {
                source: NoiseSource::new(*seed, noise, *scale),
                threshold: *threshold,
                high_chance: *high_chance,
                default: state_from_spec(default_state, facts)?,
                low: compile_states(low_states, facts)?,
                high: compile_states(high_states, facts)?,
            },
        })
    }

    /// `BlockStateProvider.getOptionalState`: the state to place at `pos`, `None` when a
    /// rule-based provider matched nothing and has no usable fallback.
    pub fn state(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
    ) -> Result<Option<BlockState>, Unsupported> {
        Ok(self.state_cow(cx, pos)?.map(Cow::into_owned))
    }

    /// [`Self::state`], borrowing the provider's own state where it names one rather than
    /// cloning it: same draws, same answer. A caller that only writes the state (with
    /// `set_block_ref`) then makes no copy at all.
    pub fn state_cow(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
    ) -> Result<Option<Cow<'_, BlockState>>, Unsupported> {
        Ok(match self {
            Self::Simple(state) => Some(Cow::Borrowed(state)),
            Self::Weighted { entries, total } => {
                if *total <= 0 {
                    return Ok(None);
                }
                // One bounded draw even for a single entry, as `WeightedList.getRandom` does.
                let mut remaining = cx.random.next_int_bounded(*total);
                let mut chosen = None;
                for (state, weight) in entries {
                    remaining -= weight;
                    if remaining < 0 {
                        chosen = Some(Cow::Borrowed(state));
                        break;
                    }
                }
                chosen
            }
            Self::RandomizedInt {
                source,
                property,
                values,
            } => {
                let Some(mut state) = source.state(cx, pos)? else {
                    return Ok(None);
                };
                // Only an integer property can be rolled; a state without one is left alone and
                // draws nothing.
                let is_integer = state
                    .properties
                    .get(property)
                    .is_some_and(|value| value.parse::<i32>().is_ok());
                if is_integer {
                    let value = sample_int(values, cx.random);
                    state.properties.insert(property.clone(), value.to_string());
                    if !cx.env.facts.knows(&state) {
                        return Err(Unsupported(
                            "randomized_int_state_provider value out of range",
                        ));
                    }
                }
                Some(Cow::Owned(state))
            }
            Self::RuleBased { fallback, rules } => {
                for (predicate, provider) in rules {
                    if predicate.test(cx, pos) {
                        if let Some(state) = provider.state_cow(cx, pos)? {
                            return Ok(Some(state));
                        }
                    }
                }
                match fallback {
                    Some(fallback) => fallback.state_cow(cx, pos)?,
                    None => None,
                }
            }
            Self::Noise { source, states } => {
                if states.is_empty() {
                    return Err(Unsupported("noise_provider without states"));
                }
                Some(Cow::Borrowed(
                    &states[pick_by_noise(states.len(), source.value(pos))],
                ))
            }
            Self::NoiseThreshold {
                source,
                threshold,
                high_chance,
                default,
                low,
                high,
            } => {
                if source.value(pos) < f64::from(*threshold) {
                    if low.is_empty() {
                        return Err(Unsupported("noise_threshold_provider without low states"));
                    }
                    Some(Cow::Borrowed(
                        &low[cx.random.next_int_bounded(low.len() as i32) as usize],
                    ))
                } else if cx.random.next_float() < *high_chance {
                    if high.is_empty() {
                        return Err(Unsupported("noise_threshold_provider without high states"));
                    }
                    Some(Cow::Borrowed(
                        &high[cx.random.next_int_bounded(high.len() as i32) as usize],
                    ))
                } else {
                    Some(Cow::Borrowed(default))
                }
            }
            Self::DualNoise {
                fast,
                slow,
                states,
                variety,
            } => {
                if states.is_empty() {
                    return Err(Unsupported("dual_noise_provider without states"));
                }
                // `Mth.clampedMap(slow, -1, 1, min, max + 1)`, truncated: how many distinct
                // candidates this position draws from.
                let fraction = (slow.slow_value(pos) + 1.0) / 2.0;
                let (low, high) = (f64::from(variety[0]), f64::from(variety[1] + 1));
                let count = (if fraction < 0.0 {
                    low
                } else if fraction > 1.0 {
                    high
                } else {
                    low + fraction * (high - low)
                }) as i32;
                if count <= 0 {
                    return Err(Unsupported("dual_noise_provider variety"));
                }
                // Vanilla builds all `count` candidates and then picks one by the fast noise;
                // each is a pure function of position, so only the picked one is computed.
                let picked = pick_by_noise(count as usize, fast.value(pos)) as i32;
                let shifted = BlockPos::new(
                    pos.x.wrapping_add(picked.wrapping_mul(54545)),
                    pos.y,
                    pos.z.wrapping_add(picked.wrapping_mul(34234)),
                );
                Some(Cow::Borrowed(
                    &states[pick_by_noise(states.len(), slow.slow_value(shifted))],
                ))
            }
            Self::Unsupported(why) => return Err(Unsupported(why)),
        })
    }
}
