//! `OxideGenerator`: the opaque handle behind `oxide_open`/`oxide_generate_chunk`. Owns
//! everything it needs (no borrow from the loaded `Datapack`), so it's safe to hand a raw
//! pointer to it across the FFI boundary — see `NoiseRouterEvaluator`'s doc comment in
//! `oxide-noise` for why that matters.

use std::ffi::CString;
use std::path::Path;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};

use oxide_biome::BiomeSearchTree;
use oxide_chunkgen::fill_chunk;
use oxide_core::{ChunkPos, ResourceLocation};
use oxide_datapack::{load_datapack, BiomeSource, NoiseGeneratorSettings};
use oxide_noise::NoiseRouterEvaluator;

pub struct OxideGenerator {
    settings: NoiseGeneratorSettings,
    default_block_name: CString,
    default_fluid_name: CString,
    router: NoiseRouterEvaluator,
    biome_tree: Option<BiomeSearchTree>,
}

impl OxideGenerator {
    pub fn open(datapack_path: &Path, dimension_id: &str, seed: i64) -> Result<Self> {
        let datapack = load_datapack(datapack_path)
            .with_context(|| format!("loading datapack at {}", datapack_path.display()))?;

        let dim_id = ResourceLocation::from_str(dimension_id)
            .map_err(|e| anyhow!("invalid dimension id {dimension_id:?}: {e}"))?;
        let dimension = datapack
            .dimensions
            .get(&dim_id)
            .ok_or_else(|| anyhow!("dimension {dim_id} not found in datapack"))?;
        let noise_settings_id = dimension.generator.settings.as_ref().ok_or_else(|| {
            anyhow!("dimension {dim_id}'s generator has no noise_settings (not minecraft:noise?)")
        })?;
        let settings = datapack
            .noise_settings
            .get(noise_settings_id)
            .ok_or_else(|| anyhow!("noise_settings {noise_settings_id} not found in datapack"))?
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

        let default_block_name = CString::new(settings.default_block.name.to_string())
            .map_err(|e| anyhow!("default_block name contains a NUL byte: {e}"))?;
        let default_fluid_name = CString::new(settings.default_fluid.name.to_string())
            .map_err(|e| anyhow!("default_fluid name contains a NUL byte: {e}"))?;

        Ok(Self {
            settings,
            default_block_name,
            default_fluid_name,
            router,
            biome_tree,
        })
    }

    pub fn min_y(&self) -> i32 {
        self.settings.noise.min_y
    }

    pub fn height(&self) -> i32 {
        self.settings.noise.height
    }

    pub fn sea_level(&self) -> i32 {
        self.settings.sea_level
    }

    pub fn default_block_name_ptr(&self) -> *const std::os::raw::c_char {
        self.default_block_name.as_ptr()
    }

    pub fn default_fluid_name_ptr(&self) -> *const std::os::raw::c_char {
        self.default_fluid_name.as_ptr()
    }

    pub fn buffer_len(&self) -> usize {
        256 * self.height() as usize
    }

    /// Fills `out` (already validated to be `buffer_len()` long by the caller) with the chunk
    /// at `(chunk_x, chunk_z)` — `oxide_generate_chunk`'s `0`/`1`/`2` encoding for air/default
    /// block/default fluid, one byte per block, `(y*16+z)*16+x` within the whole column
    /// (`y` relative to `min_y`, not restarted per 16-tall section). Biome ignored (see this
    /// crate's module doc).
    pub fn generate_chunk(&self, chunk_x: i32, chunk_z: i32, out: &mut [u8]) {
        let chunk = fill_chunk(
            ChunkPos::new(chunk_x, chunk_z),
            &self.settings,
            &self.router,
            self.biome_tree.as_ref(),
        );

        out.fill(0);
        let height = self.height();
        for section in &chunk.sections {
            let section_base_y = (section.y as i32) * 16 - self.min_y();
            for local_y in 0..16usize {
                let column_y = section_base_y + local_y as i32;
                if column_y < 0 || column_y >= height {
                    continue; // defensive: shouldn't happen for a well-formed settings.noise range
                }
                for local_z in 0..16usize {
                    for local_x in 0..16usize {
                        let src_index = (local_y * 16 + local_z) * 16 + local_x;
                        let block = section.block_states.get(src_index);
                        let value: u8 = if *block == self.settings.default_block {
                            1
                        } else if *block == self.settings.default_fluid {
                            2
                        } else {
                            continue; // air — buffer already zeroed
                        };
                        let dst = (column_y as usize * 16 + local_z) * 16 + local_x;
                        out[dst] = value;
                    }
                }
            }
        }
    }
}
