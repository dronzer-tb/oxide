//! Real generation for `oxide-pregen`: terrain, decoration, heightmaps, NBT, compression and
//! region writes, each stage timed on its own.
//!
//! Terrain is `oxide_chunkgen::generate_chunk` with each source chunk's biome carvers, and
//! decoration is `BiomeDecorator` over a `DecorationRegion` of the 3x3 undecorated chunks --
//! the same wiring as `oxide-ffi`'s `OxideGenerator` and `oxide-harness`'s oracle. A finished
//! chunk is its terrain plus the writes landing in it from the nine chunks around it, applied
//! in `oxide_chunkgen::decorate::decorated_chunk`'s fixed order (west to east, then north to
//! south), so this writes the same blocks the live module would hand the server.
//!
//! Stages run one after another over the whole area, each across all cores, so each stage's
//! wall time is its own cost. That holds the whole area in memory: about 0.4 MB per chunk.
//! Terrain is generated two chunks past the area (decoration reads that far), so a small
//! area pays a large edge: radius 16 generates 37x37 terrain chunks to finish 33x33.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::str::FromStr;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use rayon::prelude::*;

use oxide_anvil::compress::Compression;
use oxide_anvil::nbt::{self, ChunkNbtWriteOptions};
use oxide_anvil::region::{self, CompressedChunk};
use oxide_biome::BiomeSearchTree;
use oxide_chunkgen::decorate::heightmaps;
use oxide_chunkgen::{
    generate_chunk, BiomeDecorator, BiomeTemperatures, CarverSetup, DecorationRegion,
    Decorator,
};
use oxide_chunkgen::region::InternedWrites;
use oxide_core::{ChunkData, ChunkPos, ChunkStatus, ResourceLocation};
use oxide_datapack::{
    load_datapack, BiomeSource, BlockFacts, ConfiguredCarver, MultiNoiseSource,
    NoiseGeneratorSettings,
};
use oxide_noise::NoiseRouterEvaluator;

/// Everything generation needs, built once from the datapack and shared by every thread.
pub struct Generator {
    settings: NoiseGeneratorSettings,
    router: NoiseRouterEvaluator,
    biome_tree: Option<BiomeSearchTree>,
    biome_temperatures: BiomeTemperatures,
    biome_carvers: HashMap<ResourceLocation, Vec<ConfiguredCarver>>,
    seed: i64,
    decorator: BiomeDecorator,
    /// The pack's DataVersion, from its `version.json`.
    pub data_version: i32,
}

impl Generator {
    pub fn open(datapack_path: &Path, dimension_id: &str, seed: i64) -> Result<Self> {
        let datapack = load_datapack(datapack_path)
            .with_context(|| format!("loading datapack at {}", datapack_path.display()))?;
        let dim_id = ResourceLocation::from_str(dimension_id)
            .map_err(|e| anyhow!("invalid dimension id {dimension_id:?}: {e}"))?;
        let dimension = datapack
            .dimensions
            .get(&dim_id)
            .ok_or_else(|| anyhow!("dimension {dim_id} not found in datapack"))?;
        let settings_id = dimension
            .generator
            .settings
            .as_ref()
            .ok_or_else(|| anyhow!("{dim_id} has no noise settings"))?;
        let settings = datapack
            .noise_settings
            .get(settings_id)
            .ok_or_else(|| anyhow!("noise_settings {settings_id} not found"))?
            .clone();
        let router = NoiseRouterEvaluator::new(
            seed,
            &settings,
            &datapack.density_functions,
            &datapack.noise_params,
        );
        let biome_tree = match dimension.generator.biome_source.as_ref() {
            Some(BiomeSource::MultiNoise(source)) => BiomeSearchTree::from_source(source),
            _ => None,
        };
        let biome_carvers = datapack
            .biomes
            .iter()
            .map(|(id, biome)| {
                let carvers = biome
                    .carvers
                    .iter()
                    .filter_map(|c| datapack.configured_carvers.get(c).cloned())
                    .collect();
                (id.clone(), carvers)
            })
            .collect();
        let biome_temperatures: BiomeTemperatures = datapack
            .biomes
            .iter()
            .map(|(id, biome)| (id.clone(), biome.temperature))
            .collect();
        let entries = match dimension.generator.biome_source.as_ref() {
            Some(BiomeSource::MultiNoise(MultiNoiseSource::Explicit { biomes })) => biomes.clone(),
            Some(BiomeSource::MultiNoise(MultiNoiseSource::Preset { preset })) => {
                oxide_datapack::preset_entries(preset).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let mut possible_biomes: Vec<ResourceLocation> = Vec::new();
        for entry in entries {
            if !possible_biomes.contains(&entry.biome) {
                possible_biomes.push(entry.biome);
            }
        }
        let decorator = BiomeDecorator::new(&datapack, &possible_biomes, seed, settings.sea_level)
            .map_err(|e| anyhow!("BiomeDecorator::new: {e:#}"))?;
        Ok(Self {
            settings,
            router,
            biome_tree,
            biome_temperatures,
            biome_carvers,
            seed,
            decorator,
            data_version: datapack.data_version.data_version,
        })
    }

    pub fn facts(&self) -> &BlockFacts {
        self.decorator.facts()
    }

    /// Undecorated terrain: noise, ore veins, surface, carvers.
    pub fn terrain(&self, pos: ChunkPos) -> ChunkData {
        let carvers_at = |source: ChunkPos| -> Vec<ConfiguredCarver> {
            let Some(tree) = self.biome_tree.as_ref() else {
                return Vec::new();
            };
            let sample = oxide_biome::ClimateSample::sample(
                &self.router,
                source.min_block_x(),
                0,
                source.min_block_z(),
            );
            match tree.nearest(sample) {
                Some(biome) => self.biome_carvers.get(biome).cloned().unwrap_or_default(),
                None => Vec::new(),
            }
        };
        let setup = CarverSetup {
            seed: self.seed,
            carvers_at: &carvers_at,
        };
        generate_chunk(
            pos,
            &self.settings,
            &self.router,
            self.biome_tree.as_ref(),
            &self.biome_temperatures,
            Some(&setup),
        )
    }

    /// The block writes (interned: no state is cloned per write) chunk `refs[4]`'s features make, over the nine chunks around it.
    pub fn decoration_writes(&self, refs: [&ChunkData; 9]) -> InternedWrites {
        let mut region = DecorationRegion::new(refs[4].pos, refs, self.decorator.facts());
        self.decorator.decorate(&mut region);
        region.into_interned()
    }
}

/// A square of chunks: every position with `|x - cx| <= radius` and `|z - cz| <= radius`,
/// z-major.
fn square(center: ChunkPos, radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|dz| {
            (-radius..=radius).map(move |dx| ChunkPos::new(center.x + dx, center.z + dz))
        })
        .collect()
}

/// Applies `writes` (all from one source chunk) that land inside `chunk`.
///
/// Each interned state's palette index is worked out once per section, the first time a write
/// of it lands there, exactly as before: the first palette entry equal to it, or a new entry
/// (cloned and hashed only then, which is when `PalettedContainer::set` would insert it, so
/// palette order is unchanged). A palette only grows, so the index found stays the first equal
/// entry for every later write of that state to that section.
fn apply_writes(chunk: &mut ChunkData, writes: &InternedWrites) {
    const UNKNOWN: u32 = u32::MAX;
    let (min_y, height) = (chunk.min_y, chunk.height);
    let states = writes.states.len();
    let mut known = vec![UNKNOWN; chunk.sections.len() * states];
    for &(at, id) in &writes.writes {
        if at.chunk_pos() != chunk.pos || at.y < min_y || at.y >= min_y + height {
            continue;
        }
        let y = (at.y - min_y) as usize;
        let slot = ((y & 15) * 16 + (at.z & 15) as usize) * 16 + (at.x & 15) as usize;
        let section = &mut chunk.sections[y >> 4].block_states;
        let cached = &mut known[(y >> 4) * states + id as usize];
        if *cached == UNKNOWN {
            let state = &writes.states[id as usize];
            *cached = match section.palette().iter().position(|p| p == state) {
                Some(i) => i as u32,
                None => section.index_of_or_insert(state.clone()),
            };
        }
        section.set_index(slot, *cached);
    }
}

/// Wall time per stage, in seconds.
#[derive(Debug, Default, Clone, Copy)]
pub struct StageTimes {
    pub generate: f64,
    pub decorate: f64,
    pub serialise: f64,
    pub compress: f64,
    pub write: f64,
}

impl StageTimes {
    pub fn total(&self) -> f64 {
        self.generate + self.decorate + self.serialise + self.compress + self.write
    }
}

pub struct Outcome {
    pub chunks: usize,
    pub regions: usize,
    pub bytes_nbt: u64,
    pub bytes_written: u64,
    pub times: StageTimes,
    /// blake3 over every chunk's uncompressed NBT, in position order: changes if and only if
    /// the chunks written change.
    pub digest: String,
}

pub struct Options<'a> {
    pub region_dir: &'a Path,
    pub center: ChunkPos,
    pub radius: i32,
    pub compression: Compression,
    pub sync: bool,
    /// `false` skips the region files (and their time) entirely, for timing the other stages.
    pub write: bool,
}

/// Generates, decorates and writes every chunk within `opts.radius` of `opts.center`.
pub fn run(generator: &Generator, opts: &Options<'_>) -> Result<Outcome> {
    let mut times = StageTimes::default();
    // A finished chunk takes writes from its nine neighbours, and a neighbour's writes read
    // its own nine: terrain reaches two chunks past the area, writes one.
    let terrain_radius = opts.radius + 2;
    let writes_radius = opts.radius + 1;
    let index = |pos: ChunkPos, radius: i32| -> Option<usize> {
        let width = 2 * radius + 1;
        let x = pos.x - opts.center.x + radius;
        let z = pos.z - opts.center.z + radius;
        (x >= 0 && z >= 0 && x < width && z < width).then(|| (z * width + x) as usize)
    };

    // 1. Terrain.
    let started = Instant::now();
    let mut terrain: Vec<ChunkData> = square(opts.center, terrain_radius)
        .par_iter()
        .map(|&pos| generator.terrain(pos))
        .collect();
    times.generate = started.elapsed().as_secs_f64();
    let at = |pos: ChunkPos| -> &ChunkData {
        &terrain[index(pos, terrain_radius).expect("inside the terrain square")]
    };

    // 2. Decoration: each chunk's writes, then each finished chunk from its nine neighbours',
    // in `oxide_chunkgen::decorate::decorated_chunk`'s order.
    let started = Instant::now();
    let (min_y, height) = (terrain[0].min_y, terrain[0].height);
    let writes: Vec<InternedWrites> = square(opts.center, writes_radius)
        .par_iter()
        .map(|&c| {
            let refs: [&ChunkData; 9] = std::array::from_fn(|i| {
                at(ChunkPos::new(
                    c.x + (i % 3) as i32 - 1,
                    c.z + (i / 3) as i32 - 1,
                ))
            });
            generator.decoration_writes(refs)
        })
        .collect();
    let inner = square(opts.center, opts.radius);
    let facts = generator.facts();
    // The writes are made, so terrain is only read as a base now, and each inner terrain chunk
    // is the base of exactly one finished chunk: move it rather than clone it.
    let bases: Vec<ChunkData> = inner
        .iter()
        .map(|&pos| {
            let i = index(pos, terrain_radius).expect("inside the terrain square");
            std::mem::replace(&mut terrain[i], ChunkData::new(pos, min_y, height))
        })
        .collect();
    let finished: Vec<ChunkData> = bases
        .into_par_iter()
        .map(|mut chunk| {
            let pos = chunk.pos;
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let source = ChunkPos::new(pos.x + dx, pos.z + dz);
                    let i = index(source, writes_radius).expect("inside the writes square");
                    apply_writes(&mut chunk, &writes[i]);
                }
            }
            chunk.heightmaps = heightmaps(&chunk, facts);
            chunk.status = ChunkStatus::Features;
            chunk
        })
        .collect();
    drop(terrain);
    times.decorate = started.elapsed().as_secs_f64();

    // 3. NBT.
    let write_opts = ChunkNbtWriteOptions {
        data_version: generator.data_version,
        last_update: 0,
        inhabited_time: 0,
    };
    let started = Instant::now();
    let nbt_bytes: Vec<Vec<u8>> = finished
        .par_iter()
        .map(|chunk| nbt::serialize_chunk(chunk, &write_opts))
        .collect::<Result<_, _>>()?;
    times.serialise = started.elapsed().as_secs_f64();
    drop(finished);

    let mut digest = blake3::Hasher::new();
    for bytes in &nbt_bytes {
        digest.update(&(bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    let bytes_nbt = nbt_bytes.iter().map(|b| b.len() as u64).sum();

    // 4. Compression.
    let started = Instant::now();
    let compressed: Vec<CompressedChunk> = inner
        .par_iter()
        .zip(nbt_bytes.par_iter())
        .map(|(&pos, bytes)| -> Result<CompressedChunk> {
            let mut payload = Vec::new();
            opts.compression.compress_into(bytes, &mut payload)?;
            Ok(CompressedChunk {
                pos,
                scheme: opts.compression.scheme(),
                payload,
            })
        })
        .collect::<Result<_>>()?;
    times.compress = started.elapsed().as_secs_f64();
    drop(nbt_bytes);

    // 5. One write per region file.
    let mut by_region: BTreeMap<(i32, i32), Vec<CompressedChunk>> = BTreeMap::new();
    for chunk in compressed {
        by_region
            .entry(region::region_coords_for(&chunk.pos))
            .or_default()
            .push(chunk);
    }
    let regions = by_region.len();
    let mut bytes_written = 0;
    if opts.write {
        std::fs::create_dir_all(opts.region_dir)
            .with_context(|| format!("creating {}", opts.region_dir.display()))?;
        let started = Instant::now();
        bytes_written = by_region
            .par_iter()
            .map(|(&(rx, rz), chunks)| -> Result<u64> {
                let path = opts.region_dir.join(format!("r.{rx}.{rz}.mca"));
                region::write_region_compressed(&path, (rx, rz), chunks, opts.sync)
                    .with_context(|| format!("writing {}", path.display()))
            })
            .sum::<Result<u64>>()?;
        times.write = started.elapsed().as_secs_f64();
    }

    Ok(Outcome {
        chunks: inner.len(),
        regions,
        bytes_nbt,
        bytes_written,
        times,
        digest: digest.finalize().to_hex()[..16].to_string(),
    })
}
