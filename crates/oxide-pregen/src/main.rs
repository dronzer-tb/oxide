//! oxide-pregen: generates a square of chunks with Oxide and writes them straight into a world's
//! region files, with the server stopped. No server pipeline, so it runs at Oxide's own speed.
//!
//! What a pregenerated chunk holds, and what the server still does when it first loads one:
//!
//! - Default: terrain, biomes and heightmaps, saved at status `carvers`. The server places
//!   features and lights the chunk on first load, as it would for any half-generated chunk.
//! - `--decorate`: also Oxide's native decoration, with its block entities, scheduled ticks and
//!   post-processing marks, saved at status `features`. The server only lights it.
//!
//! **No structures** either way. The server places structures before terrain, so a chunk saved
//! past that point never gets them, and Oxide's own structure placement is not yet at parity.
//! Pregenerate only where villages, strongholds and the like don't matter, or regenerate those
//! areas on the server.

use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Parser;
use rayon::prelude::*;

use oxide_anvil::nbt::ChunkNbtWriteOptions;
use oxide_anvil::region;
use oxide_core::ChunkPos;
use oxide_ffi::OxideGeneratorHandle;

#[derive(Parser, Debug)]
#[command(
    name = "oxide-pregen",
    version,
    about = "Pregenerate a world offline with Oxide (no structures; see the crate docs)"
)]
struct Cli {
    /// World directory (holds session.lock and dimensions/).
    #[arg(long)]
    world: PathBuf,
    /// Datapack directory Oxide reads worldgen from (the server's exported data).
    #[arg(long)]
    datapack: PathBuf,
    #[arg(long, allow_hyphen_values = true)]
    seed: i64,
    #[arg(long, default_value = "minecraft:overworld")]
    dimension: String,
    /// Center, in chunk coordinates.
    #[arg(long, allow_hyphen_values = true, default_value_t = 0)]
    center_x: i32,
    #[arg(long, allow_hyphen_values = true, default_value_t = 0)]
    center_z: i32,
    /// Radius in chunks: a square of side 2*radius+1.
    #[arg(long)]
    radius: i32,
    /// DataVersion to stamp on every chunk; the server's own, from its version.json.
    #[arg(long)]
    data_version: i32,
    /// Also run Oxide's native decoration (slower; status `features`).
    #[arg(long)]
    decorate: bool,
    /// Replace region files that already exist. Without it the run refuses to start.
    #[arg(long)]
    overwrite: bool,
    /// Worker threads (default: all cores).
    #[arg(long)]
    threads: Option<usize>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(threads) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()?;
    }
    refuse_if_running(&cli.world)?;

    let (namespace, path) = cli
        .dimension
        .split_once(':')
        .unwrap_or(("minecraft", &cli.dimension));
    let region_dir = cli
        .world
        .join("dimensions")
        .join(namespace)
        .join(path)
        .join("region");
    fs::create_dir_all(&region_dir)
        .with_context(|| format!("creating {}", region_dir.display()))?;

    let r = cli.radius;
    let (x0, x1) = (cli.center_x - r, cli.center_x + r);
    let (z0, z1) = (cli.center_z - r, cli.center_z + r);
    let mut regions: Vec<(i32, i32)> = Vec::new();
    for rz in z0.div_euclid(32)..=z1.div_euclid(32) {
        for rx in x0.div_euclid(32)..=x1.div_euclid(32) {
            regions.push((rx, rz));
        }
    }
    if !cli.overwrite {
        let existing: Vec<_> = regions
            .iter()
            .map(|(rx, rz)| region_dir.join(format!("r.{rx}.{rz}.mca")))
            .filter(|p| p.exists())
            .collect();
        if !existing.is_empty() {
            bail!(
                "{} region file(s) already exist, e.g. {}; pass --overwrite to replace them",
                existing.len(),
                existing[0].display()
            );
        }
    }

    let generator = OxideGeneratorHandle::open(&cli.datapack, &cli.dimension, cli.seed)?;
    if cli.decorate && !generator.set_decoration(true)? {
        bail!("native decoration is unavailable for {}", cli.dimension);
    }
    let opts = ChunkNbtWriteOptions {
        data_version: cli.data_version,
        last_update: 0,
        inhabited_time: 0,
    };
    let total = ((2 * r + 1) as u64).pow(2);
    println!(
        "oxide-pregen: {total} chunks in {} regions -> {}",
        regions.len(),
        region_dir.display()
    );

    // Generation fills every core; writing a finished region overlaps with the next one.
    let (send, receive) = mpsc::sync_channel::<(PathBuf, Vec<(ChunkPos, Vec<u8>)>)>(2);
    let writer = thread::spawn(move || -> Result<()> {
        for (path, chunks) in receive {
            region::write_compressed_region(&path, &chunks)
                .with_context(|| format!("writing {}", path.display()))?;
        }
        Ok(())
    });

    let start = Instant::now();
    let mut done = 0u64;
    for (rx, rz) in regions {
        let positions: Vec<ChunkPos> = (0..32 * 32)
            .map(|i| ChunkPos::new(rx * 32 + i % 32, rz * 32 + i / 32))
            .filter(|p| (x0..=x1).contains(&p.x) && (z0..=z1).contains(&p.z))
            .collect();
        let chunks = positions
            .par_iter()
            .map(|&pos| {
                let (chunk, extras) = generator.chunk_data(pos.x, pos.z);
                Ok((
                    pos,
                    oxide_pregen::chunk_bytes(&chunk, &extras, &opts)
                        .with_context(|| format!("chunk {pos:?}"))?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        done += chunks.len() as u64;
        if send
            .send((region_dir.join(format!("r.{rx}.{rz}.mca")), chunks))
            .is_err()
        {
            break; // the writer failed; its error is reported below
        }
        let secs = start.elapsed().as_secs_f64();
        println!("{done}/{total} chunks, {:.0} cps", done as f64 / secs);
    }
    drop(send);
    writer.join().expect("writer thread panicked")?;

    let secs = start.elapsed().as_secs_f64();
    println!(
        "done: {done} chunks in {secs:.1} s, {:.0} cps",
        done as f64 / secs
    );
    Ok(())
}

/// The server holds a POSIX lock on `session.lock` while the world is open; writing region files
/// under it would be overwritten or corrupt the save.
fn refuse_if_running(world: &Path) -> Result<()> {
    let lock = world.join("session.lock");
    let Ok(file) = OpenOptions::new().read(true).write(true).open(&lock) else {
        return Ok(()); // a world the server never opened
    };
    let mut probe = libc::flock {
        l_type: libc::F_WRLCK as _,
        l_whence: libc::SEEK_SET as _,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };
    // SAFETY: F_GETLK only fills `probe`; the descriptor is open for the call's duration.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut probe) } != 0 {
        bail!(
            "could not check {}: {}",
            lock.display(),
            std::io::Error::last_os_error()
        );
    }
    if probe.l_type != libc::F_UNLCK as libc::c_short {
        bail!(
            "{} is locked by pid {}: stop the server first",
            lock.display(),
            probe.l_pid
        );
    }
    Ok(())
}
