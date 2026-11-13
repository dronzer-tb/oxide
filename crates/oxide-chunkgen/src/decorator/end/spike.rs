//! `end_spike`: the ten obsidian pillars around the main End island, each topped by a crystal.
//!
//! The config's `spikes` list is empty in 26.2, which means the layout is derived from the world
//! seed: the low 16 bits of the seed's first `Random` long seed a shuffle of the sizes 0 to 9,
//! and pillar `i` stands on a circle of radius 42 at angle `2 * (-pi + pi / 10 * i)`, its radius
//! and height following its size. Only the pillar whose centre lies in the origin's chunk is
//! built. A pillar is an obsidian cylinder up to its height with everything above it (over y 65)
//! cleared, optionally caged in iron bars, with bedrock and fire where the crystal sits.
//!
//! The crystal is an entity, which the region cannot carry; it is tallied as
//! `entity:minecraft:end_crystal`. Its facing is still drawn (`nextFloat * 360`) so the random
//! stream stays vanilla's.
//!
//! The angle's `cos`/`sin` are the platform libm, not `Math.cos`; they agree to the last bit for
//! almost every input and the result is floored to whole blocks. PARITY-CHECK: compare the ten
//! pillar centres against a real 26.2 world's.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, LegacyRandom, RandomSource};
use oxide_datapack::end_feature::EndSpikeConfig;
use oxide_datapack::BlockFacts;

use super::super::context::FeatureContext;
use super::{default_state, with_property};

/// One pillar, in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spike {
    pub center_x: i32,
    pub center_z: i32,
    pub radius: i32,
    pub height: i32,
    pub guarded: bool,
}

/// `EndSpikeFeature.getSpikesForLevel`'s default layout for `seed`.
pub fn spikes_for_seed(seed: i64) -> Vec<Spike> {
    let key = LegacyRandom::new(seed).next_long() & 65535;
    let mut random = LegacyRandom::new(key);
    let mut sizes: Vec<i32> = (0..10).collect();
    // `Util.shuffle`.
    for end in (2..=sizes.len()).rev() {
        let other = random.next_int_bounded(end as i32) as usize;
        sizes.swap(end - 1, other);
    }
    (0..10)
        .map(|i| {
            let angle = 2.0 * (-std::f64::consts::PI + (std::f64::consts::PI / 10.0) * f64::from(i));
            let size = sizes[i as usize];
            Spike {
                center_x: (42.0 * angle.cos()).floor() as i32,
                center_z: (42.0 * angle.sin()).floor() as i32,
                radius: 2 + size / 3,
                height: 76 + size * 3,
                guarded: size == 1 || size == 2,
            }
        })
        .collect()
}

#[derive(Debug)]
pub struct EndSpikeFeature {
    /// The configured pillars; empty means "derive from the world seed".
    spikes: Vec<Spike>,
    obsidian: BlockState,
    air: BlockState,
    iron_bars: BlockState,
    bedrock: BlockState,
    fire: BlockState,
}

impl EndSpikeFeature {
    pub fn compile(config: &EndSpikeConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            spikes: config
                .spikes
                .iter()
                .map(|s| Spike {
                    center_x: s.center_x,
                    center_z: s.center_z,
                    radius: s.radius,
                    height: s.height,
                    guarded: s.guarded,
                })
                .collect(),
            obsidian: default_state(facts, "obsidian")?,
            air: default_state(facts, "air")?,
            iron_bars: default_state(facts, "iron_bars")?,
            bedrock: default_state(facts, "bedrock")?,
            fire: default_state(facts, "fire")?,
        })
    }

    /// `SpikeFeature.place`: builds every pillar centred in the origin's chunk.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let spikes = if self.spikes.is_empty() { spikes_for_seed(cx.env.seed) } else { self.spikes.clone() };
        for spike in spikes {
            if cx.origin.x >> 4 == spike.center_x >> 4 && cx.origin.z >> 4 == spike.center_z >> 4 {
                self.place_spike(cx, spike);
            }
        }
        true
    }

    fn bars(&self, north: bool, south: bool, west: bool, east: bool) -> BlockState {
        [("north", north), ("south", south), ("west", west), ("east", east)]
            .into_iter()
            .fold(self.iron_bars.clone(), |state, (name, on)| {
                with_property(state, name, if on { "true" } else { "false" })
            })
    }

    /// `SpikeFeature.placeSpike`.
    fn place_spike(&self, cx: &mut FeatureContext<'_, '_, '_>, spike: Spike) {
        let r = spike.radius;
        let (center_x, center_z) = (spike.center_x, spike.center_z);
        // `BlockPos.betweenClosed`: x fastest, then y, then z.
        for z in center_z - r..=center_z + r {
            for y in cx.region.min_y()..=spike.height + 10 {
                for x in center_x - r..=center_x + r {
                    let (dx, dz) = (f64::from(x - center_x), f64::from(z - center_z));
                    let pos = BlockPos::new(x, y, z);
                    if dx * dx + dz * dz <= f64::from(r * r + 1) && y < spike.height {
                        cx.region.set_block(pos, self.obsidian.clone());
                    } else if y > 65 {
                        cx.region.set_block(pos, self.air.clone());
                    }
                }
            }
        }
        if spike.guarded {
            for dx in -2..=2i32 {
                for dz in -2..=2i32 {
                    for dy in 0..=3i32 {
                        let x_side = dx.abs() == 2;
                        let z_side = dz.abs() == 2;
                        let top = dy == 3;
                        if x_side || z_side || top {
                            let x_edge = dx == -2 || dx == 2 || top;
                            let z_edge = dz == -2 || dz == 2 || top;
                            let state = self.bars(x_edge && dz != -2, x_edge && dz != 2, z_edge && dx != -2, z_edge && dx != 2);
                            cx.region.set_block(BlockPos::new(center_x + dx, spike.height + dy, center_z + dz), state);
                        }
                    }
                }
            }
        }
        // The crystal's yaw; the entity itself is not carried.
        cx.random.next_float();
        cx.unsupported.record("entity", "minecraft:end_crystal");
        cx.region.set_block(BlockPos::new(center_x, spike.height, center_z), self.bedrock.clone());
        cx.region.set_block(BlockPos::new(center_x, spike.height + 1, center_z), self.fire.clone());
    }
}
