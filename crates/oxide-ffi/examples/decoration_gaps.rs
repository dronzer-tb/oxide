//! Generates a square of decorated chunks and prints which feature types and block rules the
//! native decorator skipped, with counts: what stands between native decoration and a world
//! with every feature in it.
//!
//! `cargo run --release -p oxide-ffi --example decoration_gaps -- <datapack> <seed> <radius>`

use anyhow::{Context, Result};
use oxide_ffi::OxideGeneratorHandle;
use rayon::prelude::*;
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let datapack = args.get(1).context("datapack path")?;
    let seed: i64 = args.get(2).context("seed")?.parse()?;
    let radius: i32 = args.get(3).map(|r| r.parse()).transpose()?.unwrap_or(16);

    let generator = OxideGeneratorHandle::open(Path::new(datapack), "minecraft:overworld", seed)?;
    generator.set_decoration(true)?;
    let coords: Vec<(i32, i32)> = (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| (x, z)))
        .collect();
    coords.par_iter().try_for_each(|&(x, z)| -> Result<()> {
        let mut blocks = vec![0u16; generator.block_buffer_len()];
        let mut biomes = vec![0u16; generator.biome_buffer_len()];
        generator.generate_chunk(x, z, &mut blocks, &mut biomes)
    })?;

    let mut gaps = generator.decoration_gaps();
    gaps.sort_by(|a, b| b.1.cmp(&a.1));
    println!("{} chunks, {} gap kinds", coords.len(), gaps.len());
    for (key, count) in gaps {
        println!("{count:>10}  {key}");
    }
    Ok(())
}
