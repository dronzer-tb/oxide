//! oxide-harness — offline Rust-vs-vanilla divergence harness. See `docs/ARCHITECTURE.md`.
//!
//! Currently a "generate, hash, and self-check" tool, not yet a differ: there is no vanilla
//! reference chunk dump to diff against (see `docs/ROADMAP.md`'s "Known unknowns" — that's a
//! gitignored, manually-extracted artifact nobody has produced for 26.2 yet). Once one exists,
//! `merkle::diverging_sections` is what localizes a mismatch.

mod invariants;
mod merkle;

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use clap::Parser;

use oxide_biome::BiomeSearchTree;
use oxide_chunkgen::{generate_chunk, BiomeTemperatures, CarverSetup};
use oxide_core::{BlockState, ChunkPos, ResourceLocation};
use oxide_datapack::{load_datapack, BiomeSource};
use oxide_noise::NoiseRouterEvaluator;

use invariants::{biome_ids_are_registered, heightmaps_match_surface};
use merkle::{build_merkle, diverging_sections};

#[derive(Parser)]
#[command(name = "oxide-harness")]
struct Args {
    /// Path to a datapack-shaped directory (`<path>/data/...`).
    #[arg(long)]
    datapack: PathBuf,
    /// `worldgen/dimension` id to generate.
    #[arg(long, default_value = "minecraft:overworld")]
    dimension: String,
    /// World seed.
    #[arg(long)]
    seed: i64,
    /// Chunks from `-radius..=radius` on each axis around the origin.
    #[arg(long, default_value_t = 2)]
    radius: i32,
    /// Merkle leaf cube side length (must evenly divide 16).
    #[arg(long, default_value_t = 4)]
    leaf_size: usize,
    /// Skip surface-rule evaluation, generating noise terrain only. For splitting where
    /// generation time actually goes -- not a mode anything should ship with.
    #[arg(long)]
    skip_surface: bool,
    /// Generate only, skipping the merkle build, invariant checks and the determinism
    /// re-generation, so a timing run measures generation and nothing else.
    #[arg(long)]
    time_only: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let datapack = load_datapack(&args.datapack)
        .with_context(|| format!("loading datapack at {}", args.datapack.display()))?;

    let dim_id = ResourceLocation::from_str(&args.dimension)
        .map_err(|e| anyhow!("invalid --dimension {:?}: {e}", args.dimension))?;
    let dimension = datapack
        .dimensions
        .get(&dim_id)
        .ok_or_else(|| anyhow!("dimension {dim_id} not found in datapack"))?;
    let noise_settings_id = dimension
        .generator
        .settings
        .as_ref()
        .ok_or_else(|| anyhow!("dimension {dim_id}'s generator has no noise_settings (not a minecraft:noise generator?)"))?;
    let settings = datapack
        .noise_settings
        .get(noise_settings_id)
        .ok_or_else(|| anyhow!("noise_settings {noise_settings_id} not found in datapack"))?;

    let router = NoiseRouterEvaluator::new(
        args.seed,
        settings,
        &datapack.density_functions,
        &datapack.noise_params,
    );

    let biome_tree = match dimension.generator.biome_source.as_ref() {
        Some(BiomeSource::MultiNoise(source)) => BiomeSearchTree::from_source(source),
        _ => None, // Fixed/Checkerboard/TheEnd/Unknown: not wired to a search tree yet.
    };
    if biome_tree.is_none() {
        tracing::warn!(
            "no explicit multi-noise biome source resolved for {dim_id} — chunks will keep the single fallback biome"
        );
    }

    let air = BlockState::new(ResourceLocation::minecraft("air"));

    // Surface rules need each biome's base temperature (the `minecraft:temperature`
    // condition); the rest of the biome definition is not read by this pass.
    let biome_temperatures: BiomeTemperatures = datapack
        .biomes
        .iter()
        .map(|(id, biome)| (id.clone(), biome.temperature))
        .collect();

    // Carvers a source chunk's biome configures. Vanilla looks the biome up per source chunk;
    // this resolves the ids once and hands back the same list, which is right for a pack whose
    // overworld biomes all name the same carvers and is flagged where it is not.
    let carver_ids: Vec<oxide_core::ResourceLocation> = datapack
        .biomes
        .iter()
        .flat_map(|(_, biome)| biome.carvers.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let configured: Vec<oxide_datapack::ConfiguredCarver> = carver_ids
        .iter()
        .filter_map(|id| datapack.configured_carvers.get(id).cloned())
        .collect();
    println!("carvers in play: {}", configured.len());
    let carvers_at = |_pos: ChunkPos| configured.clone();
    let carver_setup = CarverSetup {
        seed: args.seed,
        carvers_at: &carvers_at,
    };

    let mut air_below_zero = 0usize;
    let mut underground_water = 0usize;
    let mut underground_lava = 0usize;
    let mut vein_census: std::collections::BTreeMap<String, usize> = Default::default();
    let mut surface_census: std::collections::BTreeMap<String, usize> = Default::default();
    let mut total = 0usize;
    let mut with_failures = 0usize;
    let mut failure_counts: std::collections::HashMap<&'static str, usize> = Default::default();

    for cx in -args.radius..=args.radius {
        for cz in -args.radius..=args.radius {
            total += 1;
            let chunk = if args.skip_surface {
                oxide_chunkgen::fill_chunk(
                    ChunkPos::new(cx, cz),
                    settings,
                    &router,
                    biome_tree.as_ref(),
                )
            } else {
                generate_chunk(
                    ChunkPos::new(cx, cz),
                    settings,
                    &router,
                    biome_tree.as_ref(),
                    &biome_temperatures,
                    Some(&carver_setup),
                )
            };
            if args.time_only {
                std::hint::black_box(&chunk);
                continue;
            }
            let tree = build_merkle(&chunk, args.leaf_size);

            // Determinism self-check: the same seed and position must fill identically every
            // time, with no vanilla reference needed to catch a regression here. Exercises
            // `diverging_sections` for real, not just in unit tests, since there's no vanilla
            // tree to diff against yet (see module doc).
            let rebuilt = generate_chunk(
                ChunkPos::new(cx, cz),
                settings,
                &router,
                biome_tree.as_ref(),
                &biome_temperatures,
                Some(&carver_setup),
            );
            let rebuilt_tree = build_merkle(&rebuilt, args.leaf_size);
            let nondeterministic_sections = diverging_sections(&tree, &rebuilt_tree);

            for section in chunk.sections.iter() {
                let section_min_y = (section.y as i32) * 16;
                for index in 0..4096usize {
                    let name = section.block_states.get(index).name.path().to_string();
                    // Fluid above sea level, or lava anywhere, can only come from an aquifer:
                    // the old placeholder rule put water strictly below sea level.
                    if name == "water" && section_min_y > settings.sea_level {
                        underground_water += 1;
                    }
                    if name == "lava" {
                        underground_lava += 1;
                    }
                    if matches!(
                        name.as_str(),
                        "copper_ore"
                            | "deepslate_iron_ore"
                            | "raw_copper_block"
                            | "raw_iron_block"
                            | "granite"
                            | "tuff"
                    ) {
                        *vein_census.entry(name).or_insert(0usize) += 1;
                    }
                }
            }

            // Air below y=0 is the carver's signature: the noise fill never leaves any there.
            for section in chunk.sections.iter() {
                if (section.y as i32) * 16 >= 0 {
                    continue;
                }
                for index in 0..4096usize {
                    if section.block_states.get(index).name.path() == "air" {
                        air_below_zero += 1;
                    }
                }
            }

            for local_z in 0..16usize {
                for local_x in 0..16usize {
                    if let Some(name) =
                        top_solid_block(&chunk, local_x, local_z, &air, &settings.default_fluid)
                    {
                        *surface_census.entry(name).or_insert(0usize) += 1;
                    }
                }
            }

            let mut failures = heightmaps_match_surface(&chunk, &air, &settings.default_fluid);
            failures.extend(biome_ids_are_registered(&chunk, &datapack.biomes));
            if !nondeterministic_sections.is_empty() {
                failures.push(invariants::InvariantFailure {
                    check: "fill_is_deterministic",
                    detail: format!(
                        "{} of {} sections differ across repeated fills",
                        nondeterministic_sections.len(),
                        tree.section_hashes.len()
                    ),
                });
            }

            if failures.is_empty() {
                tracing::info!(x = cx, z = cz, hash = %tree.chunk_hash.to_hex(), "ok");
            } else {
                with_failures += 1;
                for f in &failures {
                    *failure_counts.entry(f.check).or_insert(0) += 1;
                    tracing::warn!(x = cx, z = cz, check = f.check, detail = %f.detail, "invariant failed");
                }
            }
        }
    }

    println!("air blocks below y=0 (carved): {air_below_zero}");
    println!("aquifer fluids: {underground_water} water above sea level, {underground_lava} lava");
    println!("ore vein blocks:");
    for (name, count) in vein_census.iter() {
        println!("  {count:>7}  {name}");
    }
    println!("surface blocks (top of each column):");
    let mut census: Vec<_> = surface_census.iter().collect();
    census.sort_by(|a, b| b.1.cmp(a.1));
    for (name, count) in census.iter().take(12) {
        println!("  {count:>7}  {name}");
    }
    println!();
    println!("chunks checked:  {total}");
    println!("chunks clean:    {}", total - with_failures);
    println!("chunks flagged:  {with_failures}");
    for (check, count) in &failure_counts {
        println!("  {check}: {count} failure(s)");
    }
    println!(
        "\nno vanilla reference dump available — this run reports self-consistency only, not \
         Java parity (see docs/ROADMAP.md's Known Unknowns)."
    );

    if with_failures > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Name of the highest block in one column that is neither air nor the dimension's fluid --
/// the surface material itself, which is the quickest read on whether surface rules ran. Skips
/// fluid deliberately: an ocean column would otherwise report water and say nothing about the
/// seabed the rules placed under it.
fn top_solid_block(
    chunk: &oxide_core::ChunkData,
    local_x: usize,
    local_z: usize,
    air: &BlockState,
    fluid: &BlockState,
) -> Option<String> {
    for section in chunk.sections.iter().rev() {
        for local_y in (0..16usize).rev() {
            let index = (local_y * 16 + local_z) * 16 + local_x;
            let state = section.block_states.get(index);
            if state != air && state != fluid {
                return Some(state.name.to_string());
            }
        }
    }
    None
}
