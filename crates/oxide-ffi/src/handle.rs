//! `OxideGenerator`: the opaque handle behind `oxide_open`/`oxide_generate_chunk`. Owns
//! everything it needs (no borrow from the loaded `Datapack`), so it's safe to hand a raw
//! pointer to it across the FFI boundary — see `NoiseRouterEvaluator`'s doc comment in
//! `oxide-noise` for why that matters.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::Path;
use std::str::FromStr;
use std::sync::RwLock;

use anyhow::{anyhow, Context, Result};

use oxide_biome::BiomeSearchTree;
use oxide_chunkgen::{generate_chunk, BiomeTemperatures};
use oxide_core::{ChunkPos, ResourceLocation};
use oxide_datapack::{load_datapack, BiomeSource, NoiseGeneratorSettings};
use oxide_noise::NoiseRouterEvaluator;

pub struct OxideGenerator {
    settings: NoiseGeneratorSettings,
    default_block_name: CString,
    default_fluid_name: CString,
    router: NoiseRouterEvaluator,
    biome_tree: Option<BiomeSearchTree>,
    biome_temperatures: BiomeTemperatures,
    /// Interned block-state strings, indexed by the u16 values `generate_chunk` writes. Grows
    /// as generation meets new states and never shrinks, so an index handed to the caller
    /// stays valid for the handle's whole life -- that is what lets the Java side resolve each
    /// index to a BlockData exactly once and cache it.
    block_palette: RwLock<Palette>,
    biome_palette: RwLock<Palette>,
}

/// Append-only string interner behind the u16 indices crossing the FFI boundary.
#[derive(Default)]
pub struct Palette {
    names: Vec<CString>,
    lookup: HashMap<String, u16>,
}

impl Palette {
    /// Interns `name`, returning its stable index. `u16::MAX` entries is far beyond any real
    /// datapack's block-state count; the cap is a guard, not an expected limit.
    fn intern(&mut self, name: String) -> Option<u16> {
        if let Some(index) = self.lookup.get(&name) {
            return Some(*index);
        }
        if self.names.len() >= u16::MAX as usize {
            return None;
        }
        let c = CString::new(name.clone()).ok()?;
        let index = self.names.len() as u16;
        self.names.push(c);
        self.lookup.insert(name, index);
        Some(index)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Pointer to the interned string at `index`. Stable for the handle's lifetime: entries
    /// are never removed or rewritten, and a `CString`'s buffer does not move when the backing
    /// `Vec` reallocates.
    pub fn name_ptr(&self, index: usize) -> *const std::os::raw::c_char {
        self.names
            .get(index)
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null())
    }
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

        // Surface rules read each biome's base temperature; the rest of the biome definitions
        // are not needed after this point, which is why only this map is kept.
        let biome_temperatures: BiomeTemperatures = datapack
            .biomes
            .iter()
            .map(|(id, biome)| (id.clone(), biome.temperature))
            .collect();

        // Pre-intern every biome the pack defines. A Bukkit BiomeProvider must declare the
        // biomes it can return *before* any chunk exists, and this is a superset of what the
        // biome tree can actually emit -- which is what that declaration wants.
        let mut biome_palette = Palette::default();
        for (id, _) in datapack.biomes.iter() {
            biome_palette.intern(id.to_string());
        }

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
            biome_temperatures,
            block_palette: RwLock::new(Palette::default()),
            biome_palette: RwLock::new(biome_palette),
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

    /// Number of `u16` entries `generate_chunk` writes into `out_blocks`.
    pub fn block_buffer_len(&self) -> usize {
        256 * self.height() as usize
    }

    /// Number of `u16` entries `generate_chunk` writes into `out_biomes`: one per 4x4x4 quart,
    /// 64 per section.
    pub fn biome_buffer_len(&self) -> usize {
        64 * (self.height() as usize / 16)
    }

    pub fn block_palette(&self) -> &RwLock<Palette> {
        &self.block_palette
    }

    pub fn biome_palette(&self) -> &RwLock<Palette> {
        &self.biome_palette
    }

    /// Generates the chunk at `(chunk_x, chunk_z)` -- noise fill plus surface rules -- and
    /// writes it as palette indices.
    ///
    /// `out_blocks` is one `u16` per block, `(y*16+z)*16+x` within the whole column (`y`
    /// relative to `min_y`, not restarted per section). `out_biomes` is one `u16` per biome
    /// quart, `section_index*64 + (qy*4+qz)*4+qx`. Both index this handle's palettes, which
    /// the caller reads back with the palette accessors; indices are stable for the handle's
    /// life, so a caller resolves each one once.
    ///
    /// Concurrent calls on one handle stay safe: the generator itself is read-only, and the
    /// palettes are behind an `RwLock` taken only to intern a state not seen before.
    pub fn generate_chunk(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        out_blocks: &mut [u16],
        out_biomes: &mut [u16],
    ) -> Result<()> {
        let chunk = generate_chunk(
            ChunkPos::new(chunk_x, chunk_z),
            &self.settings,
            &self.router,
            self.biome_tree.as_ref(),
            &self.biome_temperatures,
        );

        out_blocks.fill(0);
        out_biomes.fill(0);
        let height = self.height();

        for (section_index, section) in chunk.sections.iter().enumerate() {
            let section_base_y = (section.y as i32) * 16 - self.min_y();

            for local_y in 0..16usize {
                let column_y = section_base_y + local_y as i32;
                if column_y < 0 || column_y >= height {
                    continue; // defensive: a malformed settings.noise range
                }
                for local_z in 0..16usize {
                    for local_x in 0..16usize {
                        let src = (local_y * 16 + local_z) * 16 + local_x;
                        let block = section.block_states.get(src);
                        let index = self.intern(&self.block_palette, block.to_string())?;
                        let dst = (column_y as usize * 16 + local_z) * 16 + local_x;
                        out_blocks[dst] = index;
                    }
                }
            }

            let biome_base = section_index * 64;
            if biome_base + 64 > out_biomes.len() {
                continue;
            }
            for quart in 0..64usize {
                let biome = section.biomes.get(quart);
                out_biomes[biome_base + quart] =
                    self.intern(&self.biome_palette, biome.to_string())?;
            }
        }
        Ok(())
    }

    /// Biome at one block position, without generating a chunk -- climate sample plus a search
    /// of the biome tree. This is what backs a Bukkit `BiomeProvider`, which is queried per
    /// position and outside chunk generation entirely.
    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> Result<u16> {
        let name = match self.biome_tree.as_ref() {
            Some(tree) => {
                let sample = oxide_biome::ClimateSample::sample(&self.router, x, y, z);
                match tree.nearest(sample) {
                    Some(biome) => biome.to_string(),
                    None => "minecraft:plains".to_string(),
                }
            }
            // Unresolvable Preset biome source -- same fallback fill_chunk uses.
            None => "minecraft:plains".to_string(),
        };
        self.intern(&self.biome_palette, name)
    }

    fn intern(&self, palette: &RwLock<Palette>, name: String) -> Result<u16> {
        // Read first: after the first few chunks every state is already interned, so the
        // common path never takes the write lock.
        if let Some(index) = palette
            .read()
            .map_err(|_| anyhow!("palette lock poisoned"))?
            .lookup
            .get(&name)
        {
            return Ok(*index);
        }
        palette
            .write()
            .map_err(|_| anyhow!("palette lock poisoned"))?
            .intern(name.clone())
            .ok_or_else(|| anyhow!("block/biome palette is full, interning {name} failed"))
    }
}
