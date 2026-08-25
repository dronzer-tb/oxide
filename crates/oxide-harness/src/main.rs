//! oxide-harness — offline Rust-vs-vanilla divergence harness. See `docs/ARCHITECTURE.md`.
//!
//! Diffs Oxide's chunks against a real vanilla world when `--reference` names one, and reports
//! self-consistency only when it does not. A divergence is reported down to world block
//! coordinates: the Merkle root narrows to a section, the section to a leaf, and the leaf to the
//! individual blocks, so the output names a place to teleport to rather than a chunk. See
//! `docs/REFERENCE_DATA.md` for producing a reference world (gitignored, never redistributed).
//!
//! Without `--reference` it still does what it always did: generate, hash, re-generate and
//! compare, which catches non-determinism without needing Java at all.

mod compare;
mod invariants;
mod merkle;
mod reference;

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use clap::Parser;

use rayon::prelude::*;

use oxide_biome::BiomeSearchTree;
use oxide_chunkgen::{generate_chunk, BiomeTemperatures, CarverSetup};
use oxide_core::{BlockState, ChunkPos, ResourceLocation};
use oxide_datapack::{load_datapack, BiomeSource};
use oxide_noise::NoiseRouterEvaluator;

use compare::compare_chunks;
use invariants::{biome_ids_are_registered, heightmaps_match_surface};
use merkle::{build_merkle, diverging_sections};
use reference::ReferenceWorld;

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
    /// A vanilla-generated world directory to diff against, e.g. `/srv/mc/world`. Without it
    /// this run reports self-consistency only, not Java parity. Reference worlds are gitignored
    /// and must never be committed -- see docs/REFERENCE_DATA.md.
    #[arg(long)]
    reference: Option<PathBuf>,
    /// Dimension subdirectory inside the reference world (`DIM-1` for the nether, `DIM1` for
    /// the end). Omit for the overworld, whose regions sit directly under the world folder.
    #[arg(long)]
    reference_dimension_dir: Option<String>,
    /// Differing blocks to print per chunk. Caps the detail, never the count.
    #[arg(long, default_value_t = 8)]
    max_reported_blocks: usize,
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
    /// Threads to generate on. Only honoured with `--time-only`: the invariant pass reports
    /// per-chunk findings in a fixed order, and generation itself is what threading is being
    /// measured for. One shared router and carver setup serve every thread.
    #[arg(long, default_value_t = 1)]
    threads: usize,
    /// Write a sampling-profiler flamegraph of the whole run to this path. Requires the
    /// `profile` cargo feature; ignored without it.
    #[cfg(feature = "profile")]
    #[arg(long)]
    profile_out: Option<PathBuf>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    #[cfg(feature = "profile")]
    let guard = args.profile_out.as_ref().map(|_| {
        pprof::ProfilerGuardBuilder::default()
            .frequency(999)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()
            .expect("starting the sampling profiler")
    });

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

    let positions: Vec<ChunkPos> = (-args.radius..=args.radius)
        .flat_map(|cx| (-args.radius..=args.radius).map(move |cz| ChunkPos::new(cx, cz)))
        .collect();
    let generate = |pos: ChunkPos| {
        if args.skip_surface {
            oxide_chunkgen::fill_chunk(pos, settings, &router, biome_tree.as_ref())
        } else {
            generate_chunk(
                pos,
                settings,
                &router,
                biome_tree.as_ref(),
                &biome_temperatures,
                Some(&carver_setup),
            )
        }
    };
    // One shared router and carver setup serve every thread; everything a chunk mutates -- its
    // caches, its aquifer -- is built inside `generate_chunk`. Results come back in position
    // order, so a threaded run and a single-threaded one are compared against the same
    // fingerprint.
    let pool = (args.threads > 1)
        .then(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(args.threads)
                .build()
                .context("building the generation thread pool")
        })
        .transpose()?;
    let generate_all = |positions: &[ChunkPos]| -> Vec<oxide_core::ChunkData> {
        match &pool {
            Some(pool) => pool.install(|| positions.par_iter().copied().map(generate).collect()),
            None => positions.iter().copied().map(generate).collect(),
        }
    };

    if args.time_only {
        // No parity pass here on purpose: --time-only exists to measure generation and nothing
        // else, and reading region files off disk mid-run would be measured along with it.
        let chunks = generate_all(&positions);
        std::hint::black_box(&chunks);
        println!(
            "generated {} chunks on {} thread(s)",
            chunks.len(),
            args.threads
        );
        #[cfg(feature = "profile")]
        write_profile(guard, args.profile_out.as_deref())?;
        return Ok(());
    }

    let mut air_below_zero = 0usize;
    let mut underground_water = 0usize;
    let mut underground_lava = 0usize;
    let mut vein_census: std::collections::BTreeMap<String, usize> = Default::default();
    let mut surface_census: std::collections::BTreeMap<String, usize> = Default::default();
    let mut total = 0usize;
    let mut with_failures = 0usize;
    let mut failure_counts: std::collections::HashMap<&'static str, usize> = Default::default();

    let reference_world = match args.reference.as_deref() {
        Some(dir) => Some(
            ReferenceWorld::open(dir, args.reference_dimension_dir.as_deref())
                .map_err(|e| anyhow!("{e}"))?,
        ),
        None => None,
    };
    let mut reference_compared = 0usize;
    let mut reference_missing = 0usize;
    let mut reference_diverged = 0usize;

    let chunks = generate_all(&positions);
    for (pos, chunk) in positions.iter().copied().zip(chunks) {
        {
            let (cx, cz) = (pos.x, pos.z);
            total += 1;
            let tree = build_merkle(&chunk, args.leaf_size);

            // Determinism self-check: the same seed and position must fill identically every
            // time, with no vanilla reference needed to catch a regression here. Exercises
            // `diverging_sections` for real, not just in unit tests, since there's no vanilla
            // tree to diff against yet (see module doc).
            let rebuilt = generate(pos);
            let rebuilt_tree = build_merkle(&rebuilt, args.leaf_size);
            let nondeterministic_sections = diverging_sections(&tree, &rebuilt_tree);

            // Vanilla parity. Runs per chunk rather than as a second pass so a divergence is
            // reported next to the chunk that produced it, and so the generated chunk does not
            // have to be kept alive twice.
            if let Some(world) = reference_world.as_ref() {
                match world.chunk(pos).map_err(|e| anyhow!("{e}"))? {
                    None => reference_missing += 1,
                    Some(vanilla) => {
                        reference_compared += 1;
                        let report = compare_chunks(
                            &chunk,
                            &vanilla,
                            args.leaf_size,
                            args.max_reported_blocks,
                        );
                        if !report.identical {
                            reference_diverged += 1;
                            println!(
                                "chunk {cx},{cz}: {} block(s) differ from vanilla in section(s) {:?}",
                                report.total_differing_blocks, report.diverging_sections
                            );
                            for missing in &report.section_presence {
                                println!(
                                    "  section {} exists only in {}",
                                    missing.section_y, missing.only_in
                                );
                            }
                            for block in &report.blocks {
                                println!(
                                    "  {} {} {}: oxide {} / vanilla {}",
                                    block.x, block.y, block.z, block.oxide, block.vanilla
                                );
                            }
                            if report.total_differing_blocks > report.blocks.len() {
                                println!(
                                    "  ... {} more not shown (raise --max-reported-blocks)",
                                    report.total_differing_blocks - report.blocks.len()
                                );
                            }
                        }
                    }
                }
            }

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
    match reference_world.as_ref() {
        None => println!(
            "\nno --reference world given — this run reports self-consistency only, not Java \
             parity. See docs/REFERENCE_DATA.md for producing one."
        ),
        Some(_) => {
            println!();
            println!("vanilla parity:");
            println!("  compared:   {reference_compared}");
            println!("  identical:  {}", reference_compared - reference_diverged);
            println!("  diverged:   {reference_diverged}");
            if reference_missing > 0 {
                println!(
                    "  skipped:    {reference_missing} (not generated in the reference world)"
                );
            }
            if reference_compared == 0 {
                println!(
                    "  nothing was compared: the reference world has none of the chunks this \
                     run generated. Check --radius and that the world covers the origin."
                );
            }
        }
    }

    #[cfg(feature = "profile")]
    write_profile(guard, args.profile_out.as_deref())?;

    if with_failures > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Writes the sampling profile, if one was requested. `.folded` asks for collapsed stacks --
/// one line per stack, which is what a self-time table is derived from -- and anything else for
/// a flamegraph SVG.
#[cfg(feature = "profile")]
fn write_profile(
    guard: Option<pprof::ProfilerGuard<'_>>,
    path: Option<&std::path::Path>,
) -> Result<()> {
    let (Some(guard), Some(path)) = (guard, path) else {
        return Ok(());
    };
    let report = guard.report().build().context("building profile report")?;
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    if path.extension().is_some_and(|e| e == "folded") {
        use std::io::Write;
        let mut out = std::io::BufWriter::new(file);
        for (frames, count) in report.data.iter() {
            let mut names: Vec<String> = frames
                .frames
                .iter()
                .map(|level| level.iter().map(|s| s.name()).collect::<Vec<_>>().join("|"))
                .collect();
            names.reverse();
            writeln!(out, "{} {}", names.join(";"), count)?;
        }
    } else {
        report.flamegraph(file).context("writing flamegraph")?;
    }
    println!("profile written to {}", path.display());
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
