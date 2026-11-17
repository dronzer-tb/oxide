//! `OxideGenerator`: the opaque handle behind `oxide_open`/`oxide_generate_chunk`. Owns
//! everything it needs (no borrow from the loaded `Datapack`), so it's safe to hand a raw
//! pointer to it across the FFI boundary — see `NoiseRouterEvaluator`'s doc comment in
//! `oxide-noise` for why that matters.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::RwLock;

use anyhow::{anyhow, Context, Result};

use oxide_biome::BiomeSearchTree;
use oxide_chunkgen::{
    decorated_chunk_with_extras, generate_chunk, BiomeDecorator, BiomeTemperatures, CarverSetup,
    ChunkExtras, DecorationCache,
};
use oxide_core::HeightmapType;
use oxide_core::{ChunkData, ChunkPos, ResourceLocation};
use oxide_datapack::{load_datapack, BiomeSource, MultiNoiseSource, NoiseGeneratorSettings};
use oxide_noise::NoiseRouterEvaluator;

/// Distinguishes generators so a cached `ChunkCaches` cannot be reused across worlds.
static NEXT_GENERATOR_ID: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The last chunk this thread asked `base_height` about, and its caches.
    static BASE_HEIGHT_CACHES: RefCell<Option<(u64, ChunkPos, oxide_noise::ChunkCaches)>> =
        const { RefCell::new(None) };
    /// Separate slot for `biome_at`. It shares `base_height`'s shape but not its access
    /// pattern: Bukkit interleaves the two (structure placement asks for heights while the
    /// biome pass walks quarts), and a single slot made each call evict the other's chunk, so
    /// every query rebuilt a full corner grid. Two slots make both hit.
    static BIOME_CACHES: RefCell<Option<(u64, ChunkPos, oxide_noise::ChunkCaches)>> =
        const { RefCell::new(None) };
    /// The extras of the last chunk this thread generated, as `chunk_extras` returns them.
    static LAST_EXTRAS: RefCell<Option<(u64, ChunkPos, String)>> = const { RefCell::new(None) };
}

pub struct OxideGenerator {
    id: u64,
    settings: NoiseGeneratorSettings,
    default_block_name: CString,
    default_fluid_name: CString,
    router: NoiseRouterEvaluator,
    biome_tree: Option<BiomeSearchTree>,
    biome_temperatures: BiomeTemperatures,
    /// Carvers per biome id, as the biome's `carvers` list names them. Vanilla picks the
    /// carver set from the biome at each source chunk, so this is kept per biome rather than
    /// flattened into one list.
    biome_carvers: HashMap<ResourceLocation, Vec<oxide_datapack::ConfiguredCarver>>,
    seed: i64,
    /// Interned block-state strings, indexed by the u16 values `generate_chunk` writes. Grows
    /// as generation meets new states and never shrinks, so an index handed to the caller
    /// stays valid for the handle's whole life -- that is what lets the Java side resolve each
    /// index to a BlockData exactly once and cache it.
    block_palette: RwLock<Palette>,
    biome_palette: RwLock<Palette>,
    biome_indices: HashMap<ResourceLocation, u16>,
    default_biome_index: u16,
    /// Native decoration, or why this datapack cannot support it. Built at open because the
    /// datapack is not kept; whether it runs is `decorate`.
    decoration: std::result::Result<Decoration, String>,
    /// Off until `set_decoration` turns it on: with it off the handle generates exactly the
    /// terrain it always did, for a server that still decorates on top itself.
    decorate: AtomicBool,
}

/// What decorating needs beyond terrain: the feature driver and its memoised intermediates.
struct Decoration {
    decorator: BiomeDecorator,
    cache: DecorationCache,
}

/// Undecorated chunks kept for decoration, ~1.3 MB each unpacked: finishing a chunk reads the
/// 5x5 around it, and neighbouring requests share most of that. 2048 was tried for a Chunky
/// spiral and bought no speed while pushing the server process to 19 GB resident, so it stays
/// small until terrain is cached packed.
const TERRAIN_CACHE_CHUNKS: usize = 256;
/// Per-chunk decoration write lists kept. Small next to terrain.
const WRITE_CACHE_CHUNKS: usize = 1024;

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

        // Carvers each biome configures, resolved once. A biome naming a carver the pack does
        // not define is skipped rather than failing the open -- the datapack's own resolution
        // report already flags dangling ids.
        let biome_carvers: HashMap<ResourceLocation, Vec<oxide_datapack::ConfiguredCarver>> =
            datapack
                .biomes
                .iter()
                .map(|(id, biome)| {
                    let carvers = biome
                        .carvers
                        .iter()
                        .filter_map(|carver_id| datapack.configured_carvers.get(carver_id).cloned())
                        .collect();
                    (id.clone(), carvers)
                })
                .collect();

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
        let mut biome_indices = HashMap::new();
        for (id, _) in datapack.biomes.iter() {
            if let Some(idx) = biome_palette.intern(id.to_string()) {
                biome_indices.insert(id.clone(), idx);
            }
        }
        let plains_loc = ResourceLocation::minecraft("plains");
        let default_biome_index = biome_indices.get(&plains_loc).copied().unwrap_or(0);

        let default_block_name = CString::new(settings.default_block.name.to_string())
            .map_err(|e| anyhow!("default_block name contains a NUL byte: {e}"))?;
        let default_fluid_name = CString::new(settings.default_fluid.name.to_string())
            .map_err(|e| anyhow!("default_fluid name contains a NUL byte: {e}"))?;

        // The biome source's possible biomes, in the order its parameter list first names them:
        // that order is what vanilla's FeatureSorter walks, so it decides every feature's seed.
        // PARITY-CHECK: confirm against 26.2 that `possibleBiomes()` iterates in this order.
        let mut possible_biomes: Vec<ResourceLocation> = Vec::new();
        let entries = match dimension.generator.biome_source.as_ref() {
            Some(BiomeSource::MultiNoise(MultiNoiseSource::Explicit { biomes })) => biomes.clone(),
            Some(BiomeSource::MultiNoise(MultiNoiseSource::Preset { preset })) => {
                oxide_datapack::preset_entries(preset).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        for entry in entries {
            if !possible_biomes.contains(&entry.biome) {
                possible_biomes.push(entry.biome);
            }
        }
        let decoration = if possible_biomes.is_empty() {
            Err(format!(
                "dimension {dim_id} has no multi-noise biome source to decorate from"
            ))
        } else {
            BiomeDecorator::new(&datapack, &possible_biomes, seed, settings.sea_level)
                .map(|decorator| Decoration {
                    decorator,
                    cache: DecorationCache::new(TERRAIN_CACHE_CHUNKS, WRITE_CACHE_CHUNKS),
                })
                .map_err(|e| format!("{e:#}"))
        };

        Ok(Self {
            id: NEXT_GENERATOR_ID.fetch_add(1, Ordering::Relaxed),
            settings,
            default_block_name,
            default_fluid_name,
            router,
            biome_tree,
            biome_temperatures,
            biome_carvers,
            seed,
            block_palette: RwLock::new(Palette::default()),
            biome_palette: RwLock::new(biome_palette),
            biome_indices,
            default_biome_index,
            decoration,
            decorate: AtomicBool::new(false),
        })
    }

    /// Instruction count per router program -- diagnostics for the register budget.
    pub fn program_sizes(&self) -> Vec<(&'static str, usize)> {
        self.router.program_sizes()
    }

    /// Instruction listing for the named router slot.
    pub fn dump_program(&self, name: &str) -> Vec<String> {
        use oxide_noise::RouterSlot;
        let slot = match name {
            "final_density" => RouterSlot::FinalDensity,
            "initial_density_without_jaggedness" => RouterSlot::InitialDensityWithoutJaggedness,
            _ => return vec![format!("unknown slot {name}")],
        };
        self.router.dump_program(slot)
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
    /// Turns native decoration on or off, returning whether it is now on. Turning it on fails
    /// when the datapack could not support it (no block facts, no tags, an unknown biome): a
    /// caller that has switched the server's own decoration off must not get bare terrain.
    pub fn set_decoration(&self, enabled: bool) -> Result<bool> {
        if enabled {
            if let Err(reason) = &self.decoration {
                return Err(anyhow!("native decoration is unavailable: {reason}"));
            }
        }
        self.decorate.store(enabled, Ordering::Release);
        Ok(enabled)
    }

    /// How many times each unported feature type or block rule was skipped so far, as
    /// `category:detail` -> count. Empty when decoration is unavailable.
    pub fn decoration_gaps(&self) -> Vec<(String, u64)> {
        match &self.decoration {
            Ok(decoration) => decoration
                .decorator
                .unsupported()
                .entries()
                .map(|(key, count)| (key.to_string(), count))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// One chunk as `ChunkData` plus what decoration left besides blocks, for callers in Rust
    /// that write chunks themselves (the offline pregen) and have no use for the flat arrays.
    /// Terrain only, status `carvers`, unless decoration is on.
    pub fn chunk_data(&self, chunk_x: i32, chunk_z: i32) -> (ChunkData, ChunkExtras) {
        let pos = ChunkPos::new(chunk_x, chunk_z);
        match &self.decoration {
            Ok(decoration) if self.decorate.load(Ordering::Acquire) => decorated_chunk_with_extras(
                pos,
                &|at| self.terrain(at),
                &decoration.decorator,
                decoration.decorator.facts(),
                &decoration.cache,
            ),
            _ => (self.terrain(pos), ChunkExtras::default()),
        }
    }

    pub fn generate_chunk(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        out_blocks: &mut [u16],
        out_biomes: &mut [u16],
    ) -> Result<()> {
        let pos = ChunkPos::new(chunk_x, chunk_z);
        let (chunk, extras) = self.chunk_data(chunk_x, chunk_z);
        LAST_EXTRAS.with(|slot| *slot.borrow_mut() = Some((self.id, pos, extras_text(&extras))));
        self.write_out(&chunk, out_blocks, out_biomes)
    }

    /// The extras of the last `generate_chunk` on this thread, when it was this handle's chunk
    /// at `(chunk_x, chunk_z)`: one item per line, absolute coordinates,
    ///
    /// ```text
    /// be <x> <y> <z> <snbt>      block entity
    /// ft <x> <y> <z> <fluid_id>  scheduled fluid tick
    /// bt <x> <y> <z> <block_id>  scheduled block tick
    /// pp <x> <y> <z>             post-processing mark
    /// ```
    ///
    /// Empty when decoration is off. Fails when this thread's last chunk was another.
    pub fn chunk_extras(&self, chunk_x: i32, chunk_z: i32) -> Result<String> {
        let pos = ChunkPos::new(chunk_x, chunk_z);
        LAST_EXTRAS.with(|slot| match &*slot.borrow() {
            Some((id, at, text)) if *id == self.id && *at == pos => Ok(text.clone()),
            _ => Err(anyhow!(
                "no extras for chunk ({chunk_x}, {chunk_z}): it is not the last chunk this \
                 thread generated with this handle"
            )),
        })
    }

    /// The undecorated chunk at `pos`: noise fill, surface rules and carvers. A pure function
    /// of position, which decoration relies on.
    fn terrain(&self, pos: ChunkPos) -> ChunkData {
        // Which carvers reach into this chunk depends on the biome at each source chunk, the
        // same way vanilla's applyCarvers picks them.
        let carvers_at = |source: ChunkPos| -> Vec<oxide_datapack::ConfiguredCarver> {
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
        let carver_setup = CarverSetup {
            seed: self.seed,
            carvers_at: &carvers_at,
        };

        generate_chunk(
            pos,
            &self.settings,
            &self.router,
            self.biome_tree.as_ref(),
            &self.biome_temperatures,
            Some(&carver_setup),
        )
    }

    /// Writes `chunk` as palette indices in the layout `generate_chunk` documents.
    fn write_out(
        &self,
        chunk: &ChunkData,
        out_blocks: &mut [u16],
        out_biomes: &mut [u16],
    ) -> Result<()> {
        out_blocks.fill(0);
        out_biomes.fill(0);
        let height = self.height();

        for (section_index, section) in chunk.sections.iter().enumerate() {
            let section_base_y = (section.y as i32) * 16 - self.min_y();

            // Intern once per distinct state in the section, not once per block. A section
            // holds 4096 positions and a handful of distinct states, and interning means
            // formatting the state to a `String` and hashing it under a lock -- which, done per
            // block, was 98304 allocations and lookups for every chunk the server asked for,
            // dwarfing the generation the numbers were being measured for.
            let block_indices: Vec<u16> = section
                .block_states
                .palette()
                .iter()
                .map(|state| self.intern(&self.block_palette, state.to_string()))
                .collect::<Result<_>>()?;
            let blocks = section.block_states.indices();

            for local_y in 0..16usize {
                let column_y = section_base_y + local_y as i32;
                if column_y < 0 || column_y >= height {
                    continue; // defensive: a malformed settings.noise range
                }
                for local_z in 0..16usize {
                    for local_x in 0..16usize {
                        let src = (local_y * 16 + local_z) * 16 + local_x;
                        let dst = (column_y as usize * 16 + local_z) * 16 + local_x;
                        out_blocks[dst] = block_indices[blocks[src] as usize];
                    }
                }
            }

            let biome_base = section_index * 64;
            if biome_base + 64 > out_biomes.len() {
                continue;
            }
            let biome_indices: Vec<u16> = section
                .biomes
                .palette()
                .iter()
                .map(|biome| self.intern(&self.biome_palette, biome.to_string()))
                .collect::<Result<_>>()?;
            let biomes = section.biomes.indices();
            for quart in 0..64usize {
                out_biomes[biome_base + quart] = biome_indices[biomes[quart] as usize];
            }
        }
        Ok(())
    }

    /// Surface height at `(x, z)` for one heightmap type -- what a Bukkit
    /// `ChunkGenerator.getBaseHeight` override answers with. Without it CraftBukkit falls back
    /// to the *vanilla* noise generator, so structures get placed at vanilla's heights on top
    /// of this generator's terrain.
    /// Answers one column, reusing the chunk's density caches across the whole chunk.
    ///
    /// Bukkit asks this per column during structure placement -- 256 calls for one chunk -- and
    /// each `ChunkCaches` costs a full corner-grid evaluation, so building one per call made a
    /// chunk's worth of queries cost ~26x generating the chunk outright. The memo holds the last
    /// chunk asked about, per thread: Folia's region threads each walk their own region, so one
    /// slot per thread turns 256 builds into 1 without any sharing between threads. `ChunkCaches`
    /// is `!Sync` (its grids are `RefCell`s), which is also why this cannot be a field.
    pub fn base_height(&self, x: i32, z: i32, ty: HeightmapType) -> i32 {
        let chunk = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        BASE_HEIGHT_CACHES.with(|slot| {
            let mut slot = slot.borrow_mut();
            // Keyed by generator too: two worlds open at once have different routers, and a
            // cache built from one would silently answer with the other's terrain.
            let reusable = slot
                .as_ref()
                .is_some_and(|(id, pos, _)| *id == self.id && *pos == chunk);
            if !reusable {
                *slot = Some((self.id, chunk, self.router.chunk_caches(chunk.x, chunk.z)));
            }
            let (_, _, caches) = slot.as_ref().expect("just populated");
            oxide_chunkgen::base_height_in(x, z, &self.settings, &self.router, caches, ty)
        })
    }

    /// Biome at one block position, without generating a chunk -- climate sample plus a search
    /// of the biome tree. This is what backs a Bukkit `BiomeProvider`, which is queried per
    /// position and outside chunk generation entirely. Reuses thread-local chunk caches.
    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> Result<u16> {
        let idx = match self.biome_tree.as_ref() {
            Some(tree) => {
                let chunk = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
                let sample = BIOME_CACHES.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let reusable = slot
                        .as_ref()
                        .is_some_and(|(id, pos, _)| *id == self.id && *pos == chunk);
                    if !reusable {
                        *slot = Some((self.id, chunk, self.router.chunk_caches(chunk.x, chunk.z)));
                    }
                    let (_, _, caches) = slot.as_ref().expect("just populated");
                    oxide_biome::ClimateSample::sample_in_chunk(&self.router, caches, x, y, z)
                });
                match tree.nearest(sample) {
                    Some(biome) => self
                        .biome_indices
                        .get(biome)
                        .copied()
                        .unwrap_or(self.default_biome_index),
                    None => self.default_biome_index,
                }
            }
            // Unresolvable Preset biome source -- same fallback fill_chunk uses.
            None => self.default_biome_index,
        };
        Ok(idx)
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

/// `extras` in the line format [`OxideGenerator::chunk_extras`] documents: block entities, then
/// fluid ticks, block ticks and post-processing marks, each in the order decoration left them.
fn extras_text(extras: &ChunkExtras) -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for entity in &extras.block_entities {
        let p = entity.pos;
        let _ = writeln!(text, "be {} {} {} {}", p.x, p.y, p.z, entity.snbt);
    }
    for (p, fluid) in &extras.fluid_ticks {
        let _ = writeln!(text, "ft {} {} {} {fluid}", p.x, p.y, p.z);
    }
    for (p, block) in &extras.block_ticks {
        let _ = writeln!(text, "bt {} {} {} {block}", p.x, p.y, p.z);
    }
    for p in &extras.post_process {
        let _ = writeln!(text, "pp {} {} {}", p.x, p.y, p.z);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn reference() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference"))
    }

    /// Decoration is opt-in, changes what a chunk holds once on, and stays a pure function of
    /// position: a second handle asked for the same chunk first, with nothing cached, agrees.
    #[test]
    fn decoration_adds_ores_and_does_not_depend_on_request_order() {
        if !reference().join("block_facts.tsv").is_file() {
            eprintln!("skipping: no block facts in {}", reference().display());
            return;
        }
        let generate = |handle: &OxideGenerator, x: i32, z: i32| {
            let mut blocks = vec![0u16; handle.block_buffer_len()];
            let mut biomes = vec![0u16; handle.biome_buffer_len()];
            handle
                .generate_chunk(x, z, &mut blocks, &mut biomes)
                .unwrap();
            let palette = handle.block_palette().read().unwrap();
            blocks
                .iter()
                .map(|&index| {
                    unsafe { std::ffi::CStr::from_ptr(palette.name_ptr(index as usize)) }
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<String>>()
        };

        let first = OxideGenerator::open(reference(), "minecraft:overworld", 1234).unwrap();
        let bare = generate(&first, 3, -2);
        assert!(
            bare.iter().all(|name| !name.contains("_ore")),
            "terrain alone has no ore blobs"
        );
        assert!(first.set_decoration(true).unwrap());
        let neighbour = generate(&first, 4, -2);
        let decorated = generate(&first, 3, -2);
        assert!(decorated.iter().any(|name| name.contains("_ore")));

        let second = OxideGenerator::open(reference(), "minecraft:overworld", 1234).unwrap();
        second.set_decoration(true).unwrap();
        assert_eq!(generate(&second, 3, -2), decorated);
        assert_eq!(generate(&second, 4, -2), neighbour);
    }

    /// `oxide_take_chunk_extras` hands out the last generated chunk's extras in the line
    /// format, asks for a bigger buffer without writing, is empty with decoration off, and
    /// refuses a chunk this thread did not generate last.
    #[test]
    fn chunk_extras_come_out_through_the_c_export() {
        if !reference().join("block_facts.tsv").is_file() {
            eprintln!("skipping: no block facts in {}", reference().display());
            return;
        }
        let handle = OxideGenerator::open(reference(), "minecraft:overworld", 883502277).unwrap();
        let generate = |x: i32, z: i32| {
            let mut blocks = vec![0u16; handle.block_buffer_len()];
            let mut biomes = vec![0u16; handle.biome_buffer_len()];
            handle
                .generate_chunk(x, z, &mut blocks, &mut biomes)
                .unwrap();
        };
        let take = |x: i32, z: i32, cap: usize| -> (i64, Vec<u8>) {
            let mut out = vec![0xAAu8; cap];
            let needed =
                unsafe { crate::oxide_take_chunk_extras(&handle, x, z, out.as_mut_ptr(), cap) };
            (needed, out)
        };

        generate(0, 0);
        assert_eq!(take(0, 0, 0).0, 0, "decoration off: nothing");

        handle.set_decoration(true).unwrap();
        generate(0, 0);
        let (needed, untouched) = take(0, 0, 4);
        assert!(needed > 4, "chunk (0, 0) has extras");
        assert!(
            untouched.iter().all(|&b| b == 0xAA),
            "too small: nothing written"
        );
        let (again, out) = take(0, 0, needed as usize);
        assert_eq!(again, needed);
        let text = std::str::from_utf8(&out).unwrap();
        assert!(text.ends_with('\n'));
        for line in text.lines() {
            let words: Vec<&str> = line.splitn(5, ' ').collect();
            let coords: Vec<i32> = words[1..4].iter().map(|w| w.parse().unwrap()).collect();
            assert!(
                (0..16).contains(&coords[0]) && (0..16).contains(&coords[2]),
                "{line}"
            );
            match words[0] {
                "be" => assert!(words[4].starts_with("{id:\""), "{line}"),
                "ft" | "bt" => assert!(words[4].starts_with("minecraft:"), "{line}"),
                "pp" => assert_eq!(words.len(), 4, "{line}"),
                other => panic!("unknown kind {other}"),
            }
        }

        assert_eq!(take(1, 0, 0).0, -2, "not the last chunk generated here");
        assert_eq!(
            unsafe {
                crate::oxide_take_chunk_extras(std::ptr::null(), 0, 0, std::ptr::null_mut(), 0)
            },
            -1
        );
    }

    /// Answering `getBaseHeight` for a chunk's 256 columns must not cost more than generating
    /// that chunk. It once cost thirty-two times more: `base_height` built the chunk's density
    /// caches per call, and building them means evaluating five `interpolated` nodes at 1225
    /// cell corners each. Bukkit asks this per column during structure placement, so the
    /// regression was invisible offline and dominated the server.
    ///
    /// A ratio on one machine rather than an absolute time, with a wide margin, so this fails
    /// on the shape of the bug and not on how fast the host is.
    #[test]
    fn asking_about_a_chunks_columns_costs_less_than_generating_it() {
        if !reference().is_dir() {
            eprintln!(
                "skipping: no extracted vanilla export at {}",
                reference().display()
            );
            return;
        }
        let handle = OxideGenerator::open(reference(), "minecraft:overworld", 1234).unwrap();
        let mut blocks = vec![0u16; 256 * 384];
        let mut biomes = vec![0u16; 64 * 24];
        // Warm: the first call of either kind pays for the caches the rest reuse.
        handle
            .generate_chunk(0, 0, &mut blocks, &mut biomes)
            .unwrap();
        handle.base_height(0, 0, HeightmapType::OceanFloorWg);

        let t = Instant::now();
        for i in 1..5 {
            handle
                .generate_chunk(i, 0, &mut blocks, &mut biomes)
                .unwrap();
        }
        let per_chunk = t.elapsed().as_secs_f64() / 4.0;

        let t = Instant::now();
        for i in 1..5 {
            for z in 0..16 {
                for x in 0..16 {
                    std::hint::black_box(handle.base_height(
                        i * 16 + x,
                        z,
                        HeightmapType::OceanFloorWg,
                    ));
                }
            }
        }
        let per_column_set = t.elapsed().as_secs_f64() / 4.0;

        assert!(
            per_column_set < per_chunk * 5.0,
            "256 base_height calls cost {per_column_set:.4}s against {per_chunk:.4}s to \
             generate the chunk they are asking about"
        );
    }
}
