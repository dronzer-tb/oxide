//! `iceberg`: a floating mass of ice (or blue ice) above and below the sea surface.
//!
//! One iceberg is either a rounded mass or an ellipse, chosen by a roll; both are built layer by
//! layer from a small set of random sizes (`over` blocks above the water, `under` below, `width`
//! across). Blocks above sea level may be snow. After the mass is stamped it is smoothed, and some
//! icebergs get a cut carved out of one side.
//!
//! The geometry is a direct port: every float and double is where vanilla keeps it, and the
//! short-circuited draws (a roll that only happens when an earlier test passes) stay
//! short-circuited, because the draw order is part of the result.
//!
//! PARITY-CHECK: the ellipse distance uses `f64::sin` and `f64::cos` where vanilla uses
//! `Math.sin` and `Math.cos`, the platform libm and not the `Mth` table. They agree to the last
//! bit for almost every angle; a mismatch would move a block on an ellipse's edge.

use std::f64::consts::{FRAC_PI_2, PI};

use anyhow::{Context, Result};
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::ground_feature::IcebergConfig;
use oxide_datapack::{BlockFact, BlockFacts};

use super::super::context::{block_in, FeatureContext};
use super::super::state_provider::state_from_spec;
use super::{default_state, is_air, is_block, offset};

#[derive(Debug)]
pub struct IcebergFeature {
    state: BlockState,
    air: BlockState,
    water: BlockState,
    snow_block: BlockState,
}

/// What every block of one iceberg is placed against.
struct Shape {
    origin: BlockPos,
    snow_on_top: bool,
    is_ellipse: bool,
    ellipse_c: i32,
    angle: f64,
}

fn pow2(value: i32) -> f64 {
    let value = f64::from(value);
    value * value
}

fn ceil(value: f32) -> i32 {
    f64::from(value).ceil() as i32
}

/// `IcebergFeature.signedDistanceEllipse`.
fn signed_ellipse(x: i32, z: i32, local: [i32; 3], a: i32, c: i32, angle: f64) -> f64 {
    let dx = f64::from(x - local[0]);
    let dz = f64::from(z - local[2]);
    let (sin, cos) = (angle.sin(), angle.cos());
    let along = (dx * cos - dz * sin) / f64::from(a);
    let across = (dx * sin + dz * cos) / f64::from(c);
    along * along + across * across - 1.0
}

/// `IcebergFeature.heightDependentRadiusRound`.
fn radius_round(random: &mut impl RandomSource, y: i32, height: i32, width: i32) -> i32 {
    let k = 3.5f32 - random.next_float();
    let mut scale = (1.0f32 - pow2(y) as f32 / (height as f32 * k)) * width as f32;
    if height > 15 + random.next_int_bounded(5) {
        let temp = if y < 3 + random.next_int_bounded(6) {
            y / 2
        } else {
            y
        };
        scale = (1.0f32 - temp as f32 / (height as f32 * k * 0.4f32)) * width as f32;
    }
    ceil(scale / 2.0)
}

/// `IcebergFeature.heightDependentRadiusSteep`.
fn radius_steep(random: &mut impl RandomSource, y: i32, height: i32, width: i32) -> i32 {
    let k = 1.0f32 + random.next_float() / 2.0;
    let scale = (1.0f32 - y as f32 / (height as f32 * k)) * width as f32;
    ceil(scale / 2.0)
}

impl IcebergFeature {
    pub fn compile(config: &IcebergConfig, facts: &BlockFacts) -> Result<Self> {
        Ok(Self {
            state: state_from_spec(&config.state, facts).context("iceberg state")?,
            air: default_state(facts, "air")?,
            water: default_state(facts, "water")?,
            snow_block: default_state(facts, "snow_block")?,
        })
    }

    /// Packed ice, snow block or blue ice: the blocks an iceberg is made of.
    fn is_iceberg_block(&self, state: &BlockState) -> bool {
        ["packed_ice", "snow_block", "blue_ice"]
            .iter()
            .any(|path| is_block(state, path))
    }

    /// `IcebergFeature.place`: the origin is moved to sea level before anything is drawn.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let origin = BlockPos::new(cx.origin.x, cx.env.sea_level, cx.origin.z);
        let snow_on_top = cx.random.next_double() > 0.7;
        let angle = cx.random.next_double() * 2.0 * PI;
        let ellipse_a = 11 - cx.random.next_int_bounded(5);
        let ellipse_c = 3 + cx.random.next_int_bounded(3);
        let is_ellipse = cx.random.next_double() > 0.7;
        let mut over = if is_ellipse {
            cx.random.next_int_bounded(6) + 6
        } else {
            cx.random.next_int_bounded(15) + 3
        };
        if !is_ellipse && cx.random.next_double() > 0.9 {
            over += cx.random.next_int_bounded(19) + 7;
        }
        let under = (over + cx.random.next_int_bounded(11)).min(18);
        let width = (over + cx.random.next_int_bounded(7) - cx.random.next_int_bounded(5)).min(11);
        let a = if is_ellipse { ellipse_a } else { 11 };
        let shape = Shape {
            origin,
            snow_on_top,
            is_ellipse,
            ellipse_c,
            angle,
        };

        for x in -a..a {
            for z in -a..a {
                for y in 0..over {
                    let radius = if is_ellipse {
                        ceil(
                            (1.0f32 - pow2(y) as f32 / (over as f32 * 1.0f32)) * width as f32 / 2.0,
                        )
                    } else {
                        radius_round(cx.random, y, over, width)
                    };
                    if is_ellipse || x < radius {
                        self.generate_block(cx, &shape, over, x, y, z, radius, a);
                    }
                }
            }
        }
        self.smooth(cx, origin, width, over, is_ellipse, ellipse_a);
        for x in -a..a {
            for z in -a..a {
                let mut y = -1;
                while y > -under {
                    let new_a = if is_ellipse {
                        ceil(a as f32 * (1.0f32 - pow2(y) as f32 / (under as f32 * 8.0f32)))
                    } else {
                        a
                    };
                    let radius = radius_steep(cx.random, -y, under, width);
                    if x < radius {
                        self.generate_block(cx, &shape, under, x, y, z, radius, new_a);
                    }
                    y -= 1;
                }
            }
        }
        let cut_out = if is_ellipse {
            cx.random.next_double() > 0.1
        } else {
            cx.random.next_double() > 0.7
        };
        if cut_out {
            self.cut_out(cx, &shape, width, over, ellipse_a);
        }
        true
    }

    /// `IcebergFeature.generateIcebergBlock`.
    #[allow(clippy::too_many_arguments)]
    fn generate_block(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        shape: &Shape,
        height: i32,
        x: i32,
        y: i32,
        z: i32,
        radius: i32,
        a: i32,
    ) {
        let signed = if shape.is_ellipse {
            let mut c = shape.ellipse_c;
            if y > 0 && height - y <= 3 {
                c -= 4 - (height - y);
            }
            signed_ellipse(x, z, [0; 3], a, c, shape.angle)
        } else {
            let off = 10.0f32 * cx.random.next_float().clamp(0.2, 0.8) / radius as f32;
            f64::from(off) + pow2(x) + pow2(z) - pow2(radius)
        };
        // `signedDist < 0.0` fails for NaN too: a zero-width ellipse layer at its centre.
        if signed.is_nan() || signed >= 0.0 {
            return;
        }
        let pos = offset(shape.origin, x, y, z);
        let compare = if shape.is_ellipse {
            -0.5
        } else {
            f64::from(-6 - cx.random.next_int_bounded(3))
        };
        if signed > compare && cx.random.next_double() > 0.9 {
            return;
        }
        let height_difference = height - y;
        let state = block_in(cx.region, pos);
        let replaceable = cx.env.facts.is(state, BlockFact::Air)
            || ["snow_block", "ice", "water"]
                .iter()
                .any(|path| is_block(state, path));
        if !replaceable {
            return;
        }
        let is_water = is_block(state, "water");
        let randomness = !shape.is_ellipse || cx.random.next_double() > 0.05;
        let divisor = if shape.is_ellipse { 3 } else { 2 };
        let snow = shape.snow_on_top
            && !is_water
            && f64::from(height_difference)
                <= f64::from(cx.random.next_int_bounded((height / divisor).max(1)))
                    + f64::from(height) * 0.6
            && randomness;
        let placed = if snow {
            self.snow_block.clone()
        } else {
            self.state.clone()
        };
        cx.region.set_block(pos, placed);
    }

    /// `IcebergFeature.smooth`: removes blocks that hang over air and blocks with three open
    /// sides.
    fn smooth(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        origin: BlockPos,
        width: i32,
        height: i32,
        is_ellipse: bool,
        ellipse_a: i32,
    ) {
        let a = if is_ellipse { ellipse_a } else { width / 2 };
        for x in -a..=a {
            for z in -a..=a {
                for y in 0..=height {
                    let pos = offset(origin, x, y, z);
                    let state = block_in(cx.region, pos);
                    let iceberg = self.is_iceberg_block(state);
                    if !(iceberg || is_block(state, "snow")) {
                        continue;
                    }
                    if is_air(cx, offset(pos, 0, -1, 0)) {
                        cx.region.set_block_ref(pos, &self.air);
                        cx.region.set_block_ref(offset(pos, 0, 1, 0), &self.air);
                    } else if iceberg {
                        let open = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                            .iter()
                            .filter(|(dx, dz)| {
                                !self
                                    .is_iceberg_block(block_in(cx.region, offset(pos, *dx, 0, *dz)))
                            })
                            .count();
                        if open >= 3 {
                            cx.region.set_block_ref(pos, &self.air);
                        }
                    }
                }
            }
        }
    }

    /// `IcebergFeature.carve`'s caller: one cut through the iceberg, a chasm above water that
    /// becomes a water-filled slot below it.
    fn cut_out(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        shape: &Shape,
        width: i32,
        height: i32,
        ellipse_a: i32,
    ) {
        let sign_x = if cx.random.next_boolean() { -1 } else { 1 };
        let sign_z = if cx.random.next_boolean() { -1 } else { 1 };
        let mut x_offset = cx.random.next_int_bounded((width / 2 - 2).max(1));
        if cx.random.next_boolean() {
            x_offset = width / 2 + 1 - cx.random.next_int_bounded((width - width / 2 - 1).max(1));
        }
        let mut z_offset = cx.random.next_int_bounded((width / 2 - 2).max(1));
        if cx.random.next_boolean() {
            z_offset = width / 2 + 1 - cx.random.next_int_bounded((width - width / 2 - 1).max(1));
        }
        if shape.is_ellipse {
            x_offset = cx.random.next_int_bounded((ellipse_a - 5).max(1));
            z_offset = x_offset;
        }
        let local = [sign_x * x_offset, 0, sign_z * z_offset];
        let angle = if shape.is_ellipse {
            shape.angle + FRAC_PI_2
        } else {
            cx.random.next_double() * 2.0 * PI
        };
        for y in 0..height - 3 {
            let radius = radius_round(cx.random, y, height, width);
            self.carve(cx, shape, radius, y, false, angle, local, ellipse_a);
        }
        let mut y = -1;
        while y > -height + cx.random.next_int_bounded(5) {
            let radius = radius_steep(cx.random, -y, height, width);
            self.carve(cx, shape, radius, y, true, angle, local, ellipse_a);
            y -= 1;
        }
    }

    /// `IcebergFeature.carve`.
    #[allow(clippy::too_many_arguments)]
    fn carve(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        shape: &Shape,
        radius: i32,
        y: i32,
        under_water: bool,
        angle: f64,
        local: [i32; 3],
        ellipse_a: i32,
    ) {
        let a = radius + 1 + ellipse_a / 3;
        let c = (radius - 3).min(3) + shape.ellipse_c / 2 - 1;
        for x in -a..a {
            for z in -a..a {
                if signed_ellipse(x, z, local, a, c, angle) < 0.0 {
                    let pos = offset(shape.origin, x, y, z);
                    let state = block_in(cx.region, pos);
                    if self.is_iceberg_block(state) {
                        if under_water {
                            cx.region.set_block_ref(pos, &self.water);
                        } else {
                            cx.region.set_block_ref(pos, &self.air);
                            let above = offset(pos, 0, 1, 0);
                            if is_block(block_in(cx.region, above), "snow") {
                                cx.region.set_block_ref(above, &self.air);
                            }
                        }
                    }
                }
            }
        }
    }
}
