//! Parity against the real game: compares `scripts/ChunkOracle.java` dumps with Oxide.
//!
//! Each dump holds one centre chunk C as the game made it: the 3x3 around C at CARVERS
//! (terrain, before any feature), the same 3x3 after only C was brought to FEATURES, C's biome
//! cells and C's heightmaps. Oxide is asked for exactly the same things, wired the way
//! `oxide-ffi`'s `OxideGenerator` wires them (`open` + `terrain`): `generate_chunk` with each
//! source chunk's biome carvers for terrain, and `BiomeDecorator` run once over a
//! `DecorationRegion` of the 3x3 undecorated chunks for C's feature writes -- which is the
//! per-chunk write list `DecorationCache` keeps.
//!
//! Feature writes are compared as net effect: a block counts as written when its final state
//! differs from the terrain under it, for the game (FEATURES dump vs CARVERS dump) and for
//! Oxide (C's writes applied over Oxide's own terrain) alike. Where terrain already differs a
//! write can only be judged by its final state, so feature figures are read next to the terrain
//! ones, never alone.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use rayon::prelude::*;
use serde::Serialize;

use oxide_biome::BiomeSearchTree;
use oxide_chunkgen::decorate::heightmaps;
use oxide_chunkgen::{
    generate_chunk, BiomeDecorator, BiomeTemperatures, CarverSetup, DecorationRegion, Decorator,
};
use oxide_core::{BlockState, ChunkData, ChunkPos, HeightmapType, ResourceLocation};
use oxide_datapack::{load_datapack, BiomeSource, MultiNoiseSource, NoiseGeneratorSettings};
use oxide_noise::NoiseRouterEvaluator;

// ------------------------------------------------------------------------------- dump

struct Dump {
    path: PathBuf,
    center: ChunkPos,
    min_y: i32,
    height: i32,
    palette: Vec<BlockState>,
    biome_names: Vec<String>,
    carvers: Vec<(ChunkPos, Vec<u32>)>,
    features: HashMap<ChunkPos, Vec<u32>>,
    biome: Vec<u32>,
    heightmaps: Vec<(String, String, Vec<i32>)>,
}

fn parse_state(text: &str) -> Result<BlockState> {
    let (name, props) = match text.find('[') {
        Some(at) => (&text[..at], Some(text[at + 1..].trim_end_matches(']'))),
        None => (text, None),
    };
    let mut state = BlockState::new(
        ResourceLocation::from_str(name).map_err(|e| anyhow!("bad block id {name:?}: {e}"))?,
    );
    if let Some(props) = props {
        for pair in props.split(',') {
            let (k, v) = pair
                .split_once('=')
                .ok_or_else(|| anyhow!("bad property {pair:?}"))?;
            state = state.with_property(k, v);
        }
    }
    Ok(state)
}

fn unrle(line: &str, expect: usize) -> Result<Vec<u32>> {
    let mut out = Vec::with_capacity(expect);
    for token in line.split_ascii_whitespace() {
        let (id, count) = match token.split_once('*') {
            Some((id, count)) => (id.parse::<u32>()?, count.parse::<usize>()?),
            None => (token.parse::<u32>()?, 1),
        };
        out.extend(std::iter::repeat_n(id, count));
    }
    if out.len() != expect {
        bail!(
            "run-length line has {} entries, expected {expect}",
            out.len()
        );
    }
    Ok(out)
}

fn load_dump(path: &Path) -> Result<Dump> {
    let file = std::fs::File::open(path)?;
    let mut lines = BufReader::new(flate2::read::GzDecoder::new(file)).lines();
    let mut next = || -> Result<String> {
        lines
            .next()
            .ok_or_else(|| anyhow!("truncated dump"))?
            .map_err(Into::into)
    };
    if next()? != "oxide-oracle 1" {
        bail!("not an oxide-oracle 1 dump");
    }
    let mut dump = Dump {
        path: path.to_path_buf(),
        center: ChunkPos::new(0, 0),
        min_y: 0,
        height: 0,
        palette: Vec::new(),
        biome_names: Vec::new(),
        carvers: Vec::new(),
        features: HashMap::new(),
        biome: Vec::new(),
        heightmaps: Vec::new(),
    };
    loop {
        let line = next()?;
        let words: Vec<&str> = line.split(' ').collect();
        let pos = |a: usize| -> Result<ChunkPos> {
            Ok(ChunkPos::new(words[a].parse()?, words[a + 1].parse()?))
        };
        match words[0] {
            "end" => break,
            "seed" | "dimension" => {}
            "center" => dump.center = pos(1)?,
            "min_y" => {
                dump.min_y = words[1].parse()?;
                dump.height = words[3].parse()?;
            }
            "palette" => {
                for _ in 0..words[1].parse::<usize>()? {
                    dump.palette.push(parse_state(&next()?)?);
                }
            }
            "biomes" => {
                for _ in 0..words[1].parse::<usize>()? {
                    dump.biome_names.push(next()?);
                }
            }
            "blocks" => {
                let at = pos(2)?;
                let ids = unrle(&next()?, (dump.height * 256) as usize)?;
                if words[1] == "carvers" {
                    dump.carvers.push((at, ids));
                } else {
                    dump.features.insert(at, ids);
                }
            }
            "biome" => dump.biome = unrle(&next()?, (dump.height / 4 * 16) as usize)?,
            "heightmap" => {
                let values = next()?
                    .split(' ')
                    .map(|v| v.parse::<i32>())
                    .collect::<Result<Vec<_>, _>>()?;
                dump.heightmaps
                    .push((words[1].to_string(), words[2].to_string(), values));
            }
            other => bail!("unknown dump record {other:?}"),
        }
    }
    Ok(dump)
}

// ------------------------------------------------------------------------------- oxide

/// `OxideGenerator::open` + `terrain`, minus the FFI palettes.
struct Oxide {
    settings: NoiseGeneratorSettings,
    router: NoiseRouterEvaluator,
    biome_tree: Option<BiomeSearchTree>,
    biome_temperatures: BiomeTemperatures,
    biome_carvers: HashMap<ResourceLocation, std::sync::Arc<[oxide_datapack::ConfiguredCarver]>>,
    seed: i64,
    decorator: BiomeDecorator,
    /// Per step, per global index: every placed-feature id with that content.
    feature_order: Vec<Vec<Vec<String>>>,
}

impl Oxide {
    fn open(datapack_path: &Path, dimension_id: &str, seed: i64) -> Result<Self> {
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
        let feature_order = oxide_feature_order(&datapack, &possible_biomes)?;
        Ok(Self {
            settings,
            router,
            biome_tree,
            biome_temperatures,
            biome_carvers,
            seed,
            decorator,
            feature_order,
        })
    }

    fn terrain(&self, pos: ChunkPos) -> ChunkData {
        let carvers_at = |source: ChunkPos| -> std::sync::Arc<[oxide_datapack::ConfiguredCarver]> {
            let Some(tree) = self.biome_tree.as_ref() else {
                return std::sync::Arc::from([]);
            };
            let sample = oxide_biome::ClimateSample::sample(
                &self.router,
                source.min_block_x(),
                0,
                source.min_block_z(),
            );
            match tree.nearest(sample) {
                Some(biome) => self
                    .biome_carvers
                    .get(biome)
                    .cloned()
                    .unwrap_or_else(|| std::sync::Arc::from([])),
                None => std::sync::Arc::from([]),
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
}

/// The ordering `BiomeDecorator::new` builds, keyed the same way (placed-feature content), with
/// the ids behind each key so it can be read against the game's `feature_order.txt`.
fn oxide_feature_order(
    datapack: &oxide_datapack::Datapack,
    possible_biomes: &[ResourceLocation],
) -> Result<Vec<Vec<Vec<String>>>> {
    let mut ids_of: HashMap<String, Vec<String>> = HashMap::new();
    let mut lists: HashMap<ResourceLocation, Vec<Vec<String>>> = HashMap::new();
    for (biome_id, biome) in datapack.biomes.iter() {
        let mut per_step = Vec::new();
        for step in &biome.features {
            let mut keys = Vec::new();
            for feature_id in step {
                let ir = datapack
                    .placed_features
                    .get(feature_id)
                    .ok_or_else(|| anyhow!("unknown placed feature {feature_id}"))?;
                let content = serde_json::to_string(ir)?;
                let ids = ids_of.entry(content.clone()).or_default();
                if !ids.contains(&feature_id.to_string()) {
                    ids.push(feature_id.to_string());
                }
                keys.push(content);
            }
            per_step.push(keys);
        }
        lists.insert(biome_id.clone(), per_step);
    }
    let input = possible_biomes
        .iter()
        .map(|b| {
            lists
                .get(b)
                .cloned()
                .ok_or_else(|| anyhow!("biome {b} not in datapack"))
        })
        .collect::<Result<Vec<_>>>()?;
    let steps = oxide_chunkgen::feature_order::build_features_per_step(&input)
        .map_err(|cycle| anyhow!("{cycle}"))?;
    Ok(steps
        .into_iter()
        .map(|step| step.into_iter().map(|key| ids_of[&key].clone()).collect())
        .collect())
}

#[derive(Serialize, Default)]
pub struct FeatureOrderCheck {
    pub vanilla_entries: usize,
    pub oxide_entries: usize,
    /// (step, index, vanilla id, oxide ids) for every slot that disagrees.
    pub mismatches: Vec<(usize, usize, String, String)>,
}

fn check_feature_order(path: &Path, oxide: &[Vec<Vec<String>>]) -> Result<FeatureOrderCheck> {
    let mut check = FeatureOrderCheck {
        oxide_entries: oxide.iter().map(Vec::len).sum(),
        ..Default::default()
    };
    let mut vanilla: Vec<Vec<String>> = Vec::new();
    for line in std::fs::read_to_string(path)?.lines() {
        let w: Vec<&str> = line.split(' ').collect();
        let (step, index): (usize, usize) = (w[0].parse()?, w[1].parse()?);
        while vanilla.len() <= step {
            vanilla.push(Vec::new());
        }
        if vanilla[step].len() != index {
            bail!("feature_order.txt out of order at {line}");
        }
        vanilla[step].push(w[2].to_string());
        check.vanilla_entries += 1;
    }
    for step in 0..vanilla.len().max(oxide.len()) {
        let v = vanilla.get(step).map(Vec::as_slice).unwrap_or(&[]);
        let o = oxide.get(step).map(Vec::as_slice).unwrap_or(&[]);
        for index in 0..v.len().max(o.len()) {
            let vid = v.get(index).cloned().unwrap_or_else(|| "-".into());
            let oids = o.get(index).cloned().unwrap_or_default();
            if !oids.contains(&vid) {
                check.mismatches.push((step, index, vid, oids.join("|")));
            }
        }
    }
    Ok(check)
}

fn block_at(chunk: &ChunkData, x: usize, y_rel: usize, z: usize) -> &BlockState {
    chunk.sections[y_rel >> 4]
        .block_states
        .get(((y_rel & 15) * 16 + z) * 16 + x)
}

fn set_block_at(chunk: &mut ChunkData, x: usize, y_rel: usize, z: usize, state: BlockState) {
    chunk.sections[y_rel >> 4]
        .block_states
        .set(((y_rel & 15) * 16 + z) * 16 + x, state);
}

// ------------------------------------------------------------------------------- tallies

/// Pair key: block ids when they differ, full states when only properties differ.
fn pair_key(vanilla: &BlockState, oxide: &BlockState) -> String {
    if vanilla.name != oxide.name {
        format!("{} -> {}", vanilla.name, oxide.name)
    } else {
        format!("{vanilla} -> {oxide}")
    }
}

#[derive(Default, Serialize, Clone)]
pub struct Tally {
    pub total: u64,
    pub mismatched: u64,
    pub pairs: BTreeMap<String, u64>,
    /// 32-block Y bands keyed by their lowest y: (blocks, mismatched).
    pub bands: BTreeMap<i32, (u64, u64)>,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.total += other.total;
        self.mismatched += other.mismatched;
        for (k, v) in &other.pairs {
            *self.pairs.entry(k.clone()).or_default() += v;
        }
        for (k, (t, m)) in &other.bands {
            let e = self.bands.entry(*k).or_default();
            e.0 += t;
            e.1 += m;
        }
    }
}

#[derive(Default, Serialize, Clone)]
pub struct FeatureTally {
    /// Both changed the block to the same final state.
    pub matched: u64,
    /// Both changed it, to different final states: "vanilla -> oxide".
    pub different: BTreeMap<String, u64>,
    /// Only the game changed it: final vanilla block id.
    pub vanilla_only: BTreeMap<String, u64>,
    /// Only Oxide changed it: final Oxide block id.
    pub oxide_only: BTreeMap<String, u64>,
    pub vanilla_writes: u64,
    pub oxide_writes: u64,
}

impl FeatureTally {
    fn add(&mut self, o: &FeatureTally) {
        self.matched += o.matched;
        self.vanilla_writes += o.vanilla_writes;
        self.oxide_writes += o.oxide_writes;
        for (dst, src) in [
            (&mut self.different, &o.different),
            (&mut self.vanilla_only, &o.vanilla_only),
            (&mut self.oxide_only, &o.oxide_only),
        ] {
            for (k, v) in src {
                *dst.entry(k.clone()).or_default() += v;
            }
        }
    }
    fn sum(map: &BTreeMap<String, u64>) -> u64 {
        map.values().sum()
    }
}

#[derive(Serialize, Clone)]
pub struct ChunkReport {
    pub file: String,
    pub center: (i32, i32),
    pub surface_biome: String,
    /// All nine chunks at CARVERS.
    pub terrain: Tally,
    pub biome_cells: u64,
    pub biome_mismatched: u64,
    pub biome_pairs: BTreeMap<String, u64>,
    /// "phase TYPE" -> (columns, mismatched columns, sum of |difference|).
    pub heightmaps: BTreeMap<String, (u64, u64, u64)>,
    pub features: FeatureTally,
    pub views: Views,
}

#[derive(Serialize)]
pub struct Report {
    pub dimension: String,
    pub seed: i64,
    pub chunks: Vec<ChunkReport>,
    pub unsupported: Vec<(String, u64)>,
    pub feature_order: Option<FeatureOrderCheck>,
}

// ------------------------------------------------------------------------------- compare

fn compare(dump: &Dump, oxide: &Oxide) -> Result<ChunkReport> {
    let nine: Vec<ChunkPos> = dump.carvers.iter().map(|(p, _)| *p).collect();
    let terrain: Vec<ChunkData> = nine.par_iter().map(|p| oxide.terrain(*p)).collect();
    let index_of = |p: ChunkPos| nine.iter().position(|q| *q == p).expect("chunk in dump");
    for chunk in &terrain {
        if chunk.min_y != dump.min_y || chunk.height != dump.height {
            bail!(
                "Oxide world height {}+{} != dump {}+{}",
                chunk.min_y,
                chunk.height,
                dump.min_y,
                dump.height
            );
        }
    }

    // Terrain, all nine chunks.
    let mut t = Tally::default();
    for (i, (_, ids)) in dump.carvers.iter().enumerate() {
        let ox = &terrain[i];
        for (n, id) in ids.iter().enumerate() {
            let x = n & 15;
            let z = (n >> 4) & 15;
            let y_rel = n >> 8;
            let band = dump.min_y + (y_rel as i32 / 32) * 32;
            let v = &dump.palette[*id as usize];
            let o = block_at(ox, x, y_rel, z);
            t.total += 1;
            let e = t.bands.entry(band).or_default();
            e.0 += 1;
            if v != o {
                t.mismatched += 1;
                e.1 += 1;
                *t.pairs.entry(pair_key(v, o)).or_default() += 1;
            }
        }
    }

    // Biomes, centre chunk.
    let centre = &terrain[index_of(dump.center)];
    let mut biome_cells = 0;
    let mut biome_mismatched = 0;
    let mut biome_pairs = BTreeMap::new();
    for (n, id) in dump.biome.iter().enumerate() {
        let qx = n & 3;
        let qz = (n >> 2) & 3;
        let qy = n >> 4;
        let v = &dump.biome_names[*id as usize];
        let o = centre.sections[qy / 4]
            .biomes
            .get(((qy & 3) * 4 + qz) * 4 + qx)
            .to_string();
        biome_cells += 1;
        if *v != o {
            biome_mismatched += 1;
            *biome_pairs.entry(format!("{v} -> {o}")).or_default() += 1;
        }
    }
    let mid = &dump.biome_names[dump.biome[dump.biome.len() / 2] as usize];

    // Oxide's writes for the centre, as DecorationCache::writes computes them.
    let refs: [&ChunkData; 9] = std::array::from_fn(|i| &terrain[i]);
    // DecorationRegion wants the ring in dz-major, dx-minor order -- the order the dump uses.
    for (i, p) in nine.iter().enumerate() {
        let expect = ChunkPos::new(
            dump.center.x + (i % 3) as i32 - 1,
            dump.center.z + (i / 3) as i32 - 1,
        );
        if *p != expect {
            bail!(
                "dump ring order {:?} at {i}, expected {:?}",
                (p.x, p.z),
                (expect.x, expect.z)
            );
        }
    }
    let mut region = DecorationRegion::new(dump.center, refs, oxide.decorator.facts());
    oxide.decorator.decorate(&mut region);
    let writes = region.into_writes();
    // ORACLE_TRACE=1, the Rust half of ChunkOracle's trace: C's writes in first-write order.
    if std::env::var_os("ORACLE_TRACE").is_some() {
        for (at, state) in &writes {
            eprintln!(
                "OXTRACE {} {} {} {} {state}",
                dump.center.x, at.x, at.y, at.z
            );
        }
    }
    let mut decorated: Vec<ChunkData> = terrain.clone();
    for (at, state) in writes {
        let cp = at.chunk_pos();
        let Some(i) = nine.iter().position(|q| *q == cp) else {
            continue;
        };
        let y_rel = at.y - dump.min_y;
        if y_rel < 0 || y_rel >= dump.height {
            continue;
        }
        set_block_at(
            &mut decorated[i],
            (at.x & 15) as usize,
            y_rel as usize,
            (at.z & 15) as usize,
            state,
        );
    }

    let mut f = FeatureTally::default();
    for (i, (p, before)) in dump.carvers.iter().enumerate() {
        let after = &dump.features[p];
        for n in 0..before.len() {
            let x = n & 15;
            let z = (n >> 4) & 15;
            let y_rel = n >> 8;
            let v_before = &dump.palette[before[n] as usize];
            let v_after = &dump.palette[after[n] as usize];
            let o_before = block_at(&terrain[i], x, y_rel, z);
            let o_after = block_at(&decorated[i], x, y_rel, z);
            let v_changed = v_before != v_after;
            let o_changed = o_before != o_after;
            f.vanilla_writes += v_changed as u64;
            f.oxide_writes += o_changed as u64;
            match (v_changed, o_changed) {
                (true, true) if v_after == o_after => f.matched += 1,
                (true, true) => *f.different.entry(pair_key(v_after, o_after)).or_default() += 1,
                (true, false) => *f.vanilla_only.entry(v_after.name.to_string()).or_default() += 1,
                (false, true) => *f.oxide_only.entry(o_after.name.to_string()).or_default() += 1,
                (false, false) => {}
            }
        }
    }

    // Heightmaps, centre chunk.
    let ci = index_of(dump.center);
    let decorated_maps = heightmaps(&decorated[ci], oxide.decorator.facts());
    let mut hm = BTreeMap::new();
    for (phase, kind, values) in &dump.heightmaps {
        let ty = match kind.as_str() {
            "WORLD_SURFACE_WG" => HeightmapType::WorldSurfaceWg,
            "OCEAN_FLOOR_WG" => HeightmapType::OceanFloorWg,
            "WORLD_SURFACE" => HeightmapType::WorldSurface,
            "OCEAN_FLOOR" => HeightmapType::OceanFloor,
            "MOTION_BLOCKING" => HeightmapType::MotionBlocking,
            "MOTION_BLOCKING_NO_LEAVES" => HeightmapType::MotionBlockingNoLeaves,
            other => bail!("unknown heightmap {other}"),
        };
        let map = if phase == "carvers" {
            terrain[ci].heightmaps.get(&ty)
        } else {
            decorated_maps.get(&ty)
        };
        let Some(map) = map else {
            hm.insert(format!("{phase} {kind}"), (256, 256, 0));
            continue;
        };
        let mut wrong = 0;
        let mut err = 0u64;
        for (n, v) in values.iter().enumerate() {
            let o = map.get(n & 15, n >> 4);
            if o != *v {
                wrong += 1;
                err += (o - v).unsigned_abs() as u64;
            }
        }
        hm.insert(format!("{phase} {kind}"), (256, wrong, err));
    }

    Ok(ChunkReport {
        file: dump
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        center: (dump.center.x, dump.center.z),
        surface_biome: mid.clone(),
        terrain: t,
        biome_cells,
        biome_mismatched,
        biome_pairs,
        heightmaps: hm,
        features: f,
        views: views(dump, &nine, &terrain, &decorated),
    })
}

pub fn run(datapack: &Path, root: &Path, dimension: &str, seed: i64) -> Result<Report> {
    report(
        &Oxide::open(datapack, dimension, seed)?,
        root,
        dimension,
        seed,
    )
}

fn report(oxide: &Oxide, root: &Path, dimension: &str, seed: i64) -> Result<Report> {
    let dim_path = dimension.rsplit(':').next().unwrap_or(dimension);
    let dir = root.join(seed.to_string()).join(dim_path);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading oracle dumps in {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".txt.gz"))
        .collect();
    files.sort();
    if files.is_empty() {
        bail!("no oracle dumps in {}", dir.display());
    }
    let order_file = dir.join("feature_order.txt");
    let feature_order = if order_file.exists() {
        Some(check_feature_order(&order_file, &oxide.feature_order)?)
    } else {
        None
    };
    let mut chunks = Vec::new();
    for file in &files {
        let dump = load_dump(file).with_context(|| format!("parsing {}", file.display()))?;
        chunks
            .push(compare(&dump, oxide).with_context(|| format!("comparing {}", file.display()))?);
    }
    Ok(Report {
        dimension: dimension.to_string(),
        seed,
        chunks,
        feature_order,
        unsupported: oxide
            .decorator
            .unsupported()
            .entries()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    })
}

// ------------------------------------------------------------------------------- site

fn is_air(state: &BlockState) -> bool {
    matches!(state.name.path(), "air" | "cave_air" | "void_air")
}

/// Interns ids so the website's arrays carry small integers, not strings.
#[derive(Default)]
struct Interner {
    ids: HashMap<String, u32>,
    names: Vec<String>,
}

impl Interner {
    fn id(&mut self, name: String) -> u32 {
        if let Some(id) = self.ids.get(&name) {
            return *id;
        }
        let id = self.names.len() as u32;
        self.names.push(name.clone());
        self.ids.insert(name, id);
        id
    }
}

/// Flat `[id, count, id, count, ...]`.
fn rle(ids: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for id in ids {
        match out.len() {
            n if n >= 2 && out[n - 2] == *id => out[n - 1] += 1,
            _ => out.extend([*id, 1]),
        }
    }
    out
}

/// What the website draws for one dump: the 48x48 columns of the 3x3 from above, and a
/// vertical cut through the centre chunk. Both worlds are the finished (decorated) ones.
#[derive(Serialize, Clone, Default)]
pub struct Views {
    pub min_y: i32,
    pub height: i32,
    /// Block ids the arrays below index.
    pub palette: Vec<String>,
    /// Row-major z then x, 48x48: highest non-air block and its y, per world.
    pub top_vanilla: Vec<u32>,
    pub top_oxide: Vec<u32>,
    pub y_vanilla: Vec<i32>,
    pub y_oxide: Vec<i32>,
    /// Per column: terrain blocks that differ, and feature writes that disagree.
    pub terrain_wrong: Vec<u32>,
    pub feature_wrong: Vec<u32>,
    /// Cut along x at the centre chunk's z=8, 48 wide, top row first, run-length coded.
    pub slice_vanilla: Vec<u32>,
    pub slice_oxide: Vec<u32>,
    /// The same cut as a 0/1 mask: 1 where the two worlds hold different blocks.
    pub slice_wrong: Vec<u32>,
}

impl Views {
    /// Drops what shows the game's own world, leaving Oxide's and where the two differ --
    /// the form that may be published (PRD 4, 8: game output stays on this machine).
    fn strip_game(&mut self) {
        self.top_vanilla = Vec::new();
        self.y_vanilla = Vec::new();
        self.slice_vanilla = Vec::new();
    }
}

fn views(dump: &Dump, nine: &[ChunkPos], terrain: &[ChunkData], decorated: &[ChunkData]) -> Views {
    let height = dump.height as usize;
    let mut names = Interner::default();
    let mut v = Views {
        min_y: dump.min_y,
        height: dump.height,
        ..Default::default()
    };
    let mut slice_v = Vec::with_capacity(48 * height);
    let mut slice_o = Vec::with_capacity(48 * height);
    for gz in 0..48 {
        for gx in 0..48 {
            let i = (gz / 16) * 3 + gx / 16;
            let (x, z) = (gx & 15, gz & 15);
            let before = &dump.carvers[i].1;
            let after = &dump.features[&nine[i]];
            let mut top_v = None;
            let mut top_o = None;
            let (mut terrain_wrong, mut feature_wrong) = (0, 0);
            for y in (0..height).rev() {
                let n = (y << 8) | (z << 4) | x;
                let vb = &dump.palette[before[n] as usize];
                let va = &dump.palette[after[n] as usize];
                let ob = block_at(&terrain[i], x, y, z);
                let oa = block_at(&decorated[i], x, y, z);
                terrain_wrong += (vb != ob) as u32;
                let (vc, oc) = (vb != va, ob != oa);
                feature_wrong += ((vc || oc) && !(vc && oc && va == oa)) as u32;
                if top_v.is_none() && !is_air(va) {
                    top_v = Some((va, y));
                }
                if top_o.is_none() && !is_air(oa) {
                    top_o = Some((oa, y));
                }
            }
            for (top, ids, ys) in [
                (top_v, &mut v.top_vanilla, &mut v.y_vanilla),
                (top_o, &mut v.top_oxide, &mut v.y_oxide),
            ] {
                let (name, y) = match top {
                    Some((state, y)) => (state.name.to_string(), dump.min_y + y as i32),
                    None => ("minecraft:air".to_string(), dump.min_y - 1),
                };
                ids.push(names.id(name));
                ys.push(y);
            }
            v.terrain_wrong.push(terrain_wrong);
            v.feature_wrong.push(feature_wrong);
        }
    }
    for y in (0..height).rev() {
        for gx in 0..48 {
            let i = 3 + gx / 16;
            let n = (y << 8) | (8 << 4) | (gx & 15);
            let va = &dump.palette[dump.features[&nine[i]][n] as usize];
            slice_v.push(names.id(va.name.to_string()));
            slice_o.push(names.id(block_at(&decorated[i], gx & 15, y, 8).name.to_string()));
        }
    }
    let wrong: Vec<u32> = slice_v
        .iter()
        .zip(&slice_o)
        .map(|(a, b)| (a != b) as u32)
        .collect();
    v.slice_wrong = rle(&wrong);
    v.slice_vanilla = rle(&slice_v);
    v.slice_oxide = rle(&slice_o);
    v.palette = names.names;
    v
}

/// Oxide's own finished world from above, `-radius..=radius` chunks around the origin: terrain
/// plus every chunk's feature writes, applied in chunk order.
#[derive(Serialize)]
pub struct WorldMap {
    pub radius: i32,
    /// Blocks per side.
    pub side: usize,
    pub min_y: i32,
    pub height: i32,
    pub palette: Vec<String>,
    pub biomes: Vec<String>,
    /// Row-major z then x, run-length coded: highest non-air block.
    pub top: Vec<u32>,
    pub y: Vec<i32>,
    /// One cell per 4x4 columns, run-length coded: the biome at the surface.
    pub biome: Vec<u32>,
    pub threads: usize,
    /// Wall time, all threads together, for `terrain_chunks` and `side/16` squared chunks.
    pub terrain_chunks: usize,
    pub terrain_ms: u64,
    pub decoration_ms: u64,
}

/// One region's serialized chunks, keyed by its region coordinates where collected.
type RegionChunks = Vec<(ChunkPos, Vec<u8>)>;

/// Writes `world` as Anvil regions under `<dir>/region`, for a map renderer to read. Not a
/// world to load in a server: heightmaps are the undecorated terrain's and there is no light.
fn save_world(dir: &Path, data_version: i32, world: &[ChunkData]) -> Result<()> {
    let region_dir = dir.join("region");
    std::fs::create_dir_all(&region_dir)?;
    let opts = oxide_anvil::nbt::ChunkNbtWriteOptions {
        data_version,
        last_update: 0,
        inhabited_time: 0,
    };
    let mut regions: BTreeMap<(i32, i32), RegionChunks> = BTreeMap::new();
    for chunk in world {
        let bytes = oxide_anvil::nbt::serialize_chunk(chunk, &opts)?;
        regions
            .entry(oxide_anvil::region::region_coords_for(&chunk.pos))
            .or_default()
            .push((chunk.pos, bytes));
    }
    for chunks in regions.values() {
        let file = region_dir.join(oxide_anvil::region::region_file_name(&chunks[0].0));
        oxide_anvil::region::write_region_file(&file, chunks)?;
    }
    Ok(())
}

fn world_map(oxide: &Oxide, radius: i32, save: Option<(&Path, i32)>) -> Result<WorldMap> {
    let outer = radius + 1;
    let width = (2 * outer + 1) as usize;
    let square = |r: i32| -> Vec<ChunkPos> {
        (-r..=r)
            .flat_map(|z| (-r..=r).map(move |x| ChunkPos::new(x, z)))
            .collect()
    };
    let started = std::time::Instant::now();
    let terrain: Vec<ChunkData> = square(outer)
        .par_iter()
        .map(|p| oxide.terrain(*p))
        .collect();
    let terrain_ms = started.elapsed().as_millis() as u64;
    let at = |x: i32, z: i32| &terrain[(z + outer) as usize * width + (x + outer) as usize];

    let inner = square(radius);
    let started = std::time::Instant::now();
    let writes: Vec<Vec<oxide_chunkgen::BlockWrite>> = inner
        .par_iter()
        .map(|c| {
            let refs: [&ChunkData; 9] =
                std::array::from_fn(|i| at(c.x + (i % 3) as i32 - 1, c.z + (i / 3) as i32 - 1));
            let mut region = DecorationRegion::new(*c, refs, oxide.decorator.facts());
            oxide.decorator.decorate(&mut region);
            region.into_writes()
        })
        .collect();
    let decoration_ms = started.elapsed().as_millis() as u64;

    let chunks = (2 * radius + 1) as usize;
    let mut world: Vec<ChunkData> = inner.iter().map(|c| at(c.x, c.z).clone()).collect();
    let (min_y, height) = (world[0].min_y, world[0].height);
    for (at, state) in writes.into_iter().flatten() {
        let cp = at.chunk_pos();
        let y = at.y - min_y;
        if cp.x.abs() > radius || cp.z.abs() > radius || y < 0 || y >= height {
            continue;
        }
        let i = (cp.z + radius) as usize * chunks + (cp.x + radius) as usize;
        set_block_at(
            &mut world[i],
            (at.x & 15) as usize,
            y as usize,
            (at.z & 15) as usize,
            state,
        );
    }

    if let Some((dir, data_version)) = save {
        save_world(dir, data_version, &world)
            .with_context(|| format!("writing the world to {}", dir.display()))?;
    }

    let side = chunks * 16;
    let mut blocks = Interner::default();
    let mut biomes = Interner::default();
    let mut top = Vec::with_capacity(side * side);
    let mut ys = Vec::with_capacity(side * side);
    let mut biome = Vec::with_capacity(side * side / 16);
    for gz in 0..side {
        for gx in 0..side {
            let chunk = &world[(gz / 16) * chunks + gx / 16];
            let (x, z) = (gx & 15, gz & 15);
            // Below the first air from the top, so a roofed dimension shows its floor, not its lid.
            let found = (0..height as usize)
                .rev()
                .find(|y| is_air(block_at(chunk, x, *y, z)))
                .and_then(|air| (0..air).rev().find(|y| !is_air(block_at(chunk, x, *y, z))));
            let y = found.unwrap_or(0);
            top.push(blocks.id(match found {
                Some(y) => block_at(chunk, x, y, z).name.to_string(),
                None => "minecraft:air".to_string(),
            }));
            ys.push(found.map_or(min_y - 1, |y| min_y + y as i32));
            if gx & 3 == 0 && gz & 3 == 0 {
                let cell = (((y & 15) >> 2) * 4 + (z >> 2)) * 4 + (x >> 2);
                biome.push(biomes.id(chunk.sections[y >> 4].biomes.get(cell).to_string()));
            }
        }
    }
    Ok(WorldMap {
        radius,
        side,
        min_y,
        height,
        palette: blocks.names,
        biomes: biomes.names,
        top: rle(&top),
        y: ys,
        biome: rle(&biome),
        threads: rayon::current_num_threads(),
        terrain_chunks: terrain.len(),
        terrain_ms,
        decoration_ms,
    })
}

#[derive(Serialize)]
struct SiteRun<'a> {
    dimension: &'a str,
    seed: i64,
    commit: &'a str,
    generated_unix: u64,
    /// True when the views of the game's own world were left out.
    public: bool,
    /// Absent when the comparison could not run; `error` then says why.
    report: Option<&'a Report>,
    error: Option<String>,
    map: WorldMap,
}

/// `run`, plus `<out>/run_<seed>_<dimension>.json` for the website and a refreshed
/// `<out>/manifest.json` naming every run file there. A comparison that fails still leaves a
/// run file, carrying the error and Oxide's map.
#[allow(clippy::too_many_arguments)]
pub fn site(
    datapack: &Path,
    root: &Path,
    dimension: &str,
    seed: i64,
    radius: i32,
    out: &Path,
    commit: &str,
    public: bool,
    world: Option<&Path>,
) -> Result<Report> {
    // The DataVersion comes from the game's own export, never from memory.
    let save = match world {
        Some(dir) => {
            let path = datapack.join("version.json");
            let version: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?,
            )?;
            let data_version = version["data_version"]
                .as_i64()
                .ok_or_else(|| anyhow!("{} has no data_version", path.display()))?;
            Some((dir, data_version as i32))
        }
        None => None,
    };
    let oxide = Oxide::open(datapack, dimension, seed)?;
    let mut report = report(&oxide, root, dimension, seed);
    if let (true, Ok(report)) = (public, report.as_mut()) {
        report.chunks.iter_mut().for_each(|c| c.views.strip_game());
    }
    let dim_path = dimension.rsplit(':').next().unwrap_or(dimension);
    std::fs::create_dir_all(out)?;
    let run = SiteRun {
        dimension,
        seed,
        commit,
        public,
        generated_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        report: report.as_ref().ok(),
        error: report.as_ref().err().map(|e| format!("{e:#}")),
        map: world_map(&oxide, radius, save)?,
    };
    std::fs::write(
        out.join(format!("run_{seed}_{dim_path}.json")),
        serde_json::to_string(&run)?,
    )?;
    let mut runs: Vec<String> = std::fs::read_dir(out)?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|name| name.starts_with("run_") && name.ends_with(".json"))
        .collect();
    runs.sort();
    std::fs::write(out.join("manifest.json"), serde_json::to_string(&runs)?)?;
    report
}

// ------------------------------------------------------------------------------- render

fn pct(part: u64, whole: u64) -> String {
    if whole == 0 {
        "-".into()
    } else {
        format!("{:.3}%", part as f64 * 100.0 / whole as f64)
    }
}

fn top(map: &BTreeMap<String, u64>, n: usize) -> Vec<(&String, &u64)> {
    let mut v: Vec<_> = map.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    v.truncate(n);
    v
}

impl Report {
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "## {} seed {}\n", self.dimension, self.seed);
        let _ = writeln!(
            s,
            "| centre | biome (mid) | terrain match | biome cells wrong | WG heightmap cols wrong | feature writes v / o | matched | differ | v-only | o-only |"
        );
        let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|");
        let mut terrain = Tally::default();
        let mut feats = FeatureTally::default();
        let mut biome = (0u64, 0u64);
        let mut biome_pairs: BTreeMap<String, u64> = BTreeMap::new();
        let mut hm: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
        for c in &self.chunks {
            terrain.add(&c.terrain);
            feats.add(&c.features);
            biome.0 += c.biome_cells;
            biome.1 += c.biome_mismatched;
            for (k, v) in &c.biome_pairs {
                *biome_pairs.entry(k.clone()).or_default() += v;
            }
            let mut wg = 0;
            for (k, (t, w, e)) in &c.heightmaps {
                let a = hm.entry(k.clone()).or_default();
                a.0 += t;
                a.1 += w;
                a.2 += e;
                if k.starts_with("carvers") {
                    wg += w;
                }
            }
            let f = &c.features;
            let _ = writeln!(
                s,
                "| {},{} | {} | {} | {}/{} | {}/512 | {} / {} | {} | {} | {} | {} |",
                c.center.0,
                c.center.1,
                c.surface_biome.trim_start_matches("minecraft:"),
                pct(c.terrain.total - c.terrain.mismatched, c.terrain.total),
                c.biome_mismatched,
                c.biome_cells,
                wg,
                f.vanilla_writes,
                f.oxide_writes,
                f.matched,
                FeatureTally::sum(&f.different),
                FeatureTally::sum(&f.vanilla_only),
                FeatureTally::sum(&f.oxide_only),
            );
        }
        let _ = writeln!(
            s,
            "\n| centre | largest terrain mismatches (vanilla -> oxide) |\n|---|---|"
        );
        for c in &self.chunks {
            let pairs: Vec<String> = top(&c.terrain.pairs, 4)
                .into_iter()
                .map(|(k, v)| format!("`{}` {v}", k.replace("minecraft:", "")))
                .collect();
            let _ = writeln!(
                s,
                "| {},{} | {} |",
                c.center.0,
                c.center.1,
                pairs.join("; ")
            );
        }
        let _ = writeln!(s, "\n### Aggregate\n");
        let _ = writeln!(
            s,
            "- terrain blocks (9 chunks per dump, CARVERS): {} compared, {} mismatched, match {}",
            terrain.total,
            terrain.mismatched,
            pct(terrain.total - terrain.mismatched, terrain.total)
        );
        let _ = writeln!(
            s,
            "- biome cells (centre): {} compared, {} mismatched, match {}",
            biome.0,
            biome.1,
            pct(biome.0 - biome.1, biome.0)
        );
        let union = feats.matched
            + FeatureTally::sum(&feats.different)
            + FeatureTally::sum(&feats.vanilla_only)
            + FeatureTally::sum(&feats.oxide_only);
        let _ = writeln!(
            s,
            "- feature writes: vanilla {} / oxide {}; same final state {} ({} of the union {}), different state {}, vanilla-only {}, oxide-only {}",
            feats.vanilla_writes,
            feats.oxide_writes,
            feats.matched,
            pct(feats.matched, union),
            union,
            FeatureTally::sum(&feats.different),
            FeatureTally::sum(&feats.vanilla_only),
            FeatureTally::sum(&feats.oxide_only)
        );
        let _ = writeln!(
            s,
            "\n| heightmap | columns | wrong | mean abs error of wrong |\n|---|---|---|---|"
        );
        for (k, (t, w, e)) in &hm {
            let mean = if *w == 0 { 0.0 } else { *e as f64 / *w as f64 };
            let _ = writeln!(s, "| {k} | {t} | {w} | {mean:.2} |");
        }
        let _ = writeln!(
            s,
            "\n| terrain Y band | blocks | mismatched | match |\n|---|---|---|---|"
        );
        for (band, (t, m)) in &terrain.bands {
            let _ = writeln!(
                s,
                "| {}..{} | {t} | {m} | {} |",
                band,
                band + 31,
                pct(t - m, *t)
            );
        }
        let section = |s: &mut String, title: &str, map: &BTreeMap<String, u64>, n: usize| {
            let _ = writeln!(s, "\n| {title} | count |\n|---|---|");
            for (k, v) in top(map, n) {
                let _ = writeln!(s, "| `{k}` | {v} |");
            }
        };
        section(&mut s, "terrain: vanilla -> oxide", &terrain.pairs, 25);
        section(&mut s, "biome: vanilla -> oxide", &biome_pairs, 15);
        section(
            &mut s,
            "features, vanilla-only (vanilla block)",
            &feats.vanilla_only,
            20,
        );
        section(
            &mut s,
            "features, oxide-only (oxide block)",
            &feats.oxide_only,
            20,
        );
        section(
            &mut s,
            "features, different final state",
            &feats.different,
            15,
        );
        if let Some(order) = &self.feature_order {
            let _ = writeln!(
                s,
                "\nFeature order (FeatureSorter): vanilla {} slots, oxide {} slots, {} disagree",
                order.vanilla_entries,
                order.oxide_entries,
                order.mismatches.len()
            );
            for (step, index, v, o) in order.mismatches.iter().take(12) {
                let _ = writeln!(s, "- step {step} index {index}: vanilla `{v}`, oxide `{o}`");
            }
        }
        if !self.unsupported.is_empty() {
            let _ = writeln!(
                s,
                "\n| Oxide decoration gaps (skipped) | count |\n|---|---|"
            );
            for (k, v) in &self.unsupported {
                let _ = writeln!(s, "| `{k}` | {v} |");
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_round_trip() {
        assert_eq!(unrle("3*2 1 0*3", 6).unwrap(), vec![3, 3, 1, 0, 0, 0]);
        assert!(unrle("1*2", 3).is_err());
        let s = parse_state("minecraft:oak_log[axis=y]").unwrap();
        assert_eq!(s.to_string(), "minecraft:oak_log[axis=y]");
    }

    /// Runs only where `scripts/ChunkOracle.java` output exists (it is git-ignored).
    #[test]
    #[ignore]
    fn oracle_dumps_compare() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
        let oracle = root.join("oracle");
        let Ok(seeds) = std::fs::read_dir(&oracle) else {
            eprintln!("no oracle dumps under {}", oracle.display());
            return;
        };
        for seed in seeds.flatten() {
            let Ok(seed_value) = seed.file_name().to_string_lossy().parse::<i64>() else {
                continue;
            };
            for dim in ["minecraft:overworld", "minecraft:the_nether"] {
                if !seed
                    .path()
                    .join(dim.trim_start_matches("minecraft:"))
                    .exists()
                {
                    continue;
                }
                let report = run(&root, &oracle, dim, seed_value).expect("oracle comparison");
                println!("{}", report.render());
            }
        }
    }
}
