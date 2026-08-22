//! oxide-pregen — v0 smoke test for the Oxide write path.
//!
//! This does **not** generate real terrain: `oxide-noise`, `oxide-biome`, and `oxide-chunkgen`
//! are still empty stubs (see `docs/ARCHITECTURE.md`). What's under test here is everything
//! downstream of generation — chunk NBT correctness, `.mca` region writing without corruption,
//! provenance sidecar marking, and whether a real client/server loads what Oxide wrote. Terrain
//! comes from `oxide_pregen::stub`, a deliberately fake, deliberately non-vanilla generator
//! chosen specifically to make the Oxide/Java boundary visible from the air. Do not mistake this
//! output for parity work, and do not add real generation logic here — that belongs upstream.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use rayon::prelude::*;

use oxide_anvil::{nbt, provenance, region};
use oxide_core::ChunkPos;

use oxide_pregen::stub::{self, Pattern};

/// Write stub (deliberately non-vanilla) terrain chunks into an Anvil region directory.
///
/// This is the v0 smoke test for chunk NBT + `.mca` writing + provenance — NOT a terrain
/// generator. Real generation is unimplemented upstream (`oxide-noise`/`-biome`/`-chunkgen` are
/// still empty stubs); see `docs/ARCHITECTURE.md`.
#[derive(Parser, Debug)]
#[command(name = "oxide-pregen", version, about, long_about = None)]
struct Cli {
    /// World directory. Chunks are written into <world>/region/.
    #[arg(long)]
    world: PathBuf,

    /// Center chunk X coordinate.
    #[arg(long, allow_hyphen_values = true)]
    center_x: i32,

    /// Center chunk Z coordinate.
    #[arg(long, allow_hyphen_values = true)]
    center_z: i32,

    /// Radius in chunks around the center (inclusive square: side length is 2*radius + 1).
    #[arg(long)]
    radius: u32,

    /// Chunk-format DataVersion to stamp on every chunk written.
    ///
    /// REQUIRED, no default. Per docs/ARCHITECTURE.md, a Minecraft 26.2 DataVersion is never
    /// hardcoded from memory. Read the real value from your own server's exported data: run its
    /// data generator with `--reports` and take the integer out of
    /// `generated/reports/version.json` — see docs/REFERENCE_DATA.md for the exact command. Do
    /// not guess this number.
    #[arg(long)]
    data_version: i32,

    /// Report what would be written without touching any files.
    #[arg(long)]
    dry_run: bool,

    /// Stub terrain pattern.
    #[arg(long, value_enum, default_value = "checkerboard")]
    pattern: Pattern,
}

/// Every chunk position in the inclusive square of `radius` chunks around `(center_x, center_z)`.
fn chunk_positions(center_x: i32, center_z: i32, radius: i32) -> Result<Vec<ChunkPos>> {
    let x_lo = center_x
        .checked_sub(radius)
        .context("center-x - radius overflowed i32")?;
    let x_hi = center_x
        .checked_add(radius)
        .context("center-x + radius overflowed i32")?;
    let z_lo = center_z
        .checked_sub(radius)
        .context("center-z - radius overflowed i32")?;
    let z_hi = center_z
        .checked_add(radius)
        .context("center-z + radius overflowed i32")?;

    let mut positions = Vec::new();
    for x in x_lo..=x_hi {
        for z in z_lo..=z_hi {
            positions.push(ChunkPos::new(x, z));
        }
    }
    Ok(positions)
}

struct RegionReport {
    path: PathBuf,
    chunks_written: usize,
    sidecar_bits_set: u32,
    file_bytes: u64,
}

/// One region's worth of already-serialized chunks, keyed by region coordinates.
type ChunksByRegion = BTreeMap<(i32, i32), Vec<(ChunkPos, Vec<u8>)>>;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let radius = i32::try_from(cli.radius).context("--radius does not fit in i32")?;
    let positions = chunk_positions(cli.center_x, cli.center_z, radius)?;

    let mut regions_planned: BTreeMap<(i32, i32), usize> = BTreeMap::new();
    for &pos in &positions {
        *regions_planned
            .entry(region::region_coords_for(&pos))
            .or_default() += 1;
    }

    if cli.dry_run {
        for (rx, rz) in regions_planned.keys() {
            let path = cli.world.join("region").join(format!("r.{rx}.{rz}.mca"));
            println!("[dry-run] would touch {}", path.display());
        }
        println!(
            "[dry-run] {} chunk(s) across {} region file(s); pattern={:?}, data-version={}; nothing written.",
            positions.len(),
            regions_planned.len(),
            cli.pattern,
            cli.data_version
        );
        return Ok(());
    }

    let region_dir = cli.world.join("region");
    fs::create_dir_all(&region_dir)
        .with_context(|| format!("creating region directory {}", region_dir.display()))?;

    let write_opts = nbt::ChunkNbtWriteOptions {
        data_version: cli.data_version,
        last_update: 0,
        inhabited_time: 0,
    };

    // Build + serialize every chunk in parallel. The actual region-file write happens below, one
    // region at a time, each region's chunks written in sequence within its own task, so
    // `RegionGuard` is never contended against itself for the same file.
    let built: Vec<(ChunkPos, Vec<u8>)> = positions
        .par_iter()
        .map(|&pos| -> Result<(ChunkPos, Vec<u8>)> {
            let chunk = stub::build_chunk(pos, cli.pattern);
            let bytes = nbt::serialize_chunk(&chunk, &write_opts)
                .with_context(|| format!("serializing chunk {pos:?}"))?;
            Ok((pos, bytes))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut by_region: ChunksByRegion = BTreeMap::new();
    for (pos, bytes) in built {
        by_region
            .entry(region::region_coords_for(&pos))
            .or_default()
            .push((pos, bytes));
    }

    let reports: Vec<RegionReport> = by_region
        .into_par_iter()
        .map(|((rx, rz), chunks)| -> Result<RegionReport> {
            let path = region_dir.join(format!("r.{rx}.{rz}.mca"));
            for (pos, bytes) in &chunks {
                region::update_chunk_in_place(&path, pos, bytes)
                    .with_context(|| format!("writing chunk {pos:?} into {}", path.display()))?;
            }

            let sidecar_path = provenance::sidecar_path_for(&path);
            let sidecar_bits_set = provenance::load(&sidecar_path)
                .with_context(|| format!("reading provenance sidecar {}", sidecar_path.display()))?
                .count_set();
            let file_bytes = fs::metadata(&path)
                .with_context(|| format!("stat-ing {}", path.display()))?
                .len();

            tracing::info!(
                region = %path.display(),
                chunks_written = chunks.len(),
                sidecar_bits_set,
                file_bytes,
                "region written"
            );

            Ok(RegionReport {
                path,
                chunks_written: chunks.len(),
                sidecar_bits_set,
                file_bytes,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    println!("\noxide-pregen: {} region file(s) touched:", reports.len());
    for r in &reports {
        println!(
            "  {} — {} chunk(s), {} provenance bit(s) set, {} bytes",
            r.path.display(),
            r.chunks_written,
            r.sidecar_bits_set,
            r.file_bytes
        );
    }
    println!(
        "\nThis is stub terrain (see src/stub.rs) — not generated output. oxide-noise, \
         oxide-biome, and oxide-chunkgen are still empty stubs. DataVersion {} was supplied by \
         you on the command line and has NOT been validated against a real server.",
        cli.data_version
    );

    Ok(())
}
