//! oxide-pregen: offline world pregeneration, with no server in the path.
//!
//! With `--datapack`, generates real chunks (terrain, decoration, heightmaps) through
//! `oxide_pregen::generate` and writes them as Anvil regions, timing each stage; `--bench`
//! prints one `BENCH` line for `scripts/bench-pregen-vps.sh`. Chunks carry no light and are
//! marked for relight (`docs/ARCHITECTURE.md`, "Chunk output contract").
//!
//! Without `--datapack` it writes `oxide_pregen::stub`'s deliberately non-vanilla checkerboard,
//! the smoke test for the NBT/region write path it started as.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use rayon::prelude::*;

use oxide_anvil::{nbt, provenance, region};
use oxide_core::ChunkPos;

use oxide_anvil::compress::Compression;
use oxide_pregen::generate::{self, Generator};
use oxide_pregen::stub::{self, Pattern};

/// Pregenerate chunks into an Anvil region directory: real generation with `--datapack`,
/// otherwise stub checkerboard terrain.
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
    /// Required without `--datapack`; with it, defaults to the pack's own `version.json`. Per docs/ARCHITECTURE.md, a Minecraft 26.2 DataVersion is never
    /// hardcoded from memory. Read the real value from your own server's exported data: run its
    /// data generator with `--reports` and take the integer out of
    /// `generated/reports/version.json` — see docs/REFERENCE_DATA.md for the exact command. Do
    /// not guess this number.
    #[arg(long)]
    data_version: Option<i32>,

    /// Report what would be written without touching any files.
    #[arg(long)]
    dry_run: bool,

    /// Stub terrain pattern (ignored with `--datapack`).
    #[arg(long, value_enum, default_value = "checkerboard")]
    pattern: Pattern,

    /// Datapack root (the server's exported data). Turns on real generation.
    #[arg(long)]
    datapack: Option<PathBuf>,

    /// World seed, with `--datapack`.
    #[arg(long, default_value_t = 12345, allow_hyphen_values = true)]
    seed: i64,

    /// Dimension to generate, with `--datapack`.
    #[arg(long, default_value = "minecraft:overworld")]
    dimension: String,

    /// Worker threads (default: every core).
    #[arg(long)]
    threads: Option<usize>,

    /// Chunk compression in region files.
    #[arg(long, value_enum, default_value = "zlib")]
    compression: CompressionArg,

    /// zlib level 0..=9 for `--compression zlib`; vanilla writes 6.
    #[arg(long, default_value_t = 6)]
    zlib_level: u32,

    /// fsync each region before renaming it into place.
    #[arg(long)]
    sync: bool,

    /// Generate, serialise and compress, but write no region files.
    #[arg(long)]
    no_write: bool,

    /// Print one `BENCH` line with per-stage timings.
    #[arg(long)]
    bench: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum CompressionArg {
    Zlib,
    None,
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

/// Grows the calling thread's malloc arena up front, by allocating and freeing blocks of
/// address space without touching them.
///
/// glibc gives each worker thread its own arena, made of 64 MiB heaps, and grows a heap only as
/// far as each allocation needs, one `mprotect` per growth, usually of a page or two: an r16
/// run at eight threads made about 53,000 of them. `mprotect` takes the process's memory-map
/// lock for writing, so with eight threads faulting pages in under the same lock those calls
/// queued behind one another. Their waits showed up as several seconds of system time, up to
/// 20 s in bad runs. A heap keeps the write permission it was given (glibc releases trimmed
/// pages with `madvise`), so growing two heaps here, before any other work, lets the arena
/// grow within them later with no `mprotect`. The block that opened the second heap stays
/// allocated, or freeing it would unmap that heap. Nothing is written, so no page is faulted in
/// and no memory is used; on other allocators it is a short burst of allocations and frees.
fn reserve_arena() {
    // Below glibc's 128 KiB mmap threshold, so the arena serves each block.
    const BLOCK: usize = 120 * 1024;
    const HEAP: usize = 64 * 1024 * 1024;
    const BLOCKS: usize = 2 * HEAP / BLOCK - 16;
    let mut held: Vec<Vec<u8>> = (0..BLOCKS).map(|_| Vec::with_capacity(BLOCK)).collect();
    let heap_of = |block: &Vec<u8>| block.as_ptr() as usize & !(HEAP - 1);
    let first = heap_of(&held[0]);
    if let Some(second) = held.iter().position(|block| heap_of(block) != first) {
        std::mem::forget(held.swap_remove(second));
    }
    drop(std::hint::black_box(held));
}

/// One region's worth of already-serialized chunks, keyed by region coordinates.
type ChunksByRegion = BTreeMap<(i32, i32), Vec<(ChunkPos, Vec<u8>)>>;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    if let Some(threads) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .context("building the thread pool")?;
    }
    rayon::broadcast(|_| reserve_arena());
    if cli.datapack.is_some() {
        return real(&cli);
    }
    let Some(data_version) = cli.data_version else {
        anyhow::bail!("--data-version is required without --datapack (see its --help)");
    };
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
            data_version
        );
        return Ok(());
    }

    let region_dir = cli.world.join("region");
    fs::create_dir_all(&region_dir)
        .with_context(|| format!("creating region directory {}", region_dir.display()))?;

    let write_opts = nbt::ChunkNbtWriteOptions {
        data_version,
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
        "\nThis is stub terrain (see src/stub.rs), not generated output; pass --datapack for \
         real chunks. DataVersion {} was supplied by you on the command line and has NOT been \
         validated against a real server.",
        data_version
    );

    Ok(())
}

/// `--datapack`: real generation.
fn real(cli: &Cli) -> Result<()> {
    let datapack = cli.datapack.as_deref().expect("checked by the caller");
    let radius = i32::try_from(cli.radius).context("--radius does not fit in i32")?;
    let region_dir = cli.world.join("region");
    if cli.dry_run {
        println!(
            "[dry-run] {} chunk(s) into {}; nothing written.",
            (2 * radius as u64 + 1).pow(2),
            region_dir.display()
        );
        return Ok(());
    }
    let started = std::time::Instant::now();
    let mut generator = Generator::open(datapack, &cli.dimension, cli.seed)?;
    if let Some(v) = cli.data_version {
        generator.data_version = v;
    }
    let setup = started.elapsed().as_secs_f64();
    let compression = match cli.compression {
        CompressionArg::Zlib => Compression::Zlib {
            level: cli.zlib_level.min(9),
        },
        CompressionArg::None => Compression::None,
    };
    let outcome = generate::run(
        &generator,
        &generate::Options {
            region_dir: &region_dir,
            center: ChunkPos::new(cli.center_x, cli.center_z),
            radius,
            compression,
            sync: cli.sync,
            write: !cli.no_write,
        },
    )?;
    let t = outcome.times;
    let secs = t.total();
    let threads = rayon::current_num_threads();
    println!(
        "{} chunks in {} region(s), {:.1} MB NBT, {:.1} MB on disk, setup {:.2}s",
        outcome.chunks,
        outcome.regions,
        outcome.bytes_nbt as f64 / 1e6,
        outcome.bytes_written as f64 / 1e6,
        setup
    );
    if cli.bench {
        // Same leading fields as the harness's BENCH line, so the progress page reads it; the
        // stage times are wall milliseconds per stage, all threads together.
        let ms = |s: f64| s * 1000.0;
        println!(
            "BENCH chunks={} threads={} secs={:.3} cps={:.1} ms_per_chunk_core={:.2} digest={} \
             generate_ms={:.0} decorate_ms={:.0} serialise_ms={:.0} compress_ms={:.0} write_ms={:.0} \
             compression={} mb_disk={:.1}",
            outcome.chunks,
            threads,
            secs,
            outcome.chunks as f64 / secs,
            secs * 1000.0 * threads as f64 / outcome.chunks as f64,
            outcome.digest,
            ms(t.generate),
            ms(t.decorate),
            ms(t.serialise),
            ms(t.compress),
            ms(t.write),
            match compression {
                Compression::Zlib { level } => format!("zlib{level}"),
                Compression::None => "none".to_string(),
            },
            outcome.bytes_written as f64 / 1e6
        );
    }
    Ok(())
}
