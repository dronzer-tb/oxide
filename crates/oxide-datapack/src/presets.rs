//! The multi-noise biome parameter lists vanilla keeps in Java rather than in the datapack.
//!
//! A `minecraft:multi_noise` biome source usually reads `{"preset": "minecraft:overworld"}`,
//! and that preset's several hundred parameter points are built by
//! `net.minecraft.world.level.biome.OverworldBiomeBuilder` at runtime -- there is nothing in
//! `data/` to load. Without them every position falls back to one biome, which is what made a
//! whole world (and the nether) generate as `minecraft:plains`.
//!
//! Ported 2026-08-24 from decompiled 26.2 `OverworldBiomeBuilder` and
//! `MultiNoiseBiomeSourceParameterList.Preset`. The structure is kept deliberately close to
//! the Java -- same method names, same iteration order, same `pick*` helpers -- so it can be
//! diffed against the source it came from when the game updates.

use oxide_core::ResourceLocation;

use crate::climate::ClimateParam;
use crate::multi_noise::{ClimateParameters, MultiNoiseBiomeEntry};

/// Parameter points for a named preset, or `None` if the id is not one vanilla hardcodes.
pub fn preset_entries(preset: &ResourceLocation) -> Option<Vec<MultiNoiseBiomeEntry>> {
    match (preset.namespace(), preset.path()) {
        ("minecraft", "overworld") => Some(OverworldBiomeBuilder::default().build()),
        ("minecraft", "nether") => Some(nether_entries()),
        _ => None,
    }
}

/// Vanilla's `Climate.Parameter`: an inclusive range. A "point" is a zero-width one.
#[derive(Debug, Clone, Copy)]
struct Param {
    min: f32,
    max: f32,
}

impl Param {
    fn span(min: f32, max: f32) -> Self {
        Self { min, max }
    }

    fn point(value: f32) -> Self {
        Self {
            min: value,
            max: value,
        }
    }

    /// `Climate.Parameter.span(Parameter, Parameter)`: the range covering both.
    fn join(low: Param, high: Param) -> Self {
        Self {
            min: low.min,
            max: high.max,
        }
    }
}

impl From<Param> for ClimateParam {
    fn from(p: Param) -> Self {
        ClimateParam::Range([p.min, p.max])
    }
}

fn biome(id: &str) -> ResourceLocation {
    ResourceLocation::minecraft(id)
}

#[allow(clippy::too_many_arguments)]
fn entry(
    temperature: Param,
    humidity: Param,
    continentalness: Param,
    erosion: Param,
    depth: Param,
    weirdness: Param,
    offset: f32,
    id: &str,
) -> MultiNoiseBiomeEntry {
    MultiNoiseBiomeEntry {
        biome: biome(id),
        parameters: ClimateParameters {
            temperature: temperature.into(),
            humidity: humidity.into(),
            continentalness: continentalness.into(),
            erosion: erosion.into(),
            depth: depth.into(),
            weirdness: weirdness.into(),
            offset,
        },
    }
}

/// `MultiNoiseBiomeSourceParameterList.Preset.NETHER` -- five points, all of them exact
/// values rather than ranges.
fn nether_entries() -> Vec<MultiNoiseBiomeEntry> {
    let p = Param::point;
    vec![
        entry(
            p(0.0),
            p(0.0),
            p(0.0),
            p(0.0),
            p(0.0),
            p(0.0),
            0.0,
            "nether_wastes",
        ),
        entry(
            p(0.0),
            p(-0.5),
            p(0.0),
            p(0.0),
            p(0.0),
            p(0.0),
            0.0,
            "soul_sand_valley",
        ),
        entry(
            p(0.4),
            p(0.0),
            p(0.0),
            p(0.0),
            p(0.0),
            p(0.0),
            0.0,
            "crimson_forest",
        ),
        entry(
            p(0.0),
            p(0.5),
            p(0.0),
            p(0.0),
            p(0.0),
            p(0.0),
            0.375,
            "warped_forest",
        ),
        entry(
            p(-0.5),
            p(0.0),
            p(0.0),
            p(0.0),
            p(0.0),
            p(0.0),
            0.175,
            "basalt_deltas",
        ),
    ]
}

/// Field-for-field port of `OverworldBiomeBuilder`. Names match the Java so the two can be
/// compared side by side; the `debug` path (`addDebugBiomes`) is not ported, since it only
/// runs under a hardcoded `SharedConstants` debug flag that is off in a shipped server.
struct OverworldBiomeBuilder {
    full_range: Param,
    temperatures: [Param; 5],
    humidities: [Param; 5],
    erosions: [Param; 7],
    frozen_range: Param,
    unfrozen_range: Param,
    mushroom_fields_continentalness: Param,
    deep_ocean_continentalness: Param,
    ocean_continentalness: Param,
    coast_continentalness: Param,
    inland_continentalness: Param,
    near_inland_continentalness: Param,
    mid_inland_continentalness: Param,
    far_inland_continentalness: Param,
    oceans: [[&'static str; 5]; 2],
    middle_biomes: [[&'static str; 5]; 5],
    middle_biomes_variant: [[Option<&'static str>; 5]; 5],
    plateau_biomes: [[&'static str; 5]; 5],
    plateau_biomes_variant: [[Option<&'static str>; 5]; 5],
    shattered_biomes: [[Option<&'static str>; 5]; 5],
}

impl Default for OverworldBiomeBuilder {
    fn default() -> Self {
        let temperatures = [
            Param::span(-1.0, -0.45),
            Param::span(-0.45, -0.15),
            Param::span(-0.15, 0.2),
            Param::span(0.2, 0.55),
            Param::span(0.55, 1.0),
        ];
        Self {
            full_range: Param::span(-1.0, 1.0),
            temperatures,
            humidities: [
                Param::span(-1.0, -0.35),
                Param::span(-0.35, -0.1),
                Param::span(-0.1, 0.1),
                Param::span(0.1, 0.3),
                Param::span(0.3, 1.0),
            ],
            erosions: [
                Param::span(-1.0, -0.78),
                Param::span(-0.78, -0.375),
                Param::span(-0.375, -0.2225),
                Param::span(-0.2225, 0.05),
                Param::span(0.05, 0.45),
                Param::span(0.45, 0.55),
                Param::span(0.55, 1.0),
            ],
            frozen_range: temperatures[0],
            unfrozen_range: Param::join(temperatures[1], temperatures[4]),
            mushroom_fields_continentalness: Param::span(-1.2, -1.05),
            deep_ocean_continentalness: Param::span(-1.05, -0.455),
            ocean_continentalness: Param::span(-0.455, -0.19),
            coast_continentalness: Param::span(-0.19, -0.11),
            inland_continentalness: Param::span(-0.11, 0.55),
            near_inland_continentalness: Param::span(-0.11, 0.03),
            mid_inland_continentalness: Param::span(0.03, 0.3),
            far_inland_continentalness: Param::span(0.3, 1.0),
            oceans: [
                [
                    "deep_frozen_ocean",
                    "deep_cold_ocean",
                    "deep_ocean",
                    "deep_lukewarm_ocean",
                    "warm_ocean",
                ],
                [
                    "frozen_ocean",
                    "cold_ocean",
                    "ocean",
                    "lukewarm_ocean",
                    "warm_ocean",
                ],
            ],
            middle_biomes: [
                [
                    "snowy_plains",
                    "snowy_plains",
                    "snowy_plains",
                    "snowy_taiga",
                    "taiga",
                ],
                [
                    "plains",
                    "plains",
                    "forest",
                    "taiga",
                    "old_growth_spruce_taiga",
                ],
                [
                    "flower_forest",
                    "plains",
                    "forest",
                    "birch_forest",
                    "dark_forest",
                ],
                ["savanna", "savanna", "forest", "jungle", "jungle"],
                ["desert", "desert", "desert", "desert", "desert"],
            ],
            middle_biomes_variant: [
                [Some("ice_spikes"), None, Some("snowy_taiga"), None, None],
                [None, None, None, None, Some("old_growth_pine_taiga")],
                [
                    Some("sunflower_plains"),
                    None,
                    None,
                    Some("old_growth_birch_forest"),
                    None,
                ],
                [
                    None,
                    None,
                    Some("plains"),
                    Some("sparse_jungle"),
                    Some("bamboo_jungle"),
                ],
                [None, None, None, None, None],
            ],
            plateau_biomes: [
                [
                    "snowy_plains",
                    "snowy_plains",
                    "snowy_plains",
                    "snowy_taiga",
                    "snowy_taiga",
                ],
                [
                    "meadow",
                    "meadow",
                    "forest",
                    "taiga",
                    "old_growth_spruce_taiga",
                ],
                ["meadow", "meadow", "meadow", "meadow", "pale_garden"],
                [
                    "savanna_plateau",
                    "savanna_plateau",
                    "forest",
                    "forest",
                    "jungle",
                ],
                [
                    "badlands",
                    "badlands",
                    "badlands",
                    "wooded_badlands",
                    "wooded_badlands",
                ],
            ],
            plateau_biomes_variant: [
                [Some("ice_spikes"), None, None, None, None],
                [
                    Some("cherry_grove"),
                    None,
                    Some("meadow"),
                    Some("meadow"),
                    Some("old_growth_pine_taiga"),
                ],
                [
                    Some("cherry_grove"),
                    Some("cherry_grove"),
                    Some("forest"),
                    Some("birch_forest"),
                    None,
                ],
                [None, None, None, None, None],
                [
                    Some("eroded_badlands"),
                    Some("eroded_badlands"),
                    None,
                    None,
                    None,
                ],
            ],
            shattered_biomes: [
                [
                    Some("windswept_gravelly_hills"),
                    Some("windswept_gravelly_hills"),
                    Some("windswept_hills"),
                    Some("windswept_forest"),
                    Some("windswept_forest"),
                ],
                [
                    Some("windswept_gravelly_hills"),
                    Some("windswept_gravelly_hills"),
                    Some("windswept_hills"),
                    Some("windswept_forest"),
                    Some("windswept_forest"),
                ],
                [
                    Some("windswept_hills"),
                    Some("windswept_hills"),
                    Some("windswept_hills"),
                    Some("windswept_forest"),
                    Some("windswept_forest"),
                ],
                [None, None, None, None, None],
                [None, None, None, None, None],
            ],
        }
    }
}

impl OverworldBiomeBuilder {
    fn build(&self) -> Vec<MultiNoiseBiomeEntry> {
        let mut out = Vec::new();
        self.add_off_coast_biomes(&mut out);
        self.add_inland_biomes(&mut out);
        self.add_underground_biomes(&mut out);
        out
    }

    /// `addSurfaceBiome`: every surface point is emitted twice, once at each end of the depth
    /// range, so a column matches it whether it is being sampled at the surface or below it.
    #[allow(clippy::too_many_arguments)]
    fn add_surface_biome(
        &self,
        out: &mut Vec<MultiNoiseBiomeEntry>,
        temperature: Param,
        humidity: Param,
        continentalness: Param,
        erosion: Param,
        weirdness: Param,
        offset: f32,
        id: &str,
    ) {
        for depth in [Param::point(0.0), Param::point(1.0)] {
            out.push(entry(
                temperature,
                humidity,
                continentalness,
                erosion,
                depth,
                weirdness,
                offset,
                id,
            ));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_underground_biome(
        &self,
        out: &mut Vec<MultiNoiseBiomeEntry>,
        temperature: Param,
        humidity: Param,
        continentalness: Param,
        erosion: Param,
        weirdness: Param,
        offset: f32,
        id: &str,
    ) {
        out.push(entry(
            temperature,
            humidity,
            continentalness,
            erosion,
            Param::span(0.2, 0.9),
            weirdness,
            offset,
            id,
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn add_bottom_biome(
        &self,
        out: &mut Vec<MultiNoiseBiomeEntry>,
        temperature: Param,
        humidity: Param,
        continentalness: Param,
        erosion: Param,
        weirdness: Param,
        offset: f32,
        id: &str,
    ) {
        out.push(entry(
            temperature,
            humidity,
            continentalness,
            erosion,
            Param::point(1.1),
            weirdness,
            offset,
            id,
        ));
    }

    fn add_off_coast_biomes(&self, out: &mut Vec<MultiNoiseBiomeEntry>) {
        self.add_surface_biome(
            out,
            self.full_range,
            self.full_range,
            self.mushroom_fields_continentalness,
            self.full_range,
            self.full_range,
            0.0,
            "mushroom_fields",
        );
        for t in 0..self.temperatures.len() {
            let temperature = self.temperatures[t];
            self.add_surface_biome(
                out,
                temperature,
                self.full_range,
                self.deep_ocean_continentalness,
                self.full_range,
                self.full_range,
                0.0,
                self.oceans[0][t],
            );
            self.add_surface_biome(
                out,
                temperature,
                self.full_range,
                self.ocean_continentalness,
                self.full_range,
                self.full_range,
                0.0,
                self.oceans[1][t],
            );
        }
    }

    /// The weirdness axis is cut into 13 bands, each handed to the slice that shapes it.
    fn add_inland_biomes(&self, out: &mut Vec<MultiNoiseBiomeEntry>) {
        self.add_mid_slice(out, Param::span(-1.0, -0.933_333_34));
        self.add_high_slice(out, Param::span(-0.933_333_34, -0.766_666_7));
        self.add_peaks(out, Param::span(-0.766_666_7, -0.566_666_66));
        self.add_high_slice(out, Param::span(-0.566_666_66, -0.4));
        self.add_mid_slice(out, Param::span(-0.4, -0.266_666_68));
        self.add_low_slice(out, Param::span(-0.266_666_68, -0.05));
        self.add_valleys(out, Param::span(-0.05, 0.05));
        self.add_low_slice(out, Param::span(0.05, 0.266_666_68));
        self.add_mid_slice(out, Param::span(0.266_666_68, 0.4));
        self.add_high_slice(out, Param::span(0.4, 0.566_666_66));
        self.add_peaks(out, Param::span(0.566_666_66, 0.766_666_7));
        self.add_high_slice(out, Param::span(0.766_666_7, 0.933_333_34));
        self.add_mid_slice(out, Param::span(0.933_333_34, 1.0));
    }

    fn add_peaks(&self, out: &mut Vec<MultiNoiseBiomeEntry>, weirdness: Param) {
        for t in 0..self.temperatures.len() {
            let temperature = self.temperatures[t];
            for h in 0..self.humidities.len() {
                let humidity = self.humidities[h];
                let middle = self.pick_middle_biome(t, h, weirdness);
                let middle_or_badlands = self.pick_middle_biome_or_badlands_if_hot(t, h, weirdness);
                let middle_or_badlands_or_slope =
                    self.pick_middle_biome_or_badlands_if_hot_or_slope_if_cold(t, h, weirdness);
                let plateau = self.pick_plateau_biome(t, h, weirdness);
                let shattered = self.pick_shattered_biome(t, h, weirdness);
                let shattered_or_savanna =
                    self.maybe_pick_windswept_savanna_biome(t, h, weirdness, shattered);
                let peak = self.pick_peak_biome(t, h, weirdness);

                let coast_to_far =
                    Param::join(self.coast_continentalness, self.far_inland_continentalness);
                let coast_to_near =
                    Param::join(self.coast_continentalness, self.near_inland_continentalness);
                let mid_to_far = Param::join(
                    self.mid_inland_continentalness,
                    self.far_inland_continentalness,
                );

                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_far,
                    self.erosions[0],
                    weirdness,
                    0.0,
                    peak,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_near,
                    self.erosions[1],
                    weirdness,
                    0.0,
                    middle_or_badlands_or_slope,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    self.erosions[1],
                    weirdness,
                    0.0,
                    peak,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_near,
                    Param::join(self.erosions[2], self.erosions[3]),
                    weirdness,
                    0.0,
                    middle,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    self.erosions[2],
                    weirdness,
                    0.0,
                    plateau,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.mid_inland_continentalness,
                    self.erosions[3],
                    weirdness,
                    0.0,
                    middle_or_badlands,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.far_inland_continentalness,
                    self.erosions[3],
                    weirdness,
                    0.0,
                    plateau,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_far,
                    self.erosions[4],
                    weirdness,
                    0.0,
                    middle,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_near,
                    self.erosions[5],
                    weirdness,
                    0.0,
                    shattered_or_savanna,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    self.erosions[5],
                    weirdness,
                    0.0,
                    shattered,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_far,
                    self.erosions[6],
                    weirdness,
                    0.0,
                    middle,
                );
            }
        }
    }

    fn add_high_slice(&self, out: &mut Vec<MultiNoiseBiomeEntry>, weirdness: Param) {
        for t in 0..self.temperatures.len() {
            let temperature = self.temperatures[t];
            for h in 0..self.humidities.len() {
                let humidity = self.humidities[h];
                let middle = self.pick_middle_biome(t, h, weirdness);
                let middle_or_badlands = self.pick_middle_biome_or_badlands_if_hot(t, h, weirdness);
                let middle_or_badlands_or_slope =
                    self.pick_middle_biome_or_badlands_if_hot_or_slope_if_cold(t, h, weirdness);
                let plateau = self.pick_plateau_biome(t, h, weirdness);
                let shattered = self.pick_shattered_biome(t, h, weirdness);
                let middle_or_savanna =
                    self.maybe_pick_windswept_savanna_biome(t, h, weirdness, middle);
                let slope = self.pick_slope_biome(t, h, weirdness);
                let peak = self.pick_peak_biome(t, h, weirdness);

                let coast_to_far =
                    Param::join(self.coast_continentalness, self.far_inland_continentalness);
                let coast_to_near =
                    Param::join(self.coast_continentalness, self.near_inland_continentalness);
                let mid_to_far = Param::join(
                    self.mid_inland_continentalness,
                    self.far_inland_continentalness,
                );

                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.coast_continentalness,
                    Param::join(self.erosions[0], self.erosions[1]),
                    weirdness,
                    0.0,
                    middle,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.near_inland_continentalness,
                    self.erosions[0],
                    weirdness,
                    0.0,
                    slope,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    self.erosions[0],
                    weirdness,
                    0.0,
                    peak,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.near_inland_continentalness,
                    self.erosions[1],
                    weirdness,
                    0.0,
                    middle_or_badlands_or_slope,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    self.erosions[1],
                    weirdness,
                    0.0,
                    slope,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_near,
                    Param::join(self.erosions[2], self.erosions[3]),
                    weirdness,
                    0.0,
                    middle,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    self.erosions[2],
                    weirdness,
                    0.0,
                    plateau,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.mid_inland_continentalness,
                    self.erosions[3],
                    weirdness,
                    0.0,
                    middle_or_badlands,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.far_inland_continentalness,
                    self.erosions[3],
                    weirdness,
                    0.0,
                    plateau,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_far,
                    self.erosions[4],
                    weirdness,
                    0.0,
                    middle,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_near,
                    self.erosions[5],
                    weirdness,
                    0.0,
                    middle_or_savanna,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    self.erosions[5],
                    weirdness,
                    0.0,
                    shattered,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_far,
                    self.erosions[6],
                    weirdness,
                    0.0,
                    middle,
                );
            }
        }
    }

    fn add_mid_slice(&self, out: &mut Vec<MultiNoiseBiomeEntry>, weirdness: Param) {
        let near_to_far = Param::join(
            self.near_inland_continentalness,
            self.far_inland_continentalness,
        );
        self.add_surface_biome(
            out,
            self.full_range,
            self.full_range,
            self.coast_continentalness,
            Param::join(self.erosions[0], self.erosions[2]),
            weirdness,
            0.0,
            "stony_shore",
        );
        self.add_surface_biome(
            out,
            Param::join(self.temperatures[1], self.temperatures[2]),
            self.full_range,
            near_to_far,
            self.erosions[6],
            weirdness,
            0.0,
            "swamp",
        );
        self.add_surface_biome(
            out,
            Param::join(self.temperatures[3], self.temperatures[4]),
            self.full_range,
            near_to_far,
            self.erosions[6],
            weirdness,
            0.0,
            "mangrove_swamp",
        );

        for t in 0..self.temperatures.len() {
            let temperature = self.temperatures[t];
            for h in 0..self.humidities.len() {
                let humidity = self.humidities[h];
                let middle = self.pick_middle_biome(t, h, weirdness);
                let middle_or_badlands = self.pick_middle_biome_or_badlands_if_hot(t, h, weirdness);
                let middle_or_badlands_or_slope =
                    self.pick_middle_biome_or_badlands_if_hot_or_slope_if_cold(t, h, weirdness);
                let shattered = self.pick_shattered_biome(t, h, weirdness);
                let plateau = self.pick_plateau_biome(t, h, weirdness);
                let beach = self.pick_beach_biome(t);
                let middle_or_savanna =
                    self.maybe_pick_windswept_savanna_biome(t, h, weirdness, middle);
                let shattered_coast = self.pick_shattered_coast_biome(t, h, weirdness);
                let slope = self.pick_slope_biome(t, h, weirdness);

                let coast_to_near =
                    Param::join(self.coast_continentalness, self.near_inland_continentalness);
                let coast_to_far =
                    Param::join(self.coast_continentalness, self.far_inland_continentalness);
                let near_to_mid = Param::join(
                    self.near_inland_continentalness,
                    self.mid_inland_continentalness,
                );
                let mid_to_far = Param::join(
                    self.mid_inland_continentalness,
                    self.far_inland_continentalness,
                );

                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    near_to_far,
                    self.erosions[0],
                    weirdness,
                    0.0,
                    slope,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    near_to_mid,
                    self.erosions[1],
                    weirdness,
                    0.0,
                    middle_or_badlands_or_slope,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.far_inland_continentalness,
                    self.erosions[1],
                    weirdness,
                    0.0,
                    if t == 0 { slope } else { plateau },
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.near_inland_continentalness,
                    self.erosions[2],
                    weirdness,
                    0.0,
                    middle,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.mid_inland_continentalness,
                    self.erosions[2],
                    weirdness,
                    0.0,
                    middle_or_badlands,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.far_inland_continentalness,
                    self.erosions[2],
                    weirdness,
                    0.0,
                    plateau,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    coast_to_near,
                    self.erosions[3],
                    weirdness,
                    0.0,
                    middle,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    self.erosions[3],
                    weirdness,
                    0.0,
                    middle_or_badlands,
                );
                if weirdness.max < 0.0 {
                    self.add_surface_biome(
                        out,
                        temperature,
                        humidity,
                        self.coast_continentalness,
                        self.erosions[4],
                        weirdness,
                        0.0,
                        beach,
                    );
                    self.add_surface_biome(
                        out,
                        temperature,
                        humidity,
                        near_to_far,
                        self.erosions[4],
                        weirdness,
                        0.0,
                        middle,
                    );
                } else {
                    self.add_surface_biome(
                        out,
                        temperature,
                        humidity,
                        coast_to_far,
                        self.erosions[4],
                        weirdness,
                        0.0,
                        middle,
                    );
                }
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.coast_continentalness,
                    self.erosions[5],
                    weirdness,
                    0.0,
                    shattered_coast,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.near_inland_continentalness,
                    self.erosions[5],
                    weirdness,
                    0.0,
                    middle_or_savanna,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    self.erosions[5],
                    weirdness,
                    0.0,
                    shattered,
                );
                if weirdness.max < 0.0 {
                    self.add_surface_biome(
                        out,
                        temperature,
                        humidity,
                        self.coast_continentalness,
                        self.erosions[6],
                        weirdness,
                        0.0,
                        beach,
                    );
                } else {
                    self.add_surface_biome(
                        out,
                        temperature,
                        humidity,
                        self.coast_continentalness,
                        self.erosions[6],
                        weirdness,
                        0.0,
                        middle,
                    );
                }
                if t == 0 {
                    self.add_surface_biome(
                        out,
                        temperature,
                        humidity,
                        near_to_far,
                        self.erosions[6],
                        weirdness,
                        0.0,
                        middle,
                    );
                }
            }
        }
    }

    fn add_low_slice(&self, out: &mut Vec<MultiNoiseBiomeEntry>, weirdness: Param) {
        let near_to_far = Param::join(
            self.near_inland_continentalness,
            self.far_inland_continentalness,
        );
        self.add_surface_biome(
            out,
            self.full_range,
            self.full_range,
            self.coast_continentalness,
            Param::join(self.erosions[0], self.erosions[2]),
            weirdness,
            0.0,
            "stony_shore",
        );
        self.add_surface_biome(
            out,
            Param::join(self.temperatures[1], self.temperatures[2]),
            self.full_range,
            near_to_far,
            self.erosions[6],
            weirdness,
            0.0,
            "swamp",
        );
        self.add_surface_biome(
            out,
            Param::join(self.temperatures[3], self.temperatures[4]),
            self.full_range,
            near_to_far,
            self.erosions[6],
            weirdness,
            0.0,
            "mangrove_swamp",
        );

        for t in 0..self.temperatures.len() {
            let temperature = self.temperatures[t];
            for h in 0..self.humidities.len() {
                let humidity = self.humidities[h];
                let middle = self.pick_middle_biome(t, h, weirdness);
                let middle_or_badlands = self.pick_middle_biome_or_badlands_if_hot(t, h, weirdness);
                let middle_or_badlands_or_slope =
                    self.pick_middle_biome_or_badlands_if_hot_or_slope_if_cold(t, h, weirdness);
                let beach = self.pick_beach_biome(t);
                let middle_or_savanna =
                    self.maybe_pick_windswept_savanna_biome(t, h, weirdness, middle);
                let shattered_coast = self.pick_shattered_coast_biome(t, h, weirdness);

                let mid_to_far = Param::join(
                    self.mid_inland_continentalness,
                    self.far_inland_continentalness,
                );

                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.near_inland_continentalness,
                    Param::join(self.erosions[0], self.erosions[1]),
                    weirdness,
                    0.0,
                    middle_or_badlands,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    Param::join(self.erosions[0], self.erosions[1]),
                    weirdness,
                    0.0,
                    middle_or_badlands_or_slope,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.near_inland_continentalness,
                    Param::join(self.erosions[2], self.erosions[3]),
                    weirdness,
                    0.0,
                    middle,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    Param::join(self.erosions[2], self.erosions[3]),
                    weirdness,
                    0.0,
                    middle_or_badlands,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.coast_continentalness,
                    Param::join(self.erosions[3], self.erosions[4]),
                    weirdness,
                    0.0,
                    beach,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    near_to_far,
                    self.erosions[4],
                    weirdness,
                    0.0,
                    middle,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.coast_continentalness,
                    self.erosions[5],
                    weirdness,
                    0.0,
                    shattered_coast,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.near_inland_continentalness,
                    self.erosions[5],
                    weirdness,
                    0.0,
                    middle_or_savanna,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    mid_to_far,
                    self.erosions[5],
                    weirdness,
                    0.0,
                    middle,
                );
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    self.coast_continentalness,
                    self.erosions[6],
                    weirdness,
                    0.0,
                    beach,
                );
                if t == 0 {
                    self.add_surface_biome(
                        out,
                        temperature,
                        humidity,
                        near_to_far,
                        self.erosions[6],
                        weirdness,
                        0.0,
                        middle,
                    );
                }
            }
        }
    }

    fn add_valleys(&self, out: &mut Vec<MultiNoiseBiomeEntry>, weirdness: Param) {
        let cold = weirdness.max < 0.0;
        let coast_to_far = Param::join(self.coast_continentalness, self.far_inland_continentalness);
        let inland_to_far =
            Param::join(self.inland_continentalness, self.far_inland_continentalness);
        let e01 = Param::join(self.erosions[0], self.erosions[1]);
        let e25 = Param::join(self.erosions[2], self.erosions[5]);

        self.add_surface_biome(
            out,
            self.frozen_range,
            self.full_range,
            self.coast_continentalness,
            e01,
            weirdness,
            0.0,
            if cold { "stony_shore" } else { "frozen_river" },
        );
        self.add_surface_biome(
            out,
            self.unfrozen_range,
            self.full_range,
            self.coast_continentalness,
            e01,
            weirdness,
            0.0,
            if cold { "stony_shore" } else { "river" },
        );
        self.add_surface_biome(
            out,
            self.frozen_range,
            self.full_range,
            self.near_inland_continentalness,
            e01,
            weirdness,
            0.0,
            "frozen_river",
        );
        self.add_surface_biome(
            out,
            self.unfrozen_range,
            self.full_range,
            self.near_inland_continentalness,
            e01,
            weirdness,
            0.0,
            "river",
        );
        self.add_surface_biome(
            out,
            self.frozen_range,
            self.full_range,
            coast_to_far,
            e25,
            weirdness,
            0.0,
            "frozen_river",
        );
        self.add_surface_biome(
            out,
            self.unfrozen_range,
            self.full_range,
            coast_to_far,
            e25,
            weirdness,
            0.0,
            "river",
        );
        self.add_surface_biome(
            out,
            self.frozen_range,
            self.full_range,
            self.coast_continentalness,
            self.erosions[6],
            weirdness,
            0.0,
            "frozen_river",
        );
        self.add_surface_biome(
            out,
            self.unfrozen_range,
            self.full_range,
            self.coast_continentalness,
            self.erosions[6],
            weirdness,
            0.0,
            "river",
        );
        self.add_surface_biome(
            out,
            Param::join(self.temperatures[1], self.temperatures[2]),
            self.full_range,
            inland_to_far,
            self.erosions[6],
            weirdness,
            0.0,
            "swamp",
        );
        self.add_surface_biome(
            out,
            Param::join(self.temperatures[3], self.temperatures[4]),
            self.full_range,
            inland_to_far,
            self.erosions[6],
            weirdness,
            0.0,
            "mangrove_swamp",
        );
        self.add_surface_biome(
            out,
            self.frozen_range,
            self.full_range,
            inland_to_far,
            self.erosions[6],
            weirdness,
            0.0,
            "frozen_river",
        );

        for t in 0..self.temperatures.len() {
            let temperature = self.temperatures[t];
            for h in 0..self.humidities.len() {
                let humidity = self.humidities[h];
                let middle_or_badlands = self.pick_middle_biome_or_badlands_if_hot(t, h, weirdness);
                self.add_surface_biome(
                    out,
                    temperature,
                    humidity,
                    Param::join(
                        self.mid_inland_continentalness,
                        self.far_inland_continentalness,
                    ),
                    Param::join(self.erosions[0], self.erosions[1]),
                    weirdness,
                    0.0,
                    middle_or_badlands,
                );
            }
        }
    }

    fn add_underground_biomes(&self, out: &mut Vec<MultiNoiseBiomeEntry>) {
        self.add_underground_biome(
            out,
            self.full_range,
            self.full_range,
            Param::span(0.8, 1.0),
            self.full_range,
            self.full_range,
            0.0,
            "dripstone_caves",
        );
        self.add_underground_biome(
            out,
            self.full_range,
            Param::span(0.7, 1.0),
            self.full_range,
            self.full_range,
            self.full_range,
            0.0,
            "lush_caves",
        );
        self.add_underground_biome(
            out,
            self.full_range,
            self.full_range,
            Param::join(self.coast_continentalness, self.inland_continentalness),
            Param::join(self.erosions[5], self.erosions[6]),
            Param::span(-1.1, -0.85),
            0.0,
            "sulfur_caves",
        );
        self.add_bottom_biome(
            out,
            self.full_range,
            self.full_range,
            self.full_range,
            Param::join(self.erosions[0], self.erosions[1]),
            self.full_range,
            0.0,
            "deep_dark",
        );
    }

    fn pick_middle_biome(&self, t: usize, h: usize, weirdness: Param) -> &'static str {
        if weirdness.max < 0.0 {
            return self.middle_biomes[t][h];
        }
        self.middle_biomes_variant[t][h].unwrap_or(self.middle_biomes[t][h])
    }

    fn pick_middle_biome_or_badlands_if_hot(
        &self,
        t: usize,
        h: usize,
        weirdness: Param,
    ) -> &'static str {
        if t == 4 {
            self.pick_badlands_biome(h, weirdness)
        } else {
            self.pick_middle_biome(t, h, weirdness)
        }
    }

    fn pick_middle_biome_or_badlands_if_hot_or_slope_if_cold(
        &self,
        t: usize,
        h: usize,
        weirdness: Param,
    ) -> &'static str {
        if t == 0 {
            self.pick_slope_biome(t, h, weirdness)
        } else {
            self.pick_middle_biome_or_badlands_if_hot(t, h, weirdness)
        }
    }

    fn maybe_pick_windswept_savanna_biome(
        &self,
        t: usize,
        h: usize,
        weirdness: Param,
        underlying: &'static str,
    ) -> &'static str {
        if t > 1 && h < 4 && weirdness.max >= 0.0 {
            "windswept_savanna"
        } else {
            underlying
        }
    }

    fn pick_shattered_coast_biome(&self, t: usize, h: usize, weirdness: Param) -> &'static str {
        let beach_or_middle = if weirdness.max >= 0.0 {
            self.pick_middle_biome(t, h, weirdness)
        } else {
            self.pick_beach_biome(t)
        };
        self.maybe_pick_windswept_savanna_biome(t, h, weirdness, beach_or_middle)
    }

    fn pick_beach_biome(&self, t: usize) -> &'static str {
        match t {
            0 => "snowy_beach",
            4 => "desert",
            _ => "beach",
        }
    }

    fn pick_badlands_biome(&self, h: usize, weirdness: Param) -> &'static str {
        if h < 2 {
            return if weirdness.max < 0.0 {
                "badlands"
            } else {
                "eroded_badlands"
            };
        }
        if h < 3 {
            "badlands"
        } else {
            "wooded_badlands"
        }
    }

    fn pick_plateau_biome(&self, t: usize, h: usize, weirdness: Param) -> &'static str {
        if weirdness.max >= 0.0 {
            if let Some(variant) = self.plateau_biomes_variant[t][h] {
                return variant;
            }
        }
        self.plateau_biomes[t][h]
    }

    fn pick_peak_biome(&self, t: usize, h: usize, weirdness: Param) -> &'static str {
        if t <= 2 {
            return if weirdness.max < 0.0 {
                "jagged_peaks"
            } else {
                "frozen_peaks"
            };
        }
        if t == 3 {
            "stony_peaks"
        } else {
            self.pick_badlands_biome(h, weirdness)
        }
    }

    fn pick_slope_biome(&self, t: usize, h: usize, weirdness: Param) -> &'static str {
        if t >= 3 {
            return self.pick_plateau_biome(t, h, weirdness);
        }
        if h <= 1 {
            "snowy_slopes"
        } else {
            "grove"
        }
    }

    fn pick_shattered_biome(&self, t: usize, h: usize, weirdness: Param) -> &'static str {
        self.shattered_biomes[t][h].unwrap_or_else(|| self.pick_middle_biome(t, h, weirdness))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn overworld() -> Vec<MultiNoiseBiomeEntry> {
        preset_entries(&ResourceLocation::minecraft("overworld")).unwrap()
    }

    #[test]
    fn unknown_preset_is_none() {
        assert!(preset_entries(&ResourceLocation::minecraft("the_end")).is_none());
        assert!(preset_entries(&ResourceLocation::new("modid", "overworld")).is_none());
    }

    #[test]
    fn overworld_covers_the_biomes_a_player_expects() {
        let ids: HashSet<String> = overworld()
            .iter()
            .map(|e| e.biome.path().to_string())
            .collect();
        // A spread across every branch of the builder: oceans, the middle table, a variant,
        // a plateau, a shattered entry, rivers, beaches, and the three underground biomes.
        for expected in [
            "ocean",
            "deep_frozen_ocean",
            "mushroom_fields",
            "plains",
            "desert",
            "jungle",
            "sunflower_plains",
            "badlands",
            "meadow",
            "cherry_grove",
            "windswept_hills",
            "river",
            "frozen_river",
            "beach",
            "snowy_beach",
            "swamp",
            "mangrove_swamp",
            "jagged_peaks",
            "stony_peaks",
            "grove",
            "snowy_slopes",
            "windswept_savanna",
            "dripstone_caves",
            "lush_caves",
            "deep_dark",
        ] {
            assert!(
                ids.contains(expected),
                "overworld preset is missing {expected}"
            );
        }
    }

    #[test]
    fn overworld_is_not_one_biome() {
        // The bug this fixes: an unresolved preset left every position on the fallback biome,
        // so a whole world generated as plains.
        let ids: HashSet<String> = overworld()
            .iter()
            .map(|e| e.biome.path().to_string())
            .collect();
        assert!(ids.len() > 40, "only {} distinct biomes", ids.len());
    }

    #[test]
    fn nether_has_its_five_biomes() {
        let ids: Vec<String> = preset_entries(&ResourceLocation::minecraft("nether"))
            .unwrap()
            .iter()
            .map(|e| e.biome.path().to_string())
            .collect();
        assert_eq!(
            ids,
            vec![
                "nether_wastes",
                "soul_sand_valley",
                "crimson_forest",
                "warped_forest",
                "basalt_deltas"
            ]
        );
    }

    #[test]
    fn every_surface_point_is_emitted_at_both_depths() {
        // `addSurfaceBiome` emits depth 0 and depth 1; underground and bottom biomes do not.
        let entries = overworld();
        let depth_of = |e: &MultiNoiseBiomeEntry| match e.parameters.depth {
            ClimateParam::Range([lo, hi]) => (lo, hi),
            ClimateParam::Single(v) => (v, v),
        };
        let surface_zero = entries.iter().filter(|e| depth_of(e) == (0.0, 0.0)).count();
        let surface_one = entries.iter().filter(|e| depth_of(e) == (1.0, 1.0)).count();
        assert_eq!(surface_zero, surface_one);
        assert!(surface_zero > 0);
    }
}
